use crate::models::{Account, QuotaSnapshot, SwitchHistory};
use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::{Path, PathBuf};
use uuid::Uuid;

const SETTINGS_SINGLETON_ID: i64 = 1;

#[derive(Debug, Clone)]
pub struct AppStore {
    pub base_dir: PathBuf,
    pub db_path: PathBuf,
    pub snapshots_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct AccountSecret {
    pub account: Account,
    pub encrypted_auth_blob: String,
}

#[derive(Debug, Clone)]
pub struct VaultSettings {
    pub salt: Option<String>,
}

impl AppStore {
    pub fn new(base_dir: PathBuf) -> Self {
        let db_path = base_dir.join("codex-switch.db");
        let snapshots_dir = base_dir.join("snapshots");
        Self {
            base_dir,
            db_path,
            snapshots_dir,
        }
    }

    pub fn init(&self) -> Result<()> {
        std::fs::create_dir_all(&self.base_dir)
            .with_context(|| format!("创建数据目录失败: {}", self.base_dir.display()))?;
        std::fs::create_dir_all(&self.snapshots_dir)
            .with_context(|| format!("创建快照目录失败: {}", self.snapshots_dir.display()))?;
        let conn = self.open_conn()?;
        conn.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            CREATE TABLE IF NOT EXISTS app_settings (
              id INTEGER PRIMARY KEY CHECK (id = 1),
              vault_salt TEXT,
              default_account_id TEXT,
              cli_restart_mode TEXT NOT NULL DEFAULT 'force',
              quota_timeout_ms INTEGER NOT NULL DEFAULT 5000,
              quota_cache_ttl_seconds INTEGER NOT NULL DEFAULT 180,
              quota_max_concurrency INTEGER NOT NULL DEFAULT 3,
              updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS accounts (
              id TEXT PRIMARY KEY,
              name TEXT NOT NULL,
              tags_json TEXT NOT NULL,
              encrypted_auth_blob TEXT NOT NULL,
              auth_fingerprint TEXT NOT NULL,
              created_at TEXT NOT NULL,
              updated_at TEXT NOT NULL,
              last_used_at TEXT
            );

            CREATE TABLE IF NOT EXISTS switch_history (
              id TEXT PRIMARY KEY,
              from_account_id TEXT,
              to_account_id TEXT NOT NULL,
              snapshot_path TEXT,
              result TEXT NOT NULL,
              error_message TEXT,
              created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS quota_snapshots (
              id TEXT PRIMARY KEY,
              account_id TEXT NOT NULL,
              mode TEXT NOT NULL,
              remaining_value REAL,
              remaining_unit TEXT,
              quota_state TEXT NOT NULL,
              reset_at TEXT,
              source TEXT NOT NULL,
              confidence INTEGER NOT NULL,
              reason TEXT,
              created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_quota_snapshots_account_created_at
              ON quota_snapshots(account_id, created_at DESC);
        "#,
        )
        .context("初始化数据库失败")?;
        conn.execute(
            r#"
            INSERT INTO app_settings(id, updated_at)
            VALUES (?1, ?2)
            ON CONFLICT(id) DO NOTHING
        "#,
            params![SETTINGS_SINGLETON_ID, now()],
        )
        .context("初始化设置失败")?;
        Ok(())
    }

    pub fn open_conn(&self) -> Result<Connection> {
        Connection::open(&self.db_path)
            .with_context(|| format!("打开数据库失败: {}", self.db_path.display()))
    }

    pub fn get_vault_settings(&self) -> Result<VaultSettings> {
        let conn = self.open_conn()?;
        let salt: Option<String> = conn
            .query_row(
                "SELECT vault_salt FROM app_settings WHERE id = ?1",
                params![SETTINGS_SINGLETON_ID],
                |row| row.get(0),
            )
            .optional()
            .context("读取主密码设置失败")?
            .flatten();
        Ok(VaultSettings { salt })
    }

    pub fn set_vault_salt(&self, salt: &str) -> Result<()> {
        let conn = self.open_conn()?;
        conn.execute(
            r#"
            UPDATE app_settings
            SET vault_salt = ?1, updated_at = ?2
            WHERE id = ?3
        "#,
            params![salt, now(), SETTINGS_SINGLETON_ID],
        )
        .context("写入主密码盐值失败")?;
        Ok(())
    }

    pub fn get_quota_policy(&self) -> Result<(u64, u64, usize)> {
        let conn = self.open_conn()?;
        let tuple = conn
            .query_row(
                r#"
            SELECT quota_timeout_ms, quota_cache_ttl_seconds, quota_max_concurrency
            FROM app_settings WHERE id = ?1
            "#,
                params![SETTINGS_SINGLETON_ID],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)? as u64,
                        row.get::<_, i64>(1)? as u64,
                        row.get::<_, i64>(2)? as usize,
                    ))
                },
            )
            .context("读取配额策略失败")?;
        Ok(tuple)
    }

    pub fn set_quota_policy(
        &self,
        timeout_ms: u64,
        cache_ttl_seconds: u64,
        max_concurrency: usize,
    ) -> Result<()> {
        let conn = self.open_conn()?;
        conn.execute(
            r#"
            UPDATE app_settings
            SET quota_timeout_ms = ?1, quota_cache_ttl_seconds = ?2, quota_max_concurrency = ?3, updated_at = ?4
            WHERE id = ?5
            "#,
            params![
                timeout_ms as i64,
                cache_ttl_seconds as i64,
                max_concurrency as i64,
                now(),
                SETTINGS_SINGLETON_ID
            ],
        )
        .context("更新配额策略失败")?;
        Ok(())
    }

    pub fn create_account(
        &self,
        name: &str,
        tags: &[String],
        encrypted_auth_blob: &str,
        fingerprint: &str,
    ) -> Result<Account> {
        let conn = self.open_conn()?;
        let id = Uuid::new_v4().to_string();
        let timestamp = now();
        conn.execute(
            r#"
            INSERT INTO accounts(
                id, name, tags_json, encrypted_auth_blob, auth_fingerprint, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
        "#,
            params![
                id,
                name.trim(),
                serde_json::to_string(tags).context("序列化账户标签失败")?,
                encrypted_auth_blob,
                fingerprint,
                timestamp,
                timestamp
            ],
        )
        .context("写入账户失败")?;
        self.get_account(&id)?
            .ok_or_else(|| anyhow!("账户写入后未找到"))
    }

    pub fn get_account(&self, id: &str) -> Result<Option<Account>> {
        let conn = self.open_conn()?;
        conn.query_row(
            r#"
            SELECT id, name, tags_json, auth_fingerprint, created_at, updated_at, last_used_at
            FROM accounts
            WHERE id = ?1
            "#,
            params![id],
            |row| {
                Ok(Account {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    tags: serde_json::from_str::<Vec<String>>(&row.get::<_, String>(2)?)
                        .unwrap_or_default(),
                    auth_fingerprint: row.get(3)?,
                    created_at: row.get(4)?,
                    updated_at: row.get(5)?,
                    last_used_at: row.get(6)?,
                })
            },
        )
        .optional()
        .context("读取账户失败")
    }

    pub fn get_account_secret(&self, id: &str) -> Result<Option<AccountSecret>> {
        let conn = self.open_conn()?;
        conn.query_row(
            r#"
            SELECT id, name, tags_json, encrypted_auth_blob, auth_fingerprint, created_at, updated_at, last_used_at
            FROM accounts
            WHERE id = ?1
            "#,
            params![id],
            |row| {
                let tags_json: String = row.get(2)?;
                Ok(AccountSecret {
                    account: Account {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        tags: serde_json::from_str::<Vec<String>>(&tags_json).unwrap_or_default(),
                        auth_fingerprint: row.get(4)?,
                        created_at: row.get(5)?,
                        updated_at: row.get(6)?,
                        last_used_at: row.get(7)?,
                    },
                    encrypted_auth_blob: row.get(3)?,
                })
            },
        )
        .optional()
        .context("读取账户密文失败")
    }

    pub fn list_accounts(&self) -> Result<Vec<Account>> {
        let conn = self.open_conn()?;
        let mut stmt = conn.prepare(
            r#"
            SELECT id, name, tags_json, auth_fingerprint, created_at, updated_at, last_used_at
            FROM accounts
            ORDER BY updated_at DESC
        "#,
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Account {
                id: row.get(0)?,
                name: row.get(1)?,
                tags: serde_json::from_str::<Vec<String>>(&row.get::<_, String>(2)?)
                    .unwrap_or_default(),
                auth_fingerprint: row.get(3)?,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
                last_used_at: row.get(6)?,
            })
        })?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    pub fn update_account_meta(&self, id: &str, name: &str, tags: &[String]) -> Result<()> {
        let conn = self.open_conn()?;
        conn.execute(
            r#"
            UPDATE accounts
            SET name = ?1, tags_json = ?2, updated_at = ?3
            WHERE id = ?4
        "#,
            params![name.trim(), serde_json::to_string(tags)?, now(), id],
        )
        .context("更新账户失败")?;
        Ok(())
    }

    pub fn delete_account(&self, id: &str) -> Result<()> {
        let conn = self.open_conn()?;
        conn.execute("DELETE FROM accounts WHERE id = ?1", params![id])
            .context("删除账户失败")?;
        Ok(())
    }

    pub fn mark_account_used(&self, id: &str) -> Result<()> {
        let conn = self.open_conn()?;
        conn.execute(
            "UPDATE accounts SET last_used_at = ?1, updated_at = ?2 WHERE id = ?3",
            params![now(), now(), id],
        )
        .context("更新账户最近使用时间失败")?;
        Ok(())
    }

    pub fn create_switch_history(
        &self,
        from_account_id: Option<&str>,
        to_account_id: &str,
        snapshot_path: Option<&Path>,
        result: &str,
        error_message: Option<&str>,
    ) -> Result<String> {
        let conn = self.open_conn()?;
        let id = Uuid::new_v4().to_string();
        conn.execute(
            r#"
            INSERT INTO switch_history(
              id, from_account_id, to_account_id, snapshot_path, result, error_message, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
            params![
                id,
                from_account_id,
                to_account_id,
                snapshot_path.map(|p| p.display().to_string()),
                result,
                error_message,
                now()
            ],
        )
        .context("写入切换历史失败")?;
        Ok(id)
    }

    pub fn list_switch_history(&self, limit: usize) -> Result<Vec<SwitchHistory>> {
        let conn = self.open_conn()?;
        let mut stmt = conn.prepare(
            r#"
            SELECT id, from_account_id, to_account_id, snapshot_path, result, error_message, created_at
            FROM switch_history
            ORDER BY created_at DESC
            LIMIT ?1
            "#,
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok(SwitchHistory {
                id: row.get(0)?,
                from_account_id: row.get(1)?,
                to_account_id: row.get(2)?,
                snapshot_path: row.get(3)?,
                result: row.get(4)?,
                error_message: row.get(5)?,
                created_at: row.get(6)?,
            })
        })?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    pub fn get_switch_history(&self, history_id: &str) -> Result<Option<SwitchHistory>> {
        let conn = self.open_conn()?;
        conn.query_row(
            r#"
            SELECT id, from_account_id, to_account_id, snapshot_path, result, error_message, created_at
            FROM switch_history WHERE id = ?1
            "#,
            params![history_id],
            |row| {
                Ok(SwitchHistory {
                    id: row.get(0)?,
                    from_account_id: row.get(1)?,
                    to_account_id: row.get(2)?,
                    snapshot_path: row.get(3)?,
                    result: row.get(4)?,
                    error_message: row.get(5)?,
                    created_at: row.get(6)?,
                })
            },
        )
        .optional()
        .context("读取切换历史失败")
    }

    pub fn get_current_account_id(&self) -> Result<Option<String>> {
        let history = self.list_switch_history(1)?;
        Ok(history.first().map(|item| item.to_account_id.clone()))
    }

    pub fn save_quota_snapshot(
        &self,
        account_id: &str,
        mode: &str,
        remaining_value: Option<f64>,
        remaining_unit: Option<&str>,
        quota_state: &str,
        reset_at: Option<&str>,
        source: &str,
        confidence: i64,
        reason: Option<&str>,
    ) -> Result<QuotaSnapshot> {
        let conn = self.open_conn()?;
        let id = Uuid::new_v4().to_string();
        let created_at = now();
        conn.execute(
            r#"
            INSERT INTO quota_snapshots(
              id, account_id, mode, remaining_value, remaining_unit, quota_state, reset_at, source, confidence, reason, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
            "#,
            params![
                id,
                account_id,
                mode,
                remaining_value,
                remaining_unit,
                quota_state,
                reset_at,
                source,
                confidence,
                reason,
                created_at
            ],
        )
        .context("写入配额快照失败")?;
        self.get_quota_snapshot(&id)?
            .ok_or_else(|| anyhow!("写入配额快照后未找到"))
    }

    pub fn get_quota_snapshot(&self, id: &str) -> Result<Option<QuotaSnapshot>> {
        let conn = self.open_conn()?;
        conn.query_row(
            r#"
            SELECT id, account_id, mode, remaining_value, remaining_unit, quota_state, reset_at, source, confidence, reason, created_at
            FROM quota_snapshots WHERE id = ?1
            "#,
            params![id],
            map_quota_snapshot,
        )
        .optional()
        .context("读取配额快照失败")
    }

    pub fn list_quota_snapshots(
        &self,
        account_id: &str,
        limit: usize,
    ) -> Result<Vec<QuotaSnapshot>> {
        let conn = self.open_conn()?;
        let mut stmt = conn.prepare(
            r#"
            SELECT id, account_id, mode, remaining_value, remaining_unit, quota_state, reset_at, source, confidence, reason, created_at
            FROM quota_snapshots
            WHERE account_id = ?1
            ORDER BY created_at DESC
            LIMIT ?2
            "#,
        )?;
        let rows = stmt.query_map(params![account_id, limit as i64], map_quota_snapshot)?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    pub fn list_latest_quota_snapshots(&self) -> Result<Vec<QuotaSnapshot>> {
        let conn = self.open_conn()?;
        let mut stmt = conn.prepare(
            r#"
            SELECT q.id, q.account_id, q.mode, q.remaining_value, q.remaining_unit, q.quota_state, q.reset_at, q.source, q.confidence, q.reason, q.created_at
            FROM quota_snapshots q
            JOIN (
              SELECT account_id, MAX(created_at) AS max_created_at
              FROM quota_snapshots
              GROUP BY account_id
            ) l
            ON q.account_id = l.account_id AND q.created_at = l.max_created_at
            "#,
        )?;
        let rows = stmt.query_map([], map_quota_snapshot)?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    pub fn latest_quota_by_account(&self, account_id: &str) -> Result<Option<QuotaSnapshot>> {
        let conn = self.open_conn()?;
        conn.query_row(
            r#"
            SELECT id, account_id, mode, remaining_value, remaining_unit, quota_state, reset_at, source, confidence, reason, created_at
            FROM quota_snapshots
            WHERE account_id = ?1
            ORDER BY created_at DESC
            LIMIT 1
            "#,
            params![account_id],
            map_quota_snapshot,
        )
        .optional()
        .context("读取最新配额快照失败")
    }
}

fn map_quota_snapshot(row: &rusqlite::Row<'_>) -> rusqlite::Result<QuotaSnapshot> {
    Ok(QuotaSnapshot {
        id: row.get(0)?,
        account_id: row.get(1)?,
        mode: row.get(2)?,
        remaining_value: row.get(3)?,
        remaining_unit: row.get(4)?,
        quota_state: row.get(5)?,
        reset_at: row.get(6)?,
        source: row.get(7)?,
        confidence: row.get(8)?,
        reason: row.get(9)?,
        created_at: row.get(10)?,
    })
}

pub fn now() -> String {
    Utc::now().to_rfc3339()
}
