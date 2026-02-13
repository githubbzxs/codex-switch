
import { open } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  createAccountFromAuthFile,
  createAccountFromLogin,
  deleteAccount,
  getCodexCliStatus,
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
  CodexCliStatus,
  QuotaDashboardItem,
  QuotaSnapshot,
  RuntimeDiagnostics,
  SimpleStatus,
  SwitchHistory,
  UiNotice,
} from "./types";
import "./App.css";

const HISTORY_LIMIT = 30;
const CLI_STATUS_POLL_MS = 6000;

type WorkspaceView = "overview" | "vault" | "accounts" | "quota" | "history" | "diagnostics";

interface NavItem {
  id: WorkspaceView;
  label: string;
  hint: string;
}

interface NavGroup {
  title: string;
  items: NavItem[];
}

interface RunActionOptions {
  suppressNotice?: boolean;
  onError?: (message: string) => void;
}

const navGroups: NavGroup[] = [
  {
    title: "核心工作区",
    items: [
      { id: "overview", label: "概览", hint: "全局指标与快捷入口" },
      { id: "accounts", label: "账号", hint: "登录导入、编辑与切换" },
      { id: "quota", label: "配额", hint: "刷新与状态追踪" },
      { id: "history", label: "历史", hint: "回滚与审计记录" },
    ],
  },
  {
    title: "系统管理",
    items: [
      { id: "vault", label: "保险库", hint: "主密码与加密状态" },
      { id: "diagnostics", label: "诊断", hint: "运行环境与路径检查" },
    ],
  },
];

const viewMeta: Record<WorkspaceView, { title: string; description: string }> = {
  overview: { title: "全局概览", description: "快速掌握账号规模、配额状态与最新切换结果。" },
  vault: { title: "保险库控制", description: "初始化、解锁与锁定主保险库，保护账号密文。" },
  accounts: { title: "账号管理", description: "通过登录或认证文件新增账号，并维护标签和元数据。" },
  quota: { title: "配额中心", description: "按账号刷新配额，查看来源、置信度与异常原因。" },
  history: { title: "切换历史", description: "审计每次切换结果，可按记录执行回滚。" },
  diagnostics: { title: "运行诊断", description: "检查认证文件、数据库路径和 Codex 进程状态。" },
};

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
    .split(/[,，\n]/g)
    .map((item) => item.trim())
    .filter((item) => item.length > 0);
}

function formatDateTime(value?: string | null): string {
  if (!value) return "--";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return new Intl.DateTimeFormat("zh-CN", { dateStyle: "short", timeStyle: "medium" }).format(date);
}

function normalizeError(error: unknown): string {
  if (error instanceof Error && error.message) return error.message;
  if (typeof error === "string" && error.trim()) return error;
  return "发生未知错误，请稍后重试";
}

function formatRemaining(snapshot?: QuotaSnapshot | null): string {
  if (!snapshot) return "暂无数据";
  if (snapshot.remaining_value === null || Number.isNaN(snapshot.remaining_value)) return "无精确值";
  const unit = snapshot.remaining_unit ? ` ${snapshot.remaining_unit}` : "";
  return `${snapshot.remaining_value.toFixed(2)}${unit}`;
}

function quotaStateClassName(state: string): string {
  if (state === "available") return "state-available";
  if (state === "near_limit") return "state-near-limit";
  if (state === "exhausted") return "state-exhausted";
  return "state-unknown";
}

function historyResultClassName(result: string): string {
  if (result === "success") return "history-success";
  if (result === "failed") return "history-failed";
  if (result === "rolled_back") return "history-rolled";
  return "history-unknown";
}

function buildDraft(account: Account): AccountDraft {
  return { name: account.name, tagsText: account.tags.join(", ") };
}

function App() {
  const [activeView, setActiveView] = useState<WorkspaceView>("overview");
  const [vaultStatus, setVaultStatus] = useState<SimpleStatus | null>(null);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [quotaDashboard, setQuotaDashboard] = useState<QuotaDashboardItem[]>([]);
  const [historyItems, setHistoryItems] = useState<SwitchHistory[]>([]);
  const [diagnostics, setDiagnostics] = useState<RuntimeDiagnostics | null>(null);
  const [codexCliStatus, setCodexCliStatus] = useState<CodexCliStatus | null>(null);
  const [notice, setNotice] = useState<UiNotice | null>(null);

  const [masterPassword, setMasterPassword] = useState("");
  const [newAccountName, setNewAccountName] = useState("");
  const [newAccountTags, setNewAccountTags] = useState("");
  const [authFilePath, setAuthFilePath] = useState("");
  const [selectedAccountId, setSelectedAccountId] = useState("");
  const [forceRestart, setForceRestart] = useState(true);
  const [accountDrafts, setAccountDrafts] = useState<Record<string, AccountDraft>>({});
  const [loadingPage, setLoadingPage] = useState(true);
  const [actionLoading, setActionLoading] = useState<Record<string, boolean>>({});

  const latestCliRef = useRef<CodexCliStatus | null>(null);

  const vaultUnlocked = vaultStatus?.ok ?? false;
  const codexCliRunning = Boolean(codexCliStatus?.is_running);
  const codexProcessCount = codexCliStatus?.process_count ?? diagnostics?.process_count ?? 0;
  const busyActionCount = useMemo(() => Object.values(actionLoading).filter(Boolean).length, [actionLoading]);

  const accountNameMap = useMemo(() => {
    const map = new Map<string, string>();
    accounts.forEach((account) => map.set(account.id, account.name));
    return map;
  }, [accounts]);

  const stats = useMemo(
    () => ({
      accountCount: accounts.length,
      availableQuotaCount: quotaDashboard.filter((item) => item.snapshot?.quota_state === "available").length,
      warningQuotaCount: quotaDashboard.filter((item) => ["near_limit", "exhausted"].includes(item.snapshot?.quota_state ?? "")).length,
      historyCount: historyItems.length,
    }),
    [accounts.length, historyItems.length, quotaDashboard],
  );

  const resolveAccountName = useCallback(
    (id?: string | null) => {
      if (!id) return "空";
      return accountNameMap.get(id) ?? `未知账号(${id.slice(0, 8)})`;
    },
    [accountNameMap],
  );

  const isActionLoading = useCallback((key: string) => Boolean(actionLoading[key]), [actionLoading]);

  const runAction = useCallback(
    async <T,>(key: string, action: () => Promise<T>, options?: RunActionOptions): Promise<T | null> => {
      setActionLoading((prev) => ({ ...prev, [key]: true }));
      setNotice(null);
      try {
        return await action();
      } catch (error) {
        const message = normalizeError(error);
        if (!options?.suppressNotice) {
          setNotice({ kind: "error", text: message });
        }
        options?.onError?.(message);
        return null;
      } finally {
        setActionLoading((prev) => ({ ...prev, [key]: false }));
      }
    },
    [],
  );

  const refreshAllData = useCallback(async (showLoading = false): Promise<boolean> => {
    if (showLoading) setLoadingPage(true);
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
      if (showLoading) setLoadingPage(false);
    }
  }, []);

  const refreshCodexCliStatus = useCallback(
    async (silent = true) => {
      try {
        const latest = await getCodexCliStatus();
        const normalized: CodexCliStatus = {
          is_running: Boolean(latest.is_running),
          process_count: latest.process_count ?? 0,
          checked_at: latest.checked_at ?? new Date().toISOString(),
        };

        const previous = latestCliRef.current;
        if (previous && Boolean(previous.is_running) !== Boolean(normalized.is_running)) {
          if (normalized.is_running) {
            setNotice({ kind: "info", text: `Codex CLI 已启动：检测到 ${normalized.process_count ?? 0} 个相关进程。` });
          } else {
            setNotice({ kind: "info", text: "Codex CLI 已停止：检测到 Codex CLI 进程结束。" });
          }
        }

        latestCliRef.current = normalized;
        setCodexCliStatus(normalized);
      } catch (error) {
        if (!silent) {
          setNotice({ kind: "error", text: `获取 Codex CLI 状态失败：${normalizeError(error)}` });
        }
      }
    },
    [],
  );

  useEffect(() => {
    void refreshAllData(true);
    void refreshCodexCliStatus(false);
  }, [refreshAllData, refreshCodexCliStatus]);

  useEffect(() => {
    const timer = window.setInterval(() => {
      void refreshCodexCliStatus();
    }, CLI_STATUS_POLL_MS);
    return () => window.clearInterval(timer);
  }, [refreshCodexCliStatus]);

  useEffect(() => {
    setAccountDrafts((previous) => {
      const next: Record<string, AccountDraft> = {};
      accounts.forEach((account) => {
        next[account.id] = previous[account.id] ?? buildDraft(account);
      });
      return next;
    });

    setSelectedAccountId((previous) => {
      if (previous && accounts.some((account) => account.id === previous)) return previous;
      return accounts[0]?.id ?? "";
    });
  }, [accounts]);

  const clearCreateFields = () => {
    setNewAccountName("");
    setNewAccountTags("");
    setAuthFilePath("");
  };

  const handleChooseAuthFile = async () => {
    try {
      const selected = await open({
        directory: false,
        multiple: false,
        filters: [{ name: "JSON 认证文件", extensions: ["json"] }],
      });
      const pickedPath = Array.isArray(selected) ? selected[0] : selected;
      if (!pickedPath) return;
      setAuthFilePath(pickedPath);
      setNotice({ kind: "info", text: `已选择认证文件：${pickedPath}` });
    } catch (error) {
      setNotice({ kind: "error", text: `选择认证文件失败：${normalizeError(error)}` });
    }
  };

  const handleInitVault = async () => {
    if (masterPassword.trim().length < 8) {
      setNotice({ kind: "error", text: "初始化保险库至少需要 8 位主密码" });
      return;
    }
    const result = await runAction("init-vault", () => initVault(masterPassword.trim()));
    if (!result) return;
    setNotice({ kind: result.ok ? "success" : "info", text: result.message });
    if (result.ok) setMasterPassword("");
    await refreshAllData();
  };

  const handleUnlockVault = async () => {
    if (!masterPassword.trim()) {
      setNotice({ kind: "error", text: "请输入主密码后再解锁" });
      return;
    }
    const result = await runAction("unlock-vault", () => unlockVault(masterPassword.trim()));
    if (!result) return;
    setNotice({ kind: result.ok ? "success" : "info", text: result.message });
    if (result.ok) setMasterPassword("");
    await refreshAllData();
  };

  const handleLockVault = async () => {
    const result = await runAction("lock-vault", lockVault);
    if (!result) return;
    setNotice({ kind: result.ok ? "success" : "info", text: result.message });
    await refreshAllData();
  };

  const handleRefreshDiagnostics = async () => {
    const result = await runAction("refresh-diagnostics", getRuntimeDiagnostics);
    if (!result) return;
    setDiagnostics(result);
    setNotice({ kind: "success", text: "运行诊断已刷新" });
  };

  const handleImportAccountByLogin = async () => {
    if (!vaultUnlocked) {
      setNotice({ kind: "error", text: "请先解锁保险库，再执行登录添加" });
      return;
    }

    setNotice({ kind: "info", text: "登录流程已启动：已调用 Codex 登录，请在浏览器完成授权后返回应用。" });

    const result = await runAction(
      "import-account-login",
      () => createAccountFromLogin(newAccountName.trim(), parseTags(newAccountTags)),
      {
        suppressNotice: true,
        onError: (message) => {
          setNotice({ kind: "error", text: `登录并添加失败：${message}` });
        },
      },
    );
    if (!result) return;

    setNotice({ kind: "success", text: `登录并添加成功：账号 ${result.name} 已保存到保险库。` });
    clearCreateFields();
    await refreshAllData();
    await refreshCodexCliStatus(false);
  };

  const handleImportAccountByFile = async () => {
    if (!vaultUnlocked) {
      setNotice({ kind: "error", text: "请先解锁保险库，再导入认证文件" });
      return;
    }
    if (!authFilePath.trim()) {
      setNotice({ kind: "error", text: "请先选择认证 JSON 文件" });
      return;
    }
    const result = await runAction("import-account-file", () =>
      createAccountFromAuthFile(newAccountName.trim(), parseTags(newAccountTags), authFilePath.trim()),
    );
    if (!result) return;
    setNotice({ kind: "success", text: `导入认证文件成功：${result.name}` });
    clearCreateFields();
    await refreshAllData();
    await refreshCodexCliStatus(false);
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
      setNotice({ kind: "error", text: "账号名称不能为空" });
      return;
    }
    const result = await runAction(`save-${accountId}`, () =>
      updateAccountMeta(accountId, draft.name.trim(), parseTags(draft.tagsText)),
    );
    if (!result) return;
    setNotice({ kind: result.ok ? "success" : "info", text: result.message });
    await refreshAllData();
  };

  const handleDeleteAccount = async (account: Account) => {
    const confirmed = window.confirm(`确认删除账号「${account.name}」吗？该操作不可恢复。`);
    if (!confirmed) return;
    const result = await runAction(`delete-${account.id}`, () => deleteAccount(account.id));
    if (!result) return;
    setNotice({ kind: result.ok ? "success" : "info", text: result.message });
    await refreshAllData();
  };

  const executeSwitch = async (accountId: string, actionKey: string) => {
    if (!vaultUnlocked) {
      setNotice({ kind: "error", text: "请先解锁保险库，再切换账号" });
      return;
    }
    const accountName = resolveAccountName(accountId);
    const result = await runAction(actionKey, () => switchAccount(accountId, forceRestart), {
      suppressNotice: true,
      onError: (message) => {
        setNotice({ kind: "error", text: `账号切换失败：${accountName}：${message}` });
      },
    });
    if (!result) return;

    setNotice({
      kind: result.success ? "success" : "error",
      text: `${result.success ? "账号切换成功" : "账号切换失败"}：${accountName}：${result.message}`,
    });

    await refreshAllData();
    await refreshCodexCliStatus(false);
  };

  const handleSwitchSelected = async () => {
    if (!selectedAccountId) {
      setNotice({ kind: "error", text: "请先选择目标账号" });
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
    const scopeLabel = accountId ? `账号 ${resolveAccountName(accountId)}` : "全部账号";
    const result = await runAction(actionKey, () => refreshQuota(accountId, true), {
      suppressNotice: true,
      onError: (message) => {
        setNotice({ kind: "error", text: `配额刷新失败：${scopeLabel}：${message}` });
      },
    });
    if (!result) return;

    const message = `已刷新${scopeLabel}，共 ${result.length} 条记录。`;
    setNotice({ kind: "success", text: `配额刷新完成：${message}` });
    await refreshAllData();
  };

  const handleRefreshSelectedQuota = async () => {
    if (!selectedAccountId) {
      setNotice({ kind: "error", text: "请先选择目标账号" });
      return;
    }
    await executeRefreshQuota(selectedAccountId, "refresh-quota-selected");
  };

  const handleRefreshAllQuota = async () => {
    await executeRefreshQuota(undefined, "refresh-quota-all");
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
    if (!confirmed) return;
    const result = await runAction(`rollback-${item.id}`, () => rollbackToHistory(item.id));
    if (!result) return;
    setNotice({ kind: result.success ? "success" : "error", text: result.message });
    await refreshAllData();
  };

  const handleReloadAll = async () => {
    const ok = await refreshAllData(true);
    await refreshCodexCliStatus(false);
    if (ok) setNotice({ kind: "success", text: "页面数据已刷新" });
  };

  const overviewView = (
    <div className="view-stack">
      <section className="view-card">
        <div className="card-head">
          <h3>运行总览</h3>
          <button
            type="button"
            className="btn btn-secondary"
            onClick={handleRefreshAllQuota}
            disabled={!vaultUnlocked || accounts.length === 0 || isActionLoading("refresh-quota-all")}
          >
            {isActionLoading("refresh-quota-all") ? "刷新中..." : "刷新全部配额"}
          </button>
        </div>
        <div className="metric-grid">
          <article className="metric-tile"><span>账号总数</span><strong>{stats.accountCount}</strong></article>
          <article className="metric-tile"><span>配额充足</span><strong>{stats.availableQuotaCount}</strong></article>
          <article className="metric-tile"><span>需关注配额</span><strong>{stats.warningQuotaCount}</strong></article>
          <article className="metric-tile"><span>历史记录</span><strong>{stats.historyCount}</strong></article>
        </div>
      </section>

      <section className="view-card">
        <div className="card-head"><h3>快捷入口</h3></div>
        <div className="shortcut-grid">
          <button type="button" className="shortcut-tile" onClick={() => setActiveView("accounts")}>
            <span>账号管理</span>
            <small>新增账号、编辑标签并执行行内切换。</small>
          </button>
          <button type="button" className="shortcut-tile" onClick={() => setActiveView("quota")}>
            <span>配额中心</span>
            <small>按账号刷新配额，聚焦异常原因与置信度。</small>
          </button>
          <button type="button" className="shortcut-tile" onClick={() => setActiveView("history")}>
            <span>切换历史</span>
            <small>回看操作链路，必要时快速执行回滚。</small>
          </button>
          <button type="button" className="shortcut-tile" onClick={() => setActiveView("diagnostics")}>
            <span>运行诊断</span>
            <small>检查认证路径、数据库与进程健康。</small>
          </button>
        </div>
      </section>

      <section className="view-card">
        <div className="card-head">
          <h3>近期配额状态</h3>
          <button type="button" className="btn btn-ghost" onClick={() => setActiveView("quota")}>查看全部</button>
        </div>
        {quotaDashboard.length === 0 ? (
          <div className="empty-block">暂无配额数据，请先执行刷新。</div>
        ) : (
          <div className="quota-grid">
            {quotaDashboard.slice(0, 6).map((item) => {
              const snapshot = item.snapshot;
              const state = snapshot?.quota_state ?? "unknown";
              return (
                <article className="quota-card" key={item.account.id}>
                  <header>
                    <h4>{item.account.name}</h4>
                    <span className={`state-pill ${quotaStateClassName(state)}`}>
                      {quotaStateText[state] ?? quotaStateText.unknown}
                    </span>
                  </header>
                  <p className="quota-main">剩余额度：{formatRemaining(snapshot)}</p>
                  <p>最近刷新：{formatDateTime(snapshot?.created_at)}</p>
                  <p>来源：{snapshot?.source ?? "--"}</p>
                </article>
              );
            })}
          </div>
        )}
      </section>

      <section className="view-card">
        <div className="card-head">
          <h3>最近切换</h3>
          <button type="button" className="btn btn-ghost" onClick={() => setActiveView("history")}>进入历史</button>
        </div>
        {historyItems.length === 0 ? (
          <div className="empty-block">暂无切换历史。</div>
        ) : (
          <div className="history-stream">
            {historyItems.slice(0, 6).map((item) => (
              <article key={item.id} className="history-stream-item">
                <div>
                  <strong>{resolveAccountName(item.to_account_id)}</strong>
                  <p>{formatDateTime(item.created_at)}</p>
                </div>
                <span className={`history-pill ${historyResultClassName(item.result)}`}>
                  {historyResultText[item.result] ?? item.result}
                </span>
              </article>
            ))}
          </div>
        )}
      </section>
    </div>
  );

  const vaultView = (
    <div className="view-stack">
      <section className="view-card">
        <div className="card-head">
          <h3>保险库状态</h3>
          <span className={`status-badge ${vaultUnlocked ? "ok" : "warn"}`}>{vaultUnlocked ? "已解锁" : "未解锁"}</span>
        </div>
        <label className="field-label">
          主密码
          <input
            type="password"
            value={masterPassword}
            onChange={(event) => setMasterPassword(event.currentTarget.value)}
            placeholder="输入主密码"
            autoComplete="off"
          />
        </label>
        <div className="button-row">
          <button type="button" className="btn btn-primary" onClick={handleInitVault} disabled={isActionLoading("init-vault")}>
            {isActionLoading("init-vault") ? "初始化中..." : "初始化"}
          </button>
          <button type="button" className="btn btn-secondary" onClick={handleUnlockVault} disabled={isActionLoading("unlock-vault")}>
            {isActionLoading("unlock-vault") ? "解锁中..." : "解锁"}
          </button>
          <button type="button" className="btn btn-secondary" onClick={handleLockVault} disabled={isActionLoading("lock-vault")}>
            {isActionLoading("lock-vault") ? "锁定中..." : "锁定"}
          </button>
        </div>
        <p className="muted-text">状态说明：{vaultStatus?.message ?? "正在读取保险库状态..."}</p>
      </section>

      <section className="view-card">
        <h3>操作建议</h3>
        <div className="tips-list">
          <p>1. 主密码建议至少 12 位，并混合大小写、数字与符号。</p>
          <p>2. 切换高频场景建议开启“强制重启 Codex 进程”以减少残留状态。</p>
          <p>3. 执行登录并添加前，请确认浏览器可正常访问授权页面。</p>
        </div>
      </section>
    </div>
  );

  const accountsView = (
    <div className="view-stack">
      <section className="view-card">
        <div className="card-head"><h3>新增账号</h3></div>
        <div className="form-grid">
          <label className="field-label">
            账号名称
            <input
              type="text"
              value={newAccountName}
              onChange={(event) => setNewAccountName(event.currentTarget.value)}
              placeholder="可留空，系统自动命名"
            />
          </label>
          <label className="field-label">
            标签
            <input
              type="text"
              value={newAccountTags}
              onChange={(event) => setNewAccountTags(event.currentTarget.value)}
              placeholder="例如：工作，高频"
            />
          </label>
          <label className="field-label field-span-2">
            认证文件
            <div className="picker-row">
              <input type="text" value={authFilePath} placeholder="请选择 auth.json 文件" readOnly />
              <button type="button" className="btn btn-secondary" onClick={handleChooseAuthFile}>选择文件</button>
            </div>
          </label>
        </div>
        <div className="button-row">
          <button
            type="button"
            className="btn btn-primary"
            onClick={handleImportAccountByLogin}
            disabled={!vaultUnlocked || isActionLoading("import-account-login")}
          >
            {isActionLoading("import-account-login") ? "登录处理中..." : "登录并添加"}
          </button>
          <button
            type="button"
            className="btn btn-secondary"
            onClick={handleImportAccountByFile}
            disabled={!vaultUnlocked || !authFilePath.trim() || isActionLoading("import-account-file")}
          >
            {isActionLoading("import-account-file") ? "导入中..." : "导入认证文件"}
          </button>
        </div>
      </section>

      <section className="view-card">
        <div className="card-head"><h3>账号列表</h3></div>
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
                <tr><td className="empty-cell" colSpan={5}>暂无账号，请先执行登录或导入。</td></tr>
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
                    <td><span className="fingerprint" title={account.auth_fingerprint}>{account.auth_fingerprint}</span></td>
                    <td>{formatDateTime(account.last_used_at)}</td>
                    <td>
                      <div className="action-group">
                        <button type="button" className="btn btn-secondary btn-small" onClick={() => handleSaveAccountMeta(account.id)} disabled={isActionLoading(`save-${account.id}`)}>保存</button>
                        <button type="button" className="btn btn-primary btn-small" onClick={() => handleSwitchFromRow(account.id)} disabled={!vaultUnlocked || isActionLoading(`switch-${account.id}`)}>切换</button>
                        <button type="button" className="btn btn-secondary btn-small" onClick={() => handleRefreshQuotaFromRow(account.id)} disabled={!vaultUnlocked || isActionLoading(`refresh-quota-${account.id}`)}>刷配额</button>
                        <button type="button" className="btn btn-danger btn-small" onClick={() => handleDeleteAccount(account)} disabled={isActionLoading(`delete-${account.id}`)}>删除</button>
                      </div>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      </section>
    </div>
  );

  const quotaView = (
    <div className="view-stack">
      <section className="view-card">
        <div className="card-head"><h3>配额查询</h3></div>
        <div className="operation-grid">
          <label className="field-label">
            目标账号
            <select value={selectedAccountId} onChange={(event) => setSelectedAccountId(event.currentTarget.value)}>
              {accounts.length === 0 && <option value="">暂无可选账号</option>}
              {accounts.map((account) => (<option key={account.id} value={account.id}>{account.name}</option>))}
            </select>
          </label>
          <div className="button-row">
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

      <section className="view-card">
        {quotaDashboard.length === 0 ? (
          <div className="empty-block">暂无配额数据，请先点击刷新。</div>
        ) : (
          <div className="quota-grid">
            {quotaDashboard.map((item) => {
              const snapshot = item.snapshot;
              const state = snapshot?.quota_state ?? "unknown";
              return (
                <article className="quota-card" key={item.account.id}>
                  <header>
                    <h4>{item.account.name}</h4>
                    <span className={`state-pill ${quotaStateClassName(state)}`}>
                      {quotaStateText[state] ?? quotaStateText.unknown}
                    </span>
                  </header>
                  <p className="quota-main">剩余额度：{formatRemaining(snapshot)}</p>
                  <p>最近刷新：{formatDateTime(snapshot?.created_at)}</p>
                  <p>来源：{snapshot?.source ?? "--"}</p>
                  <p>置信度：{snapshot?.confidence ?? "--"}</p>
                  {snapshot?.reason ? <p className="quota-reason">原因：{snapshot.reason}</p> : null}
                  <p>标签：{item.account.tags.join("、") || "无"}</p>
                </article>
              );
            })}
          </div>
        )}
      </section>
    </div>
  );

  const historyView = (
    <div className="view-stack">
      <section className="view-card">
        <div className="card-head"><h3>切换历史</h3></div>
        <div className="table-wrap">
          <table className="data-table">
            <thead>
              <tr>
                <th>时间</th>
                <th>来源账号</th>
                <th>目标账号</th>
                <th>结果</th>
                <th>错误信息</th>
                <th>操作</th>
              </tr>
            </thead>
            <tbody>
              {historyItems.length === 0 && (
                <tr><td className="empty-cell" colSpan={6}>暂无切换历史。</td></tr>
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
                  <td className="error-cell">{item.error_message ?? "--"}</td>
                  <td>
                    <button type="button" className="btn btn-secondary btn-small" onClick={() => handleRollback(item)} disabled={!item.snapshot_path || isActionLoading(`rollback-${item.id}`)}>回滚</button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </section>
    </div>
  );

  const diagnosticsView = (
    <div className="view-stack">
      <section className="view-card">
        <div className="card-head">
          <h3>运行诊断</h3>
          <button type="button" className="btn btn-ghost" onClick={handleRefreshDiagnostics} disabled={isActionLoading("refresh-diagnostics")}>
            {isActionLoading("refresh-diagnostics") ? "刷新中..." : "刷新"}
          </button>
        </div>
        {diagnostics ? (
          <>
            <div className="diagnostic-grid">
              <article className="diagnostic-tile"><span>认证文件</span><strong>{diagnostics.codex_auth_exists ? "存在" : "缺失"}</strong></article>
              <article className="diagnostic-tile"><span>结构校验</span><strong>{diagnostics.schema_ok ? "正常" : "异常"}</strong></article>
              <article className="diagnostic-tile"><span>进程数量</span><strong>{diagnostics.process_count}</strong></article>
            </div>
            <div className="path-grid">
              <div><span>认证路径</span><code>{diagnostics.codex_auth_path}</code></div>
              <div><span>数据库路径</span><code>{diagnostics.db_path}</code></div>
              <div><span>应用数据目录</span><code>{diagnostics.app_data_dir}</code></div>
            </div>
          </>
        ) : (
          <p className="muted-text">暂无诊断数据</p>
        )}
      </section>
    </div>
  );

  const workspaceContent =
    activeView === "vault"
      ? vaultView
      : activeView === "accounts"
      ? accountsView
      : activeView === "quota"
      ? quotaView
      : activeView === "history"
      ? historyView
      : activeView === "diagnostics"
      ? diagnosticsView
      : overviewView;

  const selectedAccountName = selectedAccountId ? resolveAccountName(selectedAccountId) : "未选择账号";

  return (
    <div className="console-shell">
      <aside className="nav-sidebar">
        <div className="brand-box">
          <span className="brand-tag">Codex Switch</span>
          <h1>控制台</h1>
          <p>多账号切换、配额追踪、登录链路监控。</p>
        </div>

        <div className="nav-sections">
          {navGroups.map((group) => (
            <section className="nav-group" key={group.title}>
              <h2>{group.title}</h2>
              <div className="nav-items">
                {group.items.map((item) => (
                  <button
                    type="button"
                    key={item.id}
                    className={`nav-item ${activeView === item.id ? "active" : ""}`}
                    onClick={() => setActiveView(item.id)}
                  >
                    <span>{item.label}</span>
                    <small>{item.hint}</small>
                  </button>
                ))}
              </div>
            </section>
          ))}
        </div>

        <div className="status-stack">
          <div className={`status-badge ${vaultUnlocked ? "ok" : "warn"}`}>保险库：{vaultUnlocked ? "已解锁" : "未解锁"}</div>
          <div className={`status-badge ${codexCliRunning ? "ok" : "idle"}`}>CLI：{codexCliRunning ? "运行中" : "未运行"}</div>
          <div className="status-badge idle">活动任务：{busyActionCount}</div>
        </div>
      </aside>

      <main className="main-region">
        <header className="main-header">
          <div>
            <h2>{viewMeta[activeView].title}</h2>
            <p>{viewMeta[activeView].description}</p>
          </div>
          <div className="header-actions">
            <button type="button" className="btn btn-secondary" onClick={() => void refreshCodexCliStatus(false)}>刷新 CLI 状态</button>
            <button type="button" className="btn btn-primary" onClick={handleReloadAll} disabled={loadingPage}>刷新全部数据</button>
          </div>
        </header>

        <div className="header-subline">
          <span>CLI 进程：{codexProcessCount}</span>
          <span>最近检测：{formatDateTime(codexCliStatus?.checked_at)}</span>
        </div>

        <section className="view-card quick-actions-card">
          <div className="card-head"><h3>快捷操作</h3></div>
          <div className="quick-action-grid">
            <label className="field-label">
              目标账号
              <select value={selectedAccountId} onChange={(event) => setSelectedAccountId(event.currentTarget.value)}>
                {accounts.length === 0 && <option value="">暂无可选账号</option>}
                {accounts.map((account) => (<option key={account.id} value={account.id}>{account.name}</option>))}
              </select>
            </label>
            <label className="checkbox-label">
              <input type="checkbox" checked={forceRestart} onChange={(event) => setForceRestart(event.currentTarget.checked)} />
              切换时强制重启 Codex 进程
            </label>
            <p className="muted-text">当前选择：{selectedAccountName}</p>
            <div className="button-row quick-action-buttons">
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
            </div>
          </div>
        </section>

        {notice && <div className={`inline-notice notice-${notice.kind}`}>{notice.text}</div>}

        <section className="content-region">{workspaceContent}</section>
      </main>

      {loadingPage && <div className="loading-mask">正在加载页面数据...</div>}
    </div>
  );
}

export default App;
