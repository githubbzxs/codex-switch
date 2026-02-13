use anyhow::{anyhow, Context, Result};
use regex::Regex;
use reqwest::{header, Client, RequestBuilder, StatusCode};
use serde_json::Value;
use std::time::Duration;
use uuid::Uuid;

const CODEX_CLIENT_VERSION: &str = "0.98.0";
const CODEX_OPENAI_BETA: &str = "responses=experimental";
const CODEX_USER_AGENT: &str = "codex_cli_rs/0.98.0 (Windows NT 10.0; x86_64) codex-switch";
const CODEX_ORIGINATOR: &str = "codex_cli_rs";
const API_ENDPOINTS: [(&str, &str); 8] = [
    ("https://chatgpt.com", "/backend-api/api/codex/usage"),
    ("https://chatgpt.com", "/backend-api/wham/usage"),
    ("https://chat.openai.com", "/backend-api/api/codex/usage"),
    ("https://chat.openai.com", "/backend-api/wham/usage"),
    ("https://chatgpt.com", "/backend-api/codex/usage"),
    ("https://chatgpt.com", "/backend-api/usage"),
    ("https://chat.openai.com", "/backend-api/codex/usage"),
    ("https://chat.openai.com", "/backend-api/usage"),
];
const WEB_ENDPOINTS: [(&str, &str); 2] = [
    ("https://chatgpt.com", "/codex"),
    ("https://chat.openai.com", "/codex"),
];

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

pub async fn probe_quota(
    access_token: &str,
    account_id: Option<&str>,
    timeout_ms: u64,
) -> QuotaProbeResult {
    let (api_result, web_result) = tokio::join!(
        probe_via_api(access_token, account_id, timeout_ms),
        probe_via_web(access_token, account_id, timeout_ms)
    );
    merge_probe_results(api_result, web_result)
}

async fn probe_via_api(
    access_token: &str,
    account_id: Option<&str>,
    timeout_ms: u64,
) -> Result<QuotaProbeResult> {
    let client = build_client(timeout_ms)?;
    let mut last_reason = "source_unavailable".to_string();

    for (domain, path) in API_ENDPOINTS {
        let endpoint = format!("{domain}{path}");
        let request = apply_codex_headers(
            client.get(&endpoint),
            access_token,
            account_id,
            "application/json",
        );
        let response = match request.send().await {
            Ok(response) => response,
            Err(error) => {
                last_reason = reason_from_request_error(&error, &endpoint);
                continue;
            }
        };

        let status = response.status();
        if status == StatusCode::UNAUTHORIZED {
            return Ok(QuotaProbeResult::unavailable(
                &reason_from_http_status(status, &endpoint),
                "api",
            ));
        }

        if status == StatusCode::TOO_MANY_REQUESTS {
            return Ok(QuotaProbeResult {
                mode: "state".to_string(),
                remaining_value: None,
                remaining_unit: None,
                quota_state: "exhausted".to_string(),
                reset_at: None,
                source: "api".to_string(),
                confidence: 95,
                reason: Some(reason_from_http_status(status, &endpoint)),
            });
        }

        if !status.is_success() {
            last_reason = reason_from_http_status(status, &endpoint);
            continue;
        }

        if let Some(result) = extract_from_codex_headers(response.headers(), "api", &endpoint) {
            return Ok(result);
        }

        let json = match response.json::<Value>().await {
            Ok(json) => json,
            Err(error) => {
                last_reason = format!("json_parse_failed@{}:{}", endpoint, short_error(&error));
                continue;
            }
        };

        if let Some(result) = extract_exact_from_json(&json, "api") {
            return Ok(result);
        }

        if let Some(state) = extract_state_from_json(&json, "api") {
            return Ok(state);
        }

        last_reason = format!("quota_field_not_found@{endpoint}");
    }

    Ok(QuotaProbeResult::unavailable(&last_reason, "api"))
}

async fn probe_via_web(
    access_token: &str,
    account_id: Option<&str>,
    timeout_ms: u64,
) -> Result<QuotaProbeResult> {
    let client = build_client(timeout_ms)?;
    let mut last_reason = "source_unavailable".to_string();

    for (domain, path) in WEB_ENDPOINTS {
        let endpoint = format!("{domain}{path}");
        let request = apply_codex_headers(
            client.get(&endpoint),
            access_token,
            account_id,
            "text/html,application/xhtml+xml",
        );
        let response = match request.send().await {
            Ok(response) => response,
            Err(error) => {
                last_reason = reason_from_request_error(&error, &endpoint);
                continue;
            }
        };

        let status = response.status();
        if status == StatusCode::UNAUTHORIZED {
            return Ok(QuotaProbeResult::unavailable(
                &reason_from_http_status(status, &endpoint),
                "web",
            ));
        }

        if !status.is_success() {
            last_reason = reason_from_http_status(status, &endpoint);
            continue;
        }

        let html = match response.text().await {
            Ok(text) => text,
            Err(error) => {
                last_reason = format!("html_read_failed@{}:{}", endpoint, short_error(&error));
                continue;
            }
        };
        if let Some(result) = extract_from_html(&html) {
            return Ok(result);
        }
        last_reason = format!("html_parse_failed@{endpoint}");
    }

    Ok(QuotaProbeResult::unavailable(&last_reason, "web"))
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

fn apply_codex_headers(
    request: RequestBuilder,
    access_token: &str,
    account_id: Option<&str>,
    accept: &'static str,
) -> RequestBuilder {
    let mut request = request
        .bearer_auth(access_token)
        .header("Version", CODEX_CLIENT_VERSION)
        .header("Openai-Beta", CODEX_OPENAI_BETA)
        .header("Session_id", Uuid::new_v4().to_string())
        .header(header::USER_AGENT, CODEX_USER_AGENT)
        .header("Originator", CODEX_ORIGINATOR)
        .header(header::ACCEPT, accept)
        .header(header::CONNECTION, "Keep-Alive");

    if let Some(account_id) = account_id.map(str::trim).filter(|value| !value.is_empty()) {
        request = request.header("Chatgpt-Account-Id", account_id);
    }
    request
}

fn reason_from_http_status(status: StatusCode, endpoint: &str) -> String {
    let reason = match status {
        StatusCode::UNAUTHORIZED => "auth_expired",
        StatusCode::FORBIDDEN => "auth_forbidden",
        StatusCode::NOT_FOUND => "endpoint_not_found",
        StatusCode::TOO_MANY_REQUESTS => "rate_limited",
        StatusCode::MOVED_PERMANENTLY
        | StatusCode::FOUND
        | StatusCode::TEMPORARY_REDIRECT
        | StatusCode::PERMANENT_REDIRECT => "endpoint_redirected",
        StatusCode::REQUEST_TIMEOUT | StatusCode::GATEWAY_TIMEOUT => "upstream_timeout",
        StatusCode::BAD_GATEWAY
        | StatusCode::SERVICE_UNAVAILABLE
        | StatusCode::INTERNAL_SERVER_ERROR => "upstream_unavailable",
        _ if status.is_client_error() => "client_error",
        _ if status.is_server_error() => "server_error",
        _ => "source_unavailable",
    };
    format!("{reason}@{}:{endpoint}", status.as_u16())
}

fn reason_from_request_error(error: &reqwest::Error, endpoint: &str) -> String {
    let reason = if error.is_timeout() {
        "request_timeout"
    } else if error.is_connect() {
        "connect_failed"
    } else if error.is_request() {
        "request_build_failed"
    } else if error.is_decode() {
        "response_decode_failed"
    } else {
        "request_failed"
    };
    format!("{reason}@{endpoint}")
}

fn short_error(error: &impl std::fmt::Display) -> String {
    let message = error.to_string();
    let compact = message.replace('\n', " ").replace('\r', " ");
    compact.chars().take(120).collect()
}

fn build_client(timeout_ms: u64) -> Result<Client> {
    let mut headers = header::HeaderMap::new();
    headers.insert(
        header::USER_AGENT,
        header::HeaderValue::from_static(CODEX_USER_AGENT),
    );
    headers.insert(
        header::ACCEPT,
        header::HeaderValue::from_static("application/json, text/html;q=0.9"),
    );
    Client::builder()
        .timeout(Duration::from_millis(timeout_ms))
        .default_headers(headers)
        .build()
        .context("初始化配额 HTTP 客户端失败")
}

fn extract_from_codex_headers(
    headers: &header::HeaderMap,
    source: &str,
    endpoint: &str,
) -> Option<QuotaProbeResult> {
    const REMAINING_KEYS: &[&str] = &[
        "x-codex-remaining",
        "x-codex-remaining-quota",
        "x-codex-usage-remaining",
        "x-codex-quota-remaining",
    ];
    const USED_KEYS: &[&str] = &["x-codex-used", "x-codex-usage-used", "x-codex-consumed"];
    const LIMIT_KEYS: &[&str] = &["x-codex-limit", "x-codex-usage-limit", "x-codex-total"];
    const UNIT_KEYS: &[&str] = &["x-codex-unit", "x-codex-quota-unit", "x-codex-remaining-unit"];
    const RESET_KEYS: &[&str] = &[
        "x-codex-reset-at",
        "x-codex-reset",
        "x-codex-reset-ts",
        "x-codex-reset-time",
    ];
    const STATE_KEYS: &[&str] = &["x-codex-state", "x-codex-quota-state", "x-codex-usage-state"];

    let remaining = extract_header_number_by_keys(headers, REMAINING_KEYS).or_else(|| {
        let used = extract_header_number_by_keys(headers, USED_KEYS)?;
        let limit = extract_header_number_by_keys(headers, LIMIT_KEYS)?;
        Some((limit - used).max(0.0))
    });
    let unit = extract_header_text_by_keys(headers, UNIT_KEYS);
    let reset_at = extract_header_text_by_keys(headers, RESET_KEYS);
    let state_from_header = extract_header_text_by_keys(headers, STATE_KEYS)
        .as_deref()
        .and_then(normalize_quota_state);

    if let Some(value) = remaining {
        return Some(QuotaProbeResult {
            mode: "exact".to_string(),
            remaining_value: Some(value),
            remaining_unit: unit,
            quota_state: state_from_header
                .map(ToString::to_string)
                .unwrap_or_else(|| state_from_value(value).to_string()),
            reset_at,
            source: source.to_string(),
            confidence: 96,
            reason: Some(format!("x_codex_headers@{endpoint}")),
        });
    }

    if let Some(state) = state_from_header {
        return Some(QuotaProbeResult {
            mode: "state".to_string(),
            remaining_value: None,
            remaining_unit: unit,
            quota_state: state.to_string(),
            reset_at,
            source: source.to_string(),
            confidence: 80,
            reason: Some(format!("x_codex_headers_state@{endpoint}")),
        });
    }

    None
}

fn extract_header_text_by_keys(headers: &header::HeaderMap, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        headers
            .get(*key)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
    })
}

fn extract_header_number_by_keys(headers: &header::HeaderMap, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| {
        let raw = headers
            .get(*key)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)?;
        parse_first_f64(raw)
    })
}

fn parse_first_f64(text: &str) -> Option<f64> {
    let regex = Regex::new(r"(-?[0-9]+(?:\.[0-9]+)?)").ok()?;
    let capture = regex.captures(text)?;
    capture.get(1)?.as_str().parse::<f64>().ok()
}

fn normalize_quota_state(raw: &str) -> Option<&'static str> {
    let lower = raw.to_lowercase();
    if lower.contains("exhaust")
        || lower.contains("limit")
        || lower.contains("deny")
        || lower.contains("blocked")
    {
        Some("exhausted")
    } else if lower.contains("near")
        || lower.contains("warn")
        || lower.contains("low")
        || lower.contains("throttle")
    {
        Some("near_limit")
    } else if lower.contains("ok")
        || lower.contains("allow")
        || lower.contains("available")
        || lower.contains("active")
    {
        Some("available")
    } else {
        None
    }
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
        confidence: 88,
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
    let regex = Regex::new("(?i)(remaining|quota)\\D{0,20}([0-9]+(?:\\.[0-9]+)?)").ok()?;
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

    let lower = html.to_lowercase();
    if lower.contains("limit reached")
        || lower.contains("quota exceeded")
        || lower.contains("you've reached your usage limit")
    {
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
        .get("access_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| anyhow!("该账号缺少 access_token 字段，无法查询配额"))
}

#[cfg(test)]
mod tests {
    use super::extract_from_codex_headers;
    use reqwest::header::{HeaderMap, HeaderValue};

    #[test]
    fn parses_remaining_from_codex_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("x-codex-remaining", HeaderValue::from_static("12.5"));
        headers.insert("x-codex-unit", HeaderValue::from_static("requests"));

        let result = extract_from_codex_headers(
            &headers,
            "api",
            "https://chatgpt.com/backend-api/api/codex/usage",
        )
        .expect("should parse headers");

        assert_eq!(result.mode, "exact");
        assert_eq!(result.remaining_value, Some(12.5));
        assert_eq!(result.remaining_unit.as_deref(), Some("requests"));
        assert_eq!(result.quota_state, "available");
    }

    #[test]
    fn computes_remaining_from_limit_minus_used_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("x-codex-limit", HeaderValue::from_static("100"));
        headers.insert("x-codex-used", HeaderValue::from_static("97"));

        let result = extract_from_codex_headers(
            &headers,
            "api",
            "https://chat.openai.com/backend-api/wham/usage",
        )
        .expect("should parse headers");

        assert_eq!(result.mode, "exact");
        assert_eq!(result.remaining_value, Some(3.0));
        assert_eq!(result.quota_state, "near_limit");
    }

    #[test]
    fn parses_state_only_from_codex_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("x-codex-state", HeaderValue::from_static("exhausted"));

        let result = extract_from_codex_headers(
            &headers,
            "api",
            "https://chatgpt.com/backend-api/api/codex/usage",
        )
        .expect("should parse state headers");

        assert_eq!(result.mode, "state");
        assert_eq!(result.remaining_value, None);
        assert_eq!(result.quota_state, "exhausted");
    }
}
