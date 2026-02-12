import { invoke } from "@tauri-apps/api/core";
import type {
  Account,
  QuotaDashboardItem,
  QuotaRefreshPolicyInput,
  QuotaSnapshot,
  RuntimeDiagnostics,
  SimpleStatus,
  SwitchHistory,
  SwitchResult,
} from "./types";

function invokeCommand<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  return invoke<T>(command, args);
}

export function initVault(masterPassword: string): Promise<SimpleStatus> {
  return invokeCommand("init_vault", { masterPassword });
}

export function unlockVault(masterPassword: string): Promise<SimpleStatus> {
  return invokeCommand("unlock_vault", { masterPassword });
}

export function lockVault(): Promise<SimpleStatus> {
  return invokeCommand("lock_vault");
}

export function getVaultStatus(): Promise<SimpleStatus> {
  return invokeCommand("vault_status");
}

export function importCurrentCodexAuth(name: string, tags: string[]): Promise<Account> {
  return invokeCommand("import_current_codex_auth", { name, tags });
}

export function createAccountFromImport(name: string, tags: string[]): Promise<Account> {
  return invokeCommand("create_account_from_import", { name, tags });
}

export function createAccountFromLogin(name: string, tags: string[]): Promise<Account> {
  return invokeCommand("create_account_from_login", { name, tags });
}

export function listAccounts(): Promise<Account[]> {
  return invokeCommand("list_accounts");
}

export function updateAccountMeta(id: string, name: string, tags: string[]): Promise<SimpleStatus> {
  return invokeCommand("update_account_meta", { id, name, tags });
}

export function deleteAccount(id: string): Promise<SimpleStatus> {
  return invokeCommand("delete_account", { id });
}

export function switchAccount(id: string, forceRestart: boolean): Promise<SwitchResult> {
  return invokeCommand("switch_account", { id, forceRestart });
}

export function rollbackToHistory(historyId: string): Promise<SwitchResult> {
  return invokeCommand("rollback_to_history", { historyId });
}

export function listSwitchHistory(limit?: number): Promise<SwitchHistory[]> {
  return invokeCommand("list_switch_history", { limit: limit ?? null });
}

export function refreshQuota(accountId?: string, force?: boolean): Promise<QuotaSnapshot[]> {
  return invokeCommand("refresh_quota", {
    accountId: accountId ?? null,
    force: force ?? null,
  });
}

export function getQuotaDashboard(): Promise<QuotaDashboardItem[]> {
  return invokeCommand("get_quota_dashboard");
}

export function listQuotaSnapshots(accountId: string, limit?: number): Promise<QuotaSnapshot[]> {
  return invokeCommand("list_quota_snapshots", {
    accountId,
    limit: limit ?? null,
  });
}

export function setQuotaRefreshPolicy(policy: QuotaRefreshPolicyInput): Promise<SimpleStatus> {
  return invokeCommand("set_quota_refresh_policy", {
    policy: {
      timeout_ms: policy.timeoutMs,
      cache_ttl_seconds: policy.cacheTtlSeconds,
      max_concurrency: policy.maxConcurrency,
    },
  });
}

export function getRuntimeDiagnostics(): Promise<RuntimeDiagnostics> {
  return invokeCommand("get_runtime_diagnostics");
}
