export type QuotaState = "available" | "near_limit" | "exhausted" | "unknown" | (string & {});

export type SwitchHistoryResult = "success" | "failed" | "rolled_back" | (string & {});

export interface SimpleStatus {
  ok: boolean;
  message: string;
}

export interface Account {
  id: string;
  name: string;
  tags: string[];
  auth_fingerprint: string;
  created_at: string;
  updated_at: string;
  last_used_at: string | null;
}

export interface SwitchHistory {
  id: string;
  from_account_id: string | null;
  to_account_id: string;
  snapshot_path: string | null;
  result: SwitchHistoryResult;
  error_message: string | null;
  created_at: string;
}

export interface SwitchResult {
  success: boolean;
  history_id: string;
  snapshot_path: string | null;
  message: string;
}

export interface RuntimeDiagnostics {
  codex_auth_path: string;
  codex_auth_exists: boolean;
  app_data_dir: string;
  db_path: string;
  schema_ok: boolean;
  process_count: number;
}

export interface CodexCliStatus {
  is_running?: boolean;
  running?: boolean;
  process_count?: number;
  checked_at?: string | null;
  last_checked_at?: string | null;
  requires_user_input?: boolean;
  prompt?: string | null;
  current_action?: string | null;
  last_event?: string | null;
  last_event_message?: string | null;
  status?: string | null;
}

export interface QuotaSnapshot {
  id: string;
  account_id: string;
  mode: string;
  remaining_value: number | null;
  remaining_unit: string | null;
  quota_state: QuotaState;
  reset_at: string | null;
  source: string;
  confidence: number;
  reason: string | null;
  created_at: string;
}

export interface QuotaDashboardItem {
  account: Account;
  snapshot: QuotaSnapshot | null;
}

export interface QuotaRefreshPolicyInput {
  timeoutMs: number;
  cacheTtlSeconds: number;
  maxConcurrency: number;
}

export interface AccountDraft {
  name: string;
  tagsText: string;
}

export interface UiNotice {
  kind: "success" | "error" | "info";
  text: string;
}
