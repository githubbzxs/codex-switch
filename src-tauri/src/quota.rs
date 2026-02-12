use anyhow::{anyhow, Context, Result};
use regex::Regex;
use reqwest::{header, Client, StatusCode};
use serde_json::Value;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct QuotaProbeResult {
    pub mode: String,
    pub remaining_value: Option<f64>,
    pub remaining_unit: Option<String>,
    pub quota_state: String,
    pub reset_at: Option<String>,
    pub source: String,
    pub confidence: i64,
    pub reason: Option<String>,
}

impl QuotaProbeResult {
    pub fn unavailable(reason: &str, source: &str) -> Self {
        Self {
            mode: "state".to_string(),
            remaining_value: None,
            remaining_unit: None,
            quota_state: "unknown".to_string(),
            reset_at: None,
            source: source.to_string(),
            confidence: 20,
            reason: Some(reason.to_string()),
        }
    }
}

pub async fn probe_quota(access_token: &str, timeout_ms: u64) -> QuotaProbeResult {
    let (api_result, web_result) = tokio::join!(
        probe_via_api(access_token, timeout_ms),
        probe_via_web(access_token, timeout_ms)
    );
    merge_probe_results(api_result, web_result)
}

async fn probe_via_api(access_token: &str, timeout_ms: u64) -> Result<QuotaProbeResult> {
    let endpoints = [
        "https://chat.openai.com/backend-api/codex/usage",
        "https://chat.openai.com/backend-api/usage",
    ];
    let client = build_client(timeout_ms)?;
    let mut last_reason = "source_unavailable".to_string();

    for endpoint in endpoints {
        let response = client
            .get(endpoint)
            .bearer_auth(access_token)
            .send()
            .await
            .with_context(|| format!("调用配额接口失败: {endpoint}"))?;

        if response.status() == StatusCode::UNAUTHORIZED {
            return Ok(QuotaProbeResult::unavailable("auth_expired", "api"));
        }
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            return Ok(QuotaProbeResult {
                mode: "state".to_string(),
                remaining_value: None,
                remaining_unit: None,
                quota_state: "exhausted".to_string(),
                reset_at: None,
                source: "api".to_string(),
                confidence: 95,
                reason: Some("rate_limited".to_string()),
            });
        }
        if !response.status().is_success() {
            last_reason = reason_from_status(response.status()).to_string();
            continue;
        }

        let json = response
            .json::<Value>()
            .await
            .context("解析配额 JSON 失败")?;
        if let Some(result) = extract_exact_from_json(&json, "api") {
            return Ok(result);
        }

        if let Some(state) = extract_state_from_json(&json, "api") {
            return Ok(state);
        }
    }

    Ok(QuotaProbeResult::unavailable(&last_reason, "api"))
}

async fn probe_via_web(access_token: &str, timeout_ms: u64) -> Result<QuotaProbeResult> {
    let client = build_client(timeout_ms)?;
    let response = client
        .get("https://chat.openai.com/codex")
        .bearer_auth(access_token)
        .send()
        .await
        .context("抓取配额页面失败")?;

    if response.status() == StatusCode::UNAUTHORIZED {
        return Ok(QuotaProbeResult::unavailable("auth_expired", "web"));
    }
    if !response.status().is_success() {
        return Ok(QuotaProbeResult::unavailable(reason_from_status(response.status()), "web"));
    }
    let html = response.text().await.context("读取配额页面内容失败")?;
    if let Some(result) = extract_from_html(&html) {
        return Ok(result);
    }

    Ok(QuotaProbeResult::unavailable("parse_failed", "web"))
}

fn merge_probe_results(
    api_result: Result<QuotaProbeResult>,
    web_result: Result<QuotaProbeResult>,
) -> QuotaProbeResult {
    let api =
        api_result.unwrap_or_else(|_| QuotaProbeResult::unavailable("source_unavailable", "api"));
    let web =
        web_result.unwrap_or_else(|_| QuotaProbeResult::unavailable("source_unavailable", "web"));

    let candidates = [api.clone(), web.clone()];
    if let Some(exact) = candidates.iter().find(|item| item.mode == "exact") {
        return exact.clone();
    }

    let preferred = candidates
        .iter()
        .find(|item| item.quota_state != "unknown")
        .cloned();

    preferred.unwrap_or(QuotaProbeResult {
        mode: "state".to_string(),
        remaining_value: None,
        remaining_unit: None,
        quota_state: "unknown".to_string(),
        reset_at: None,
        source: "merged".to_string(),
        confidence: 20,
        reason: Some(format!(
            "api:{}|web:{}",
            api.reason.unwrap_or_else(|| "unknown".to_string()),
            web.reason.unwrap_or_else(|| "unknown".to_string())
        )),
    })
}

fn reason_from_status(status: StatusCode) -> &'static str {
    match status {
        StatusCode::FORBIDDEN => "forbidden",
        StatusCode::NOT_FOUND
        | StatusCode::MOVED_PERMANENTLY
        | StatusCode::FOUND
        | StatusCode::TEMPORARY_REDIRECT
        | StatusCode::PERMANENT_REDIRECT => "endpoint_changed",
        _ => "source_unavailable",
    }
}
fn build_client(timeout_ms: u64) -> Result<Client> {
    let mut headers = header::HeaderMap::new();
    headers.insert(
        header::USER_AGENT,
        header::HeaderValue::from_static("codex-switch/0.1"),
    );
    headers.insert(
        header::ACCEPT,
        header::HeaderValue::from_static("application/json, text/html;q=0.9"),
    );
    Client::builder()
        .timeout(Duration::from_millis(timeout_ms))
        .default_headers(headers)
        .build()
        .context("初始化 HTTP 客户端失败")
}

fn extract_exact_from_json(json: &Value, source: &str) -> Option<QuotaProbeResult> {
    let mut numeric_candidates: Vec<(String, f64)> = Vec::new();
    collect_numeric_candidates("", json, &mut numeric_candidates);
    let remaining = numeric_candidates
        .iter()
        .find(|(path, _)| path.to_lowercase().contains("remaining"))
        .or_else(|| {
            numeric_candidates
                .iter()
                .find(|(path, _)| path.to_lowercase().contains("quota"))
        })
        .map(|(_, value)| *value);

    remaining.map(|value| QuotaProbeResult {
        mode: "exact".to_string(),
        remaining_value: Some(value),
        remaining_unit: extract_text_by_key(json, &["unit", "quota_unit", "remaining_unit"]),
        quota_state: state_from_value(value).to_string(),
        reset_at: extract_text_by_key(json, &["reset_at", "resetAt", "next_reset"]),
        source: source.to_string(),
        confidence: 85,
        reason: None,
    })
}

fn extract_state_from_json(json: &Value, source: &str) -> Option<QuotaProbeResult> {
    let exhausted = find_bool_by_keys(json, &["quota_exhausted", "limit_reached", "exhausted"])
        .unwrap_or(false);
    if exhausted {
        return Some(QuotaProbeResult {
            mode: "state".to_string(),
            remaining_value: None,
            remaining_unit: None,
            quota_state: "exhausted".to_string(),
            reset_at: extract_text_by_key(json, &["reset_at", "resetAt", "next_reset"]),
            source: source.to_string(),
            confidence: 75,
            reason: Some("state_only".to_string()),
        });
    }

    None
}

fn extract_from_html(html: &str) -> Option<QuotaProbeResult> {
    let regex = Regex::new("(?i)(remaining|quota)\\D{0,12}([0-9]+(?:\\.[0-9]+)?)").ok()?;
    if let Some(capture) = regex.captures(html) {
        let value = capture.get(2)?.as_str().parse::<f64>().ok()?;
        return Some(QuotaProbeResult {
            mode: "exact".to_string(),
            remaining_value: Some(value),
            remaining_unit: Some("units".to_string()),
            quota_state: state_from_value(value).to_string(),
            reset_at: None,
            source: "web".to_string(),
            confidence: 60,
            reason: None,
        });
    }
    if html.to_lowercase().contains("limit reached") {
        return Some(QuotaProbeResult {
            mode: "state".to_string(),
            remaining_value: None,
            remaining_unit: None,
            quota_state: "exhausted".to_string(),
            reset_at: None,
            source: "web".to_string(),
            confidence: 55,
            reason: Some("state_only".to_string()),
        });
    }
    None
}

fn collect_numeric_candidates(prefix: &str, value: &Value, out: &mut Vec<(String, f64)>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                let path = if prefix.is_empty() {
                    key.to_string()
                } else {
                    format!("{prefix}.{key}")
                };
                collect_numeric_candidates(&path, child, out);
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                let path = format!("{prefix}[{index}]");
                collect_numeric_candidates(&path, child, out);
            }
        }
        Value::Number(num) => {
            if let Some(value) = num.as_f64() {
                out.push((prefix.to_string(), value));
            }
        }
        _ => {}
    }
}

fn extract_text_by_key(json: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| find_text_by_key(json, key))
}

fn find_text_by_key(value: &Value, target: &str) -> Option<String> {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                if key.eq_ignore_ascii_case(target) {
                    if let Some(text) = child.as_str() {
                        return Some(text.to_string());
                    }
                }
                if let Some(found) = find_text_by_key(child, target) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(items) => items.iter().find_map(|item| find_text_by_key(item, target)),
        _ => None,
    }
}

fn find_bool_by_keys(value: &Value, keys: &[&str]) -> Option<bool> {
    for key in keys {
        if let Some(found) = find_bool_by_key(value, key) {
            return Some(found);
        }
    }
    None
}

fn find_bool_by_key(value: &Value, target: &str) -> Option<bool> {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                if key.eq_ignore_ascii_case(target) {
                    if let Some(flag) = child.as_bool() {
                        return Some(flag);
                    }
                }
                if let Some(found) = find_bool_by_key(child, target) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(items) => items.iter().find_map(|item| find_bool_by_key(item, target)),
        _ => None,
    }
}

fn state_from_value(value: f64) -> &'static str {
    if value <= 0.0 {
        "exhausted"
    } else if value <= 3.0 {
        "near_limit"
    } else {
        "available"
    }
}

pub fn ensure_access_token(auth_json: &Value) -> Result<String> {
    auth_json
        .pointer("/tokens/access_token")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| anyhow!("该账户缺少 access_token，无法查询配额"))
}
