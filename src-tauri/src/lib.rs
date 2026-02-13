mod app_state;
mod codex;
mod crypto;
mod models;
mod quota;
mod store;

use anyhow::Context;
use app_state::AppState;
use codex::{
    atomic_write, codex_auth_path, compute_fingerprint, count_codex_processes, create_snapshot,
    kill_codex_processes, read_and_validate_auth_json, restart_codex, run_codex_login,
    validate_auth_json,
};
use models::{
    Account, CodexCliStatus, QuotaDashboardItem, QuotaRefreshPolicy, QuotaSnapshot,
    RuntimeDiagnostics, SimpleStatus, SwitchHistory, SwitchResult,
};
use quota::{ensure_access_token, probe_quota};
use serde_json::Value;
use std::{collections::HashMap, fs, path::PathBuf, time::Duration};
use tauri::State;
use zeroize::Zeroize;

type CmdResult<T> = Result<T, String>;
const LOGIN_AUTH_POLL_MAX_ATTEMPTS: usize = 8;
const LOGIN_AUTH_POLL_INTERVAL_MS: u64 = 500;

fn map_error<T>(result: anyhow::Result<T>) -> CmdResult<T> {
    result.map_err(|error| error.to_string())
}

fn ensure_name(name: &str, auth_json: &Value) -> String {
    if !name.trim().is_empty() {
        return name.trim().to_string();
    }
    if let Some(email) = auth_json
        .get("email")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return email.to_string();
    }

    if let Some(account_id) = auth_json
        .get("account_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return account_id.to_string();
    }

    let suffix = compute_fingerprint(auth_json)
        .ok()
        .and_then(|fingerprint| fingerprint.split(':').nth(1).map(|hash| hash.to_string()))
        .map(|hash| hash.chars().take(4).collect::<String>())
        .filter(|hash| hash.len() == 4)
        .unwrap_or_else(|| "0000".to_string());
    format!("未命名账号-{suffix}")
}

fn unique_tags(tags: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    tags.into_iter()
        .map(|tag| tag.trim().to_string())
        .filter(|tag| !tag.is_empty())
        .filter(|tag| seen.insert(tag.clone()))
        .collect()
}

fn import_account_from_auth_json(
    state: &AppState,
    name: &str,
    tags: Vec<String>,
    previous_fingerprint: Option<&str>,
    auth_json: Value,
) -> anyhow::Result<Account> {
    let mut key = state.get_vault_key()?;
    let auth_text = serde_json::to_string_pretty(&auth_json)?;
    let fingerprint = compute_fingerprint(&auth_json)?;

    if let Some(previous) = previous_fingerprint {
        if previous == fingerprint {
            key.zeroize();
            return Err(anyhow::anyhow!(
                "登录完成，但检测到仍是当前账号。请在浏览器切换到新账号后重试。"
            ));
        }
    }

    if state
        .store
        .find_account_by_fingerprint(&fingerprint)?
        .is_some()
    {
        key.zeroize();
        return Err(anyhow::anyhow!("该账号已存在，已跳过重复导入。"));
    }

    let encrypted = crypto::encrypt_to_base64(&key, auth_text.as_bytes())?;
    key.zeroize();
    state.store.create_account(
        &ensure_name(name, &auth_json),
        &unique_tags(tags),
        &encrypted,
        &fingerprint,
    )
}

fn import_account_from_current_auth(
    state: &AppState,
    name: &str,
    tags: Vec<String>,
    previous_fingerprint: Option<&str>,
) -> anyhow::Result<Account> {
    let auth_path = codex_auth_path()?;
    let auth_json = read_and_validate_auth_json(&auth_path)?;
    import_account_from_auth_json(state, name, tags, previous_fingerprint, auth_json)
}

async fn wait_for_login_auth_json(previous_auth_text: Option<&str>) -> anyhow::Result<Value> {
    let auth_path = codex_auth_path()?;
    let previous_auth_text = previous_auth_text.map(str::to_owned);

    for _ in 0..LOGIN_AUTH_POLL_MAX_ATTEMPTS {
        if let Ok(current_text) = fs::read_to_string(&auth_path) {
            let updated = match previous_auth_text.as_ref() {
                Some(previous) => previous != &current_text,
                None => true,
            };
            if updated {
                if let Ok(json) = validate_auth_json(&current_text) {
                    return Ok(json);
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(LOGIN_AUTH_POLL_INTERVAL_MS)).await;
    }

    Err(anyhow::anyhow!(
        "登录已结束，但 ~/.codex/auth.json 未在预期时间内更新。请确认浏览器授权已完成后重试。"
    ))
}

#[tauri::command]
fn init_vault(state: State<'_, AppState>, master_password: String) -> CmdResult<SimpleStatus> {
    if master_password.trim().len() < 8 {
        return Err("主密码至少需要 8 位".to_string());
    }
    map_error((|| {
        let initialized = state.init_vault(master_password.trim())?;
        if initialized {
            Ok(SimpleStatus {
                ok: true,
                message: "保险库已初始化并解锁".to_string(),
            })
        } else {
            Ok(SimpleStatus {
                ok: false,
                message: "保险库已存在，请直接解锁".to_string(),
            })
        }
    })())
}

#[tauri::command]
fn unlock_vault(state: State<'_, AppState>, master_password: String) -> CmdResult<SimpleStatus> {
    map_error((|| {
        state.unlock_vault(master_password.trim())?;
        Ok(SimpleStatus {
            ok: true,
            message: "保险库已解锁".to_string(),
        })
    })())
}

#[tauri::command]
fn lock_vault(state: State<'_, AppState>) -> CmdResult<SimpleStatus> {
    map_error((|| {
        state.lock_vault()?;
        Ok(SimpleStatus {
            ok: true,
            message: "保险库已锁定".to_string(),
        })
    })())
}

#[tauri::command]
fn vault_status(state: State<'_, AppState>) -> CmdResult<SimpleStatus> {
    map_error((|| {
        let unlocked = state.is_vault_unlocked()?;
        Ok(SimpleStatus {
            ok: unlocked,
            message: if unlocked {
                "已解锁".to_string()
            } else {
                "未解锁".to_string()
            },
        })
    })())
}

#[tauri::command]
fn import_current_codex_auth(
    state: State<'_, AppState>,
    name: String,
    tags: Vec<String>,
) -> CmdResult<Account> {
    map_error(import_account_from_current_auth(&state, &name, tags, None))
}

#[tauri::command]
fn create_account_from_import(
    state: State<'_, AppState>,
    name: String,
    tags: Vec<String>,
) -> CmdResult<Account> {
    import_current_codex_auth(state, name, tags)
}

#[tauri::command]
fn create_account_from_auth_file(
    state: State<'_, AppState>,
    path: String,
    name: String,
    tags: Vec<String>,
) -> CmdResult<Account> {
    map_error((|| {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            return Err(anyhow::anyhow!("认证文件路径不能为空"));
        }

        let file_path = PathBuf::from(trimmed);
        let auth_text = fs::read_to_string(&file_path)
            .with_context(|| format!("读取认证文件失败: {}", file_path.display()))?;
        let auth_json =
            validate_auth_json(&auth_text).with_context(|| "认证文件格式校验失败".to_string())?;
        import_account_from_auth_json(&state, &name, tags, None, auth_json)
    })())
}

#[tauri::command]
async fn create_account_from_login(
    state: State<'_, AppState>,
    name: String,
    tags: Vec<String>,
) -> CmdResult<Account> {
    map_error(
        async move {
            if !state.is_vault_unlocked()? {
                return Err(anyhow::anyhow!("请先解锁保险库，再进行登录添加"));
            }

            let auth_path = codex_auth_path()?;
            let previous_auth_text = fs::read_to_string(&auth_path).ok();
            let previous_fingerprint = previous_auth_text
                .as_deref()
                .and_then(|text| validate_auth_json(text).ok())
                .and_then(|json| compute_fingerprint(&json).ok());

            tauri::async_runtime::spawn_blocking(|| run_codex_login(900))
                .await
                .map_err(|error| anyhow::anyhow!("等待登录任务失败: {error}"))??;

            let latest_auth_json = wait_for_login_auth_json(previous_auth_text.as_deref()).await?;
            import_account_from_auth_json(
                &state,
                &name,
                tags,
                previous_fingerprint.as_deref(),
                latest_auth_json,
            )
        }
        .await,
    )
}

#[tauri::command]
fn list_accounts(state: State<'_, AppState>) -> CmdResult<Vec<Account>> {
    map_error(state.store.list_accounts())
}

#[tauri::command]
fn update_account_meta(
    state: State<'_, AppState>,
    id: String,
    name: String,
    tags: Vec<String>,
) -> CmdResult<SimpleStatus> {
    map_error((|| {
        state
            .store
            .update_account_meta(id.trim(), name.trim(), &unique_tags(tags))?;
        Ok(SimpleStatus {
            ok: true,
            message: "账户信息已更新".to_string(),
        })
    })())
}

#[tauri::command]
fn delete_account(state: State<'_, AppState>, id: String) -> CmdResult<SimpleStatus> {
    map_error((|| {
        state.store.delete_account(id.trim())?;
        Ok(SimpleStatus {
            ok: true,
            message: "账户已删除".to_string(),
        })
    })())
}

#[tauri::command]
fn switch_account(
    state: State<'_, AppState>,
    id: String,
    force_restart: bool,
) -> CmdResult<SwitchResult> {
    map_error((|| {
        let account_secret = state
            .store
            .get_account_secret(id.trim())?
            .ok_or_else(|| anyhow::anyhow!("目标账户不存在"))?;

        let from_account = state.store.get_current_account_id()?;
        let mut key = state.get_vault_key()?;
        let decrypted = crypto::decrypt_from_base64(&key, &account_secret.encrypted_auth_blob)?;
        key.zeroize();
        let auth_text = String::from_utf8(decrypted)?;
        validate_auth_json(&auth_text)?;

        let auth_path = codex_auth_path()?;
        let snapshot_path = create_snapshot(&auth_path, &state.store.snapshots_dir)?;

        let write_result = atomic_write(&auth_path, &auth_text);
        if let Err(error) = write_result {
            let history_id = state.store.create_switch_history(
                from_account.as_deref(),
                account_secret.account.id.as_str(),
                snapshot_path.as_deref(),
                "failed",
                Some(&error.to_string()),
            )?;
            return Ok(SwitchResult {
                success: false,
                history_id,
                snapshot_path: snapshot_path.map(|path| path.display().to_string()),
                message: format!("切换失败：{error}"),
            });
        }

        let mut killed_count = 0usize;
        if force_restart {
            killed_count = kill_codex_processes();
            let _ = restart_codex();
        }
        state
            .store
            .mark_account_used(account_secret.account.id.as_str())?;
        let history_id = state.store.create_switch_history(
            from_account.as_deref(),
            account_secret.account.id.as_str(),
            snapshot_path.as_deref(),
            "success",
            None,
        )?;
        Ok(SwitchResult {
            success: true,
            history_id,
            snapshot_path: snapshot_path.map(|path| path.display().to_string()),
            message: if force_restart {
                format!("切换完成，已处理 {killed_count} 个 Codex 进程")
            } else {
                "切换完成".to_string()
            },
        })
    })())
}

#[tauri::command]
fn rollback_to_history(state: State<'_, AppState>, history_id: String) -> CmdResult<SwitchResult> {
    map_error((|| {
        let history = state
            .store
            .get_switch_history(history_id.trim())?
            .ok_or_else(|| anyhow::anyhow!("历史记录不存在"))?;
        let snapshot_path = history
            .snapshot_path
            .as_ref()
            .map(PathBuf::from)
            .ok_or_else(|| anyhow::anyhow!("该历史记录没有可回滚快照"))?;
        if !snapshot_path.exists() {
            return Err(anyhow::anyhow!(
                "快照文件不存在: {}",
                snapshot_path.display()
            ));
        }
        let snapshot_content = std::fs::read_to_string(&snapshot_path)?;
        validate_auth_json(&snapshot_content)?;
        let auth_path = codex_auth_path()?;
        let current_snapshot = create_snapshot(&auth_path, &state.store.snapshots_dir)?;
        atomic_write(&auth_path, &snapshot_content)?;
        let killed_count = kill_codex_processes();
        let _ = restart_codex();
        let created_history_id = state.store.create_switch_history(
            history.from_account_id.as_deref(),
            history.to_account_id.as_str(),
            current_snapshot.as_deref(),
            "rolled_back",
            None,
        )?;

        Ok(SwitchResult {
            success: true,
            history_id: created_history_id,
            snapshot_path: Some(snapshot_path.display().to_string()),
            message: format!("回滚完成，已处理 {killed_count} 个 Codex 进程"),
        })
    })())
}

#[tauri::command]
fn list_switch_history(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> CmdResult<Vec<SwitchHistory>> {
    map_error(state.store.list_switch_history(limit.unwrap_or(100)))
}

#[tauri::command]
async fn refresh_quota(
    state: State<'_, AppState>,
    account_id: Option<String>,
    force: Option<bool>,
) -> CmdResult<Vec<QuotaSnapshot>> {
    map_error(
        async move {
            let mut key = state.get_vault_key()?;
            let accounts = if let Some(account_id) = account_id {
                let account = state
                    .store
                    .get_account_secret(account_id.trim())?
                    .ok_or_else(|| anyhow::anyhow!("账户不存在"))?;
                vec![account]
            } else {
                let account_list = state.store.list_accounts()?;
                let mut result = Vec::new();
                for account in account_list {
                    if let Some(secret) = state.store.get_account_secret(&account.id)? {
                        result.push(secret);
                    }
                }
                result
            };

            let (timeout_ms, ttl_seconds, _max_concurrency) = state.store.get_quota_policy()?;
            let force_refresh = force.unwrap_or(false);
            let mut snapshots = Vec::new();

            for account in accounts {
                if !force_refresh {
                    if let Some(existing) =
                        state.store.latest_quota_by_account(&account.account.id)?
                    {
                        let age = chrono::DateTime::parse_from_rfc3339(&existing.created_at)
                            .map(|time| {
                                chrono::Utc::now()
                                    .signed_duration_since(time.with_timezone(&chrono::Utc))
                                    .num_seconds()
                            })
                            .unwrap_or(i64::MAX);
                        if age >= 0 && age as u64 <= ttl_seconds {
                            snapshots.push(existing);
                            continue;
                        }
                    }
                }

                let decrypted = crypto::decrypt_from_base64(&key, &account.encrypted_auth_blob)?;
                let auth_text = String::from_utf8(decrypted)?;
                let auth_json: Value = serde_json::from_str(&auth_text)?;
                let access_token = ensure_access_token(&auth_json)?;
                let chatgpt_account_id = auth_json
                    .get("account_id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty());
                let probe = probe_quota(&access_token, chatgpt_account_id, timeout_ms).await;
                let saved = state.store.save_quota_snapshot(
                    &account.account.id,
                    &probe.mode,
                    probe.remaining_value,
                    probe.remaining_unit.as_deref(),
                    &probe.quota_state,
                    probe.reset_at.as_deref(),
                    &probe.source,
                    probe.confidence,
                    probe.reason.as_deref(),
                )?;
                snapshots.push(saved);
            }
            key.zeroize();
            Ok(snapshots)
        }
        .await,
    )
}

#[tauri::command]
fn get_quota_dashboard(state: State<'_, AppState>) -> CmdResult<Vec<QuotaDashboardItem>> {
    map_error((|| {
        let accounts = state.store.list_accounts()?;
        let snapshots = state.store.list_latest_quota_snapshots()?;
        let snapshot_map: HashMap<String, QuotaSnapshot> = snapshots
            .into_iter()
            .map(|snapshot| (snapshot.account_id.clone(), snapshot))
            .collect();

        let mut dashboard = accounts
            .into_iter()
            .map(|account| {
                let snapshot = snapshot_map.get(&account.id).cloned();
                QuotaDashboardItem { account, snapshot }
            })
            .collect::<Vec<_>>();

        dashboard.sort_by_key(|item| {
            state_rank(
                item.snapshot
                    .as_ref()
                    .map(|snapshot| snapshot.quota_state.as_str()),
            )
        });
        Ok(dashboard)
    })())
}

#[tauri::command]
fn list_quota_snapshots(
    state: State<'_, AppState>,
    account_id: String,
    limit: Option<usize>,
) -> CmdResult<Vec<QuotaSnapshot>> {
    map_error(
        state
            .store
            .list_quota_snapshots(account_id.trim(), limit.unwrap_or(50)),
    )
}

#[tauri::command]
fn set_quota_refresh_policy(
    state: State<'_, AppState>,
    policy: QuotaRefreshPolicy,
) -> CmdResult<SimpleStatus> {
    map_error((|| {
        let timeout_ms = policy.timeout_ms.clamp(1000, 30_000);
        let ttl_seconds = policy.cache_ttl_seconds.clamp(30, 3600);
        let max_concurrency = policy.max_concurrency.clamp(1, 8);
        state
            .store
            .set_quota_policy(timeout_ms, ttl_seconds, max_concurrency)?;
        Ok(SimpleStatus {
            ok: true,
            message: "配额刷新策略已更新".to_string(),
        })
    })())
}

#[tauri::command]
fn get_runtime_diagnostics(state: State<'_, AppState>) -> CmdResult<RuntimeDiagnostics> {
    map_error((|| {
        let auth_path = codex_auth_path()?;
        let codex_auth_exists = auth_path.exists();
        let schema_ok = if codex_auth_exists {
            read_and_validate_auth_json(&auth_path).is_ok()
        } else {
            false
        };

        let process_count = count_codex_processes();

        Ok(RuntimeDiagnostics {
            codex_auth_path: auth_path.display().to_string(),
            codex_auth_exists,
            app_data_dir: state.store.base_dir.display().to_string(),
            db_path: state.store.db_path.display().to_string(),
            schema_ok,
            process_count,
        })
    })())
}

#[tauri::command]
fn get_codex_cli_status() -> CmdResult<CodexCliStatus> {
    map_error((|| {
        let process_count = count_codex_processes();
        Ok(CodexCliStatus {
            is_running: process_count > 0,
            process_count,
            checked_at: chrono::Utc::now().to_rfc3339(),
        })
    })())
}

fn state_rank(state: Option<&str>) -> u8 {
    match state.unwrap_or("unknown") {
        "available" => 0,
        "near_limit" => 1,
        "exhausted" => 2,
        _ => 3,
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let base_dir = dirs::data_local_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .join("codex-switch");
    let store = store::AppStore::new(base_dir);
    let state = AppState::initialize(store).expect("初始化应用状态失败");

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            init_vault,
            unlock_vault,
            lock_vault,
            vault_status,
            import_current_codex_auth,
            create_account_from_import,
            create_account_from_auth_file,
            create_account_from_login,
            list_accounts,
            update_account_meta,
            delete_account,
            switch_account,
            rollback_to_history,
            list_switch_history,
            refresh_quota,
            get_quota_dashboard,
            list_quota_snapshots,
            set_quota_refresh_policy,
            get_runtime_diagnostics,
            get_codex_cli_status,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
