import { FormEvent, useCallback, useEffect, useMemo, useState } from "react";
import {
  createAccountFromLogin,
  deleteAccount,
  getQuotaDashboard,
  getRuntimeDiagnostics,
  getVaultStatus,
  initVault,
  listAccounts,
  listSwitchHistory,
  lockVault,
  refreshQuota,
  rollbackToHistory,
  switchAccount,
  unlockVault,
  updateAccountMeta,
} from "./api";
import type {
  Account,
  AccountDraft,
  QuotaSnapshot,
  RuntimeDiagnostics,
  SimpleStatus,
  SwitchHistory,
  UiNotice,
} from "./types";
import "./App.css";

const HISTORY_LIMIT = 30;

const quotaStateText: Record<string, string> = {
  available: "配额充足",
  near_limit: "接近上限",
  exhausted: "配额耗尽",
  unknown: "状态未知",
};

const historyResultText: Record<string, string> = {
  success: "切换成功",
  failed: "切换失败",
  rolled_back: "已回滚",
};

function parseTags(input: string): string[] {
  return input
    .split(/[,\n，;；]/g)
    .map((item) => item.trim())
    .filter((item) => item.length > 0);
}

function formatDateTime(value?: string | null): string {
  if (!value) {
    return "—";
  }
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return value;
  }
  return new Intl.DateTimeFormat("zh-CN", {
    dateStyle: "short",
    timeStyle: "medium",
  }).format(date);
}

function normalizeError(error: unknown): string {
  if (error instanceof Error && error.message) {
    return error.message;
  }
  if (typeof error === "string" && error.trim()) {
    return error;
  }
  return "发生未知错误，请稍后重试";
}

function formatRemaining(snapshot?: QuotaSnapshot | null): string {
  if (!snapshot) {
    return "暂无数据";
  }
  if (snapshot.remaining_value === null || Number.isNaN(snapshot.remaining_value)) {
    return "无精确值";
  }
  const unit = snapshot.remaining_unit ? ` ${snapshot.remaining_unit}` : "";
  return `${snapshot.remaining_value.toFixed(2)}${unit}`;
}

function quotaStateClassName(state: string): string {
  switch (state) {
    case "available":
      return "state-available";
    case "near_limit":
      return "state-near-limit";
    case "exhausted":
      return "state-exhausted";
    default:
      return "state-unknown";
  }
}

function historyResultClassName(result: string): string {
  switch (result) {
    case "success":
      return "history-success";
    case "failed":
      return "history-failed";
    case "rolled_back":
      return "history-rolled";
    default:
      return "history-unknown";
  }
}

function buildDraft(account: Account): AccountDraft {
  return {
    name: account.name,
    tagsText: account.tags.join(", "),
  };
}

function App() {
  const [vaultStatus, setVaultStatus] = useState<SimpleStatus | null>(null);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [quotaDashboard, setQuotaDashboard] = useState<Array<{ account: Account; snapshot: QuotaSnapshot | null }>>(
    [],
  );
  const [historyItems, setHistoryItems] = useState<SwitchHistory[]>([]);
  const [diagnostics, setDiagnostics] = useState<RuntimeDiagnostics | null>(null);
  const [notice, setNotice] = useState<UiNotice | null>(null);

  const [masterPassword, setMasterPassword] = useState("");
  const [importName, setImportName] = useState("");
  const [importTags, setImportTags] = useState("");
  const [selectedAccountId, setSelectedAccountId] = useState("");
  const [forceRestart, setForceRestart] = useState(true);
  const [accountDrafts, setAccountDrafts] = useState<Record<string, AccountDraft>>({});
  const [loadingPage, setLoadingPage] = useState(true);
  const [actionLoading, setActionLoading] = useState<Record<string, boolean>>({});

  const vaultUnlocked = vaultStatus?.ok ?? false;

  const accountNameMap = useMemo(() => {
    const map = new Map<string, string>();
    for (const account of accounts) {
      map.set(account.id, account.name);
    }
    return map;
  }, [accounts]);

  const resolveAccountName = useCallback(
    (id?: string | null) => {
      if (!id) {
        return "空";
      }
      return accountNameMap.get(id) ?? `未知账户(${id.slice(0, 8)})`;
    },
    [accountNameMap],
  );

  const isActionLoading = useCallback((key: string) => Boolean(actionLoading[key]), [actionLoading]);

  const refreshAllData = useCallback(async (showLoading = false): Promise<boolean> => {
    if (showLoading) {
      setLoadingPage(true);
    }
    try {
      const [status, diagnosticsData, accountList, dashboardData, historyData] = await Promise.all([
        getVaultStatus(),
        getRuntimeDiagnostics(),
        listAccounts(),
        getQuotaDashboard(),
        listSwitchHistory(HISTORY_LIMIT),
      ]);
      setVaultStatus(status);
      setDiagnostics(diagnosticsData);
      setAccounts(accountList);
      setQuotaDashboard(dashboardData);
      setHistoryItems(historyData);
      return true;
    } catch (error) {
      setNotice({ kind: "error", text: `加载数据失败：${normalizeError(error)}` });
      return false;
    } finally {
      if (showLoading) {
        setLoadingPage(false);
      }
    }
  }, []);

  useEffect(() => {
    void refreshAllData(true);
  }, [refreshAllData]);

  useEffect(() => {
    setAccountDrafts((previous) => {
      const next: Record<string, AccountDraft> = {};
      for (const account of accounts) {
        next[account.id] = previous[account.id] ?? buildDraft(account);
      }
      return next;
    });

    setSelectedAccountId((previous) => {
      if (previous && accounts.some((account) => account.id === previous)) {
        return previous;
      }
      return accounts[0]?.id ?? "";
    });
  }, [accounts]);

  const runAction = useCallback(async <T,>(key: string, action: () => Promise<T>): Promise<T | null> => {
    setActionLoading((previous) => ({ ...previous, [key]: true }));
    setNotice(null);
    try {
      return await action();
    } catch (error) {
      setNotice({ kind: "error", text: normalizeError(error) });
      return null;
    } finally {
      setActionLoading((previous) => ({ ...previous, [key]: false }));
    }
  }, []);

  const handleInitVault = async () => {
    if (masterPassword.trim().length < 8) {
      setNotice({ kind: "error", text: "初始化保险库至少需要 8 位主密码" });
      return;
    }
    const result = await runAction("init-vault", () => initVault(masterPassword.trim()));
    if (!result) {
      return;
    }
    setNotice({ kind: result.ok ? "success" : "info", text: result.message });
    if (result.ok) {
      setMasterPassword("");
    }
    await refreshAllData();
  };

  const handleUnlockVault = async () => {
    if (!masterPassword.trim()) {
      setNotice({ kind: "error", text: "请输入主密码后再解锁" });
      return;
    }
    const result = await runAction("unlock-vault", () => unlockVault(masterPassword.trim()));
    if (!result) {
      return;
    }
    setNotice({ kind: result.ok ? "success" : "info", text: result.message });
    if (result.ok) {
      setMasterPassword("");
    }
    await refreshAllData();
  };

  const handleLockVault = async () => {
    const result = await runAction("lock-vault", lockVault);
    if (!result) {
      return;
    }
    setNotice({ kind: result.ok ? "success" : "info", text: result.message });
    await refreshAllData();
  };

  const handleRefreshDiagnostics = async () => {
    const result = await runAction("refresh-diagnostics", getRuntimeDiagnostics);
    if (!result) {
      return;
    }
    setDiagnostics(result);
    setNotice({ kind: "success", text: "运行诊断已刷新" });
  };

  const handleImportAccount = async (event: FormEvent) => {
    event.preventDefault();
    if (!vaultUnlocked) {
      setNotice({ kind: "error", text: "请先解锁保险库，再执行登录添加" });
      return;
    }
    const result = await runAction("import-account", () =>
      createAccountFromLogin(importName.trim(), parseTags(importTags)),
    );
    if (!result) {
      return;
    }
    setNotice({ kind: "success", text: `已完成登录并添加账户：${result.name}` });
    setImportName("");
    setImportTags("");
    await refreshAllData();
  };

  const updateDraftField = (accountId: string, field: keyof AccountDraft, value: string) => {
    setAccountDrafts((previous) => ({
      ...previous,
      [accountId]: {
        ...(previous[accountId] ?? { name: "", tagsText: "" }),
        [field]: value,
      },
    }));
  };

  const handleSaveAccountMeta = async (accountId: string) => {
    const draft = accountDrafts[accountId];
    if (!draft || !draft.name.trim()) {
      setNotice({ kind: "error", text: "账户名称不能为空" });
      return;
    }
    const result = await runAction(`save-${accountId}`, () =>
      updateAccountMeta(accountId, draft.name.trim(), parseTags(draft.tagsText)),
    );
    if (!result) {
      return;
    }
    setNotice({ kind: result.ok ? "success" : "info", text: result.message });
    await refreshAllData();
  };

  const handleDeleteAccount = async (account: Account) => {
    const confirmed = window.confirm(`确认删除账户「${account.name}」吗？该操作不可恢复。`);
    if (!confirmed) {
      return;
    }
    const result = await runAction(`delete-${account.id}`, () => deleteAccount(account.id));
    if (!result) {
      return;
    }
    setNotice({ kind: result.ok ? "success" : "info", text: result.message });
    await refreshAllData();
  };

  const executeSwitch = async (accountId: string, actionKey: string) => {
    if (!vaultUnlocked) {
      setNotice({ kind: "error", text: "请先解锁保险库，再切换账户" });
      return;
    }
    const result = await runAction(actionKey, () => switchAccount(accountId, forceRestart));
    if (!result) {
      return;
    }
    setNotice({ kind: result.success ? "success" : "error", text: result.message });
    await refreshAllData();
  };

  const handleSwitchSelected = async () => {
    if (!selectedAccountId) {
      setNotice({ kind: "error", text: "请先选择目标账户" });
      return;
    }
    await executeSwitch(selectedAccountId, "switch-selected");
  };

  const handleSwitchFromRow = async (accountId: string) => {
    setSelectedAccountId(accountId);
    await executeSwitch(accountId, `switch-${accountId}`);
  };

  const executeRefreshQuota = async (accountId: string | undefined, actionKey: string) => {
    if (!vaultUnlocked) {
      setNotice({ kind: "error", text: "请先解锁保险库，再刷新配额" });
      return;
    }
    const result = await runAction(actionKey, () => refreshQuota(accountId, true));
    if (!result) {
      return;
    }
    const scope = accountId ? "目标账户" : "全部账户";
    setNotice({ kind: "success", text: `已刷新${scope}配额，共 ${result.length} 条记录` });
    await refreshAllData();
  };

  const handleRefreshAllQuota = async () => {
    await executeRefreshQuota(undefined, "refresh-quota-all");
  };

  const handleRefreshSelectedQuota = async () => {
    if (!selectedAccountId) {
      setNotice({ kind: "error", text: "请先选择目标账户" });
      return;
    }
    await executeRefreshQuota(selectedAccountId, "refresh-quota-selected");
  };

  const handleRefreshQuotaFromRow = async (accountId: string) => {
    await executeRefreshQuota(accountId, `refresh-quota-${accountId}`);
  };

  const handleRollback = async (item: SwitchHistory) => {
    if (!item.snapshot_path) {
      setNotice({ kind: "error", text: "该历史记录没有可回滚快照" });
      return;
    }
    const confirmed = window.confirm("确认回滚到该历史快照吗？当前配置将被覆盖。");
    if (!confirmed) {
      return;
    }
    const result = await runAction(`rollback-${item.id}`, () => rollbackToHistory(item.id));
    if (!result) {
      return;
    }
    setNotice({ kind: result.success ? "success" : "error", text: result.message });
    await refreshAllData();
  };

  const handleReloadAll = async () => {
    const ok = await runAction("reload-all", () => refreshAllData());
    if (ok) {
      setNotice({ kind: "success", text: "页面数据已刷新" });
    }
  };

  return (
    <div className="app-shell">
      <section className="panel">
        <div className="section-header">
          <h1 className="main-title">Codex 账户切换控制台</h1>
          <button
            type="button"
            className="btn btn-secondary"
            onClick={handleReloadAll}
            disabled={loadingPage || isActionLoading("reload-all")}
          >
            {isActionLoading("reload-all") ? "刷新中..." : "刷新全部数据"}
          </button>
        </div>

        <div className="status-grid">
          <div>
            <div className={`vault-badge ${vaultUnlocked ? "unlocked" : "locked"}`}>
              保险库：{vaultUnlocked ? "已解锁" : "已锁定"}
            </div>
            <p className="muted-text">{vaultStatus?.message ?? "正在读取保险库状态..."}</p>
          </div>

          <div className="status-actions">
            <label className="field-label">
              主密码
              <input
                type="password"
                value={masterPassword}
                onChange={(event) => setMasterPassword(event.currentTarget.value)}
                placeholder="请输入主密码"
                autoComplete="off"
              />
            </label>
            <div className="button-row">
              <button
                type="button"
                className="btn btn-primary"
                onClick={handleInitVault}
                disabled={isActionLoading("init-vault")}
              >
                {isActionLoading("init-vault") ? "初始化中..." : "初始化保险库"}
              </button>
              <button
                type="button"
                className="btn btn-primary"
                onClick={handleUnlockVault}
                disabled={isActionLoading("unlock-vault")}
              >
                {isActionLoading("unlock-vault") ? "解锁中..." : "解锁保险库"}
              </button>
              <button
                type="button"
                className="btn btn-secondary"
                onClick={handleLockVault}
                disabled={isActionLoading("lock-vault")}
              >
                {isActionLoading("lock-vault") ? "锁定中..." : "锁定保险库"}
              </button>
              <button
                type="button"
                className="btn btn-secondary"
                onClick={handleRefreshDiagnostics}
                disabled={isActionLoading("refresh-diagnostics")}
              >
                {isActionLoading("refresh-diagnostics") ? "刷新中..." : "刷新诊断"}
              </button>
            </div>
          </div>
        </div>

        {diagnostics && (
          <div className="diagnostics-grid">
            <div className="diagnostic-item">
              <span>Codex 登录文件</span>
              <strong>{diagnostics.codex_auth_exists ? "存在" : "缺失"}</strong>
            </div>
            <div className="diagnostic-item">
              <span>登录结构校验</span>
              <strong>{diagnostics.schema_ok ? "正常" : "异常"}</strong>
            </div>
            <div className="diagnostic-item">
              <span>当前 Codex 进程</span>
              <strong>{diagnostics.process_count} 个</strong>
            </div>
            <details className="diagnostic-detail">
              <summary>查看路径详情</summary>
              <p>auth 路径：{diagnostics.codex_auth_path}</p>
              <p>应用数据目录：{diagnostics.app_data_dir}</p>
              <p>数据库路径：{diagnostics.db_path}</p>
            </details>
          </div>
        )}
      </section>

      {notice && <div className={`notice notice-${notice.kind}`}>{notice.text}</div>}

      <div className="two-col-grid">
        <section className="panel">
          <h2 className="section-title">账户管理</h2>
          <form className="inline-form" onSubmit={handleImportAccount}>
            <label className="field-label">
              账户名称
              <input
                type="text"
                value={importName}
                onChange={(event) => setImportName(event.currentTarget.value)}
                placeholder="可留空，系统自动命名"
              />
            </label>
            <label className="field-label">
              标签
              <input
                type="text"
                value={importTags}
                onChange={(event) => setImportTags(event.currentTarget.value)}
                placeholder="例如：工作, 高频"
              />
            </label>
            <button type="submit" className="btn btn-primary" disabled={!vaultUnlocked || isActionLoading("import-account")}>
              {isActionLoading("import-account") ? "登录处理中..." : "登录并添加"}
            </button>
          </form>

          <div className="table-wrap">
            <table className="data-table">
              <thead>
                <tr>
                  <th>名称</th>
                  <th>标签</th>
                  <th>指纹</th>
                  <th>最近使用</th>
                  <th>操作</th>
                </tr>
              </thead>
              <tbody>
                {accounts.length === 0 && (
                  <tr>
                    <td className="empty-cell" colSpan={5}>
                      暂无账户，请先执行“登录并添加”。
                    </td>
                  </tr>
                )}
                {accounts.map((account) => {
                  const draft = accountDrafts[account.id] ?? buildDraft(account);
                  return (
                    <tr key={account.id}>
                      <td>
                        <input
                          type="text"
                          value={draft.name}
                          onChange={(event) => updateDraftField(account.id, "name", event.currentTarget.value)}
                        />
                      </td>
                      <td>
                        <input
                          type="text"
                          value={draft.tagsText}
                          onChange={(event) => updateDraftField(account.id, "tagsText", event.currentTarget.value)}
                        />
                      </td>
                      <td>
                        <span className="fingerprint" title={account.auth_fingerprint}>
                          {account.auth_fingerprint}
                        </span>
                      </td>
                      <td>{formatDateTime(account.last_used_at)}</td>
                      <td>
                        <div className="action-group">
                          <button
                            type="button"
                            className="btn btn-secondary btn-small"
                            onClick={() => handleSaveAccountMeta(account.id)}
                            disabled={isActionLoading(`save-${account.id}`)}
                          >
                            保存
                          </button>
                          <button
                            type="button"
                            className="btn btn-primary btn-small"
                            onClick={() => handleSwitchFromRow(account.id)}
                            disabled={!vaultUnlocked || isActionLoading(`switch-${account.id}`)}
                          >
                            切换
                          </button>
                          <button
                            type="button"
                            className="btn btn-secondary btn-small"
                            onClick={() => handleRefreshQuotaFromRow(account.id)}
                            disabled={!vaultUnlocked || isActionLoading(`refresh-quota-${account.id}`)}
                          >
                            刷配额
                          </button>
                          <button
                            type="button"
                            className="btn btn-danger btn-small"
                            onClick={() => handleDeleteAccount(account)}
                            disabled={isActionLoading(`delete-${account.id}`)}
                          >
                            删除
                          </button>
                        </div>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        </section>

        <section className="panel">
          <h2 className="section-title">配额看板</h2>
          {quotaDashboard.length === 0 ? (
            <div className="empty-block">暂无配额数据，请在操作区点击刷新。</div>
          ) : (
            <div className="quota-grid">
              {quotaDashboard.map((item) => {
                const snapshot = item.snapshot;
                const state = snapshot?.quota_state ?? "unknown";
                return (
                  <article className="quota-card" key={item.account.id}>
                    <header>
                      <h3>{item.account.name}</h3>
                      <span className={`state-pill ${quotaStateClassName(state)}`}>
                        {quotaStateText[state] ?? quotaStateText.unknown}
                      </span>
                    </header>
                    <p className="quota-main">剩余额度：{formatRemaining(snapshot)}</p>
                    <p>最近刷新：{formatDateTime(snapshot?.created_at)}</p>
                    <p>数据来源：{snapshot?.source ?? "—"}</p>
                    <p>置信度：{snapshot?.confidence ?? "—"}</p>
                    <p>标签：{item.account.tags.join("、") || "无"}</p>
                  </article>
                );
              })}
            </div>
          )}
        </section>
      </div>

      <section className="panel">
        <h2 className="section-title">操作区</h2>
        <div className="operation-grid">
          <label className="field-label">
            目标账户
            <select value={selectedAccountId} onChange={(event) => setSelectedAccountId(event.currentTarget.value)}>
              {accounts.length === 0 && <option value="">暂无可选账户</option>}
              {accounts.map((account) => (
                <option key={account.id} value={account.id}>
                  {account.name}
                </option>
              ))}
            </select>
          </label>

          <label className="checkbox-label">
            <input
              type="checkbox"
              checked={forceRestart}
              onChange={(event) => setForceRestart(event.currentTarget.checked)}
            />
            切换时强制重启 Codex 进程
          </label>

          <div className="button-row">
            <button
              type="button"
              className="btn btn-primary"
              onClick={handleSwitchSelected}
              disabled={!vaultUnlocked || !selectedAccountId || isActionLoading("switch-selected")}
            >
              {isActionLoading("switch-selected") ? "切换中..." : "一键切换"}
            </button>
            <button
              type="button"
              className="btn btn-secondary"
              onClick={handleRefreshSelectedQuota}
              disabled={!vaultUnlocked || !selectedAccountId || isActionLoading("refresh-quota-selected")}
            >
              {isActionLoading("refresh-quota-selected") ? "刷新中..." : "刷新所选配额"}
            </button>
            <button
              type="button"
              className="btn btn-secondary"
              onClick={handleRefreshAllQuota}
              disabled={!vaultUnlocked || accounts.length === 0 || isActionLoading("refresh-quota-all")}
            >
              {isActionLoading("refresh-quota-all") ? "刷新中..." : "刷新全部配额"}
            </button>
          </div>
        </div>
      </section>

      <section className="panel">
        <h2 className="section-title">切换历史</h2>
        <div className="table-wrap">
          <table className="data-table history-table">
            <thead>
              <tr>
                <th>时间</th>
                <th>来源账户</th>
                <th>目标账户</th>
                <th>结果</th>
                <th>错误信息</th>
                <th>操作</th>
              </tr>
            </thead>
            <tbody>
              {historyItems.length === 0 && (
                <tr>
                  <td className="empty-cell" colSpan={6}>
                    暂无切换历史。
                  </td>
                </tr>
              )}
              {historyItems.map((item) => (
                <tr key={item.id}>
                  <td>{formatDateTime(item.created_at)}</td>
                  <td>{resolveAccountName(item.from_account_id)}</td>
                  <td>{resolveAccountName(item.to_account_id)}</td>
                  <td>
                    <span className={`history-pill ${historyResultClassName(item.result)}`}>
                      {historyResultText[item.result] ?? item.result}
                    </span>
                  </td>
                  <td className="error-cell">{item.error_message ?? "—"}</td>
                  <td>
                    <button
                      type="button"
                      className="btn btn-secondary btn-small"
                      onClick={() => handleRollback(item)}
                      disabled={!item.snapshot_path || isActionLoading(`rollback-${item.id}`)}
                    >
                      回滚
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </section>

      {loadingPage && <div className="loading-mask">正在加载页面数据...</div>}
    </div>
  );
}

export default App;
