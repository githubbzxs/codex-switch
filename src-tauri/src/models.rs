use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub name: String,
    pub tags: Vec<String>,
    pub auth_fingerprint: String,
    pub created_at: String,
    pub updated_at: String,
    pub last_used_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwitchHistory {
    pub id: String,
    pub from_account_id: Option<String>,
    pub to_account_id: String,
    pub snapshot_path: Option<String>,
    pub result: String,
    pub error_message: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwitchResult {
    pub success: bool,
    pub history_id: String,
    pub snapshot_path: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeDiagnostics {
    pub codex_auth_path: String,
    pub codex_auth_exists: bool,
    pub app_data_dir: String,
    pub db_path: String,
    pub schema_ok: bool,
    pub process_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaSnapshot {
    pub id: String,
    pub account_id: String,
    pub mode: String,
    pub remaining_value: Option<f64>,
    pub remaining_unit: Option<String>,
    pub quota_state: String,
    pub reset_at: Option<String>,
    pub source: String,
    pub confidence: i64,
    pub reason: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaDashboardItem {
    pub account: Account,
    pub snapshot: Option<QuotaSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimpleStatus {
    pub ok: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaRefreshPolicy {
    pub timeout_ms: u64,
    pub cache_ttl_seconds: u64,
    pub max_concurrency: usize,
}

impl Default for QuotaRefreshPolicy {
    fn default() -> Self {
        Self {
            timeout_ms: 5000,
            cache_ttl_seconds: 180,
            max_concurrency: 3,
        }
    }
}
