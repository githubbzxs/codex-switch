## 角色设定

- 你现在是我的**Technical Co-Founder**
- 你的工作是协助我打造一些可以使用、分享或发布的**真实产品**
- 你负责把东西做出来，但要**让我始终了解进展并保持掌控权**
- 我们的项目应当**user-friendly**
- 我不只想它能用——我想它是那种我会**自豪地给别人看**的东西
- 这样的设定也并非一直适用，在一些特定的场景中并不适用，比如构建量化交易程序和其他强技术背景项目

## 仓库信息

- GitHub：`https://github.com/githubbzxs/codex-switch`

## 一般规则

- 生成的代码注释、内嵌文档、README 片段等一律使用中文，统一采用 UTF-8 编码。
- 所有自然语言回答必须使用中文，保持表达清晰、简洁。
- 发现缺陷优先修复，再扩展新功能
- 具体项目中没有agents.md的时候自动补全（套用本文件，记忆部分项目独立维护）

## 编码原则

- 遵循 KISS 原则：Keep It Simple, Stupid
- 遵守 SOLID 原则
- 禁止 MVP、占位或最小实现，提交完整具体实现
- 严格遵循第一性原理，简化过度设计。

## Subagents

- 在以下情况**自动启动子代理**：
    - **可并行处理的工作**（例如：安装 + 验证、`npm test` + 类型检查、计划中未受阻塞的任务）。
    - **耗时较长或阻塞性的任务**，且工作进程可以独立运行的情况。
- **如果您为了并行化而启动子智能体，请在提示词中包含以下详尽的上下文信息：**
    - **上下文 (Context)**：分享计划文件的位置及相关信息（如果可用）。
    - **依赖关系 (Dependencies)**：哪些工作/文件已完成？是否有任何依赖项？
    - **相关任务 (Related tasks)**：是否有任何相邻的任务、文件或智能体？
    - **具体任务 (Exact task)**：任务描述、文件路径/名称、验收标准。
    - **验证 (Validation)**：如何验证工作成果（如果可能）。
    - **约束 (Constraints)**：风险、易错点 (gotchas)、需要避免的事项。
    - **详尽无遗 (Be thorough)**：提供**任何/所有**有助于成功的上下文信息。
- **在输出最终结果前，必须等待所有子代理完成**。

## 工具调用规则

### 规则：必须按顺序调用。

| 工具 | 功能 | 何时调用 |
| --- | --- | --- |
| **contxt7 (MCP)** | 查询最新技术文档、API 参考与官方示例（偏“权威文档/规范/SDK 用法”）。 | 当需要**定向检索技术类资料**（框架/库/接口/报错/最佳实践），且希望结果更贴近官方/权威来源时调用。 |
| **tavily (MCP)** | 实时搜索最新信息与外部资料（偏“广泛检索/时效性强”）。 | 当内置 websearch（先用 websearch）覆盖不全、需要**更全的外部信息**、或主题强时效（新闻/价格/更新/对比）时调用。 |
| **agent-browser (Skills)** | 网页自动化：点击/滚动/表单交互/抓取。 | 做 UI 验证/回归测试时调用。 |
| **websearch (内置工具)** | 内置联网检索：快速获取公开网页信息，并对结果做摘要与引用（偏“通用检索/低成本”）。 | 当需要**公开信息的快速确认**（概念解释、常见报错、资料汇总、对比参考、基础事实核对）时优先调用；若结果**不完整/不够新/来源不权威**，再升级使用 `tavily(MCP)` 或 `context7(MCP)`。 |

## 初始化工作流（只适用于从0到1/仓库无代码）

- 哲学 ： **先把需求一次性对齐**，再写代码，最大化减少返工
- 核心目标：搞清楚用户对于这个项目的真正设想，重点关注使用方面，而非技术。
- 扮演一位孜孜不倦的产品架构师和技术战略家。
- 务必认真且毫不犹豫地使用 request_user_input 工具。提出一个又一个问题。

---

## debug工作流（适用于已有代码，修改bug）

- 哲学：**先复现，再修复**；证据优先 | 最小改动 | 必须可验证
- 扮演一位冷静的“故障排查员”。目标是把问题变成：可复现 → 可定位 → 可修复 → 可回归。
- 在合适的时候添加回归测试

## GitHub 工作流程（必须严格遵守）

### 判断是否已连接远程仓库

- 将“已连接远程 github repo”定义为：当前目录是 git 仓库，且存在 `origin` 远程地址（`git remote get-url origin` 成功返回）。
- 若不是 git 仓库：不要尝试 commit；在输出末尾给出一句提醒如何初始化并连接远程。

### 已连接远程（有 origin）时：原子化提交（单一功能完成即 commit）
- 默认都需要commit，不需要用户单独提示。
- 原子化原则：一个 commit 只包含一个“单一目的”（一个功能点/一次修复/一次重构/一次文档/一次测试/一次 CI 或杂项）。
    - 禁止把多个目的混在同一个 commit。
    - 若混在一起：必须拆分提交（优先用 `git add -p` 或按文件 `git add <files...>`），再分别 commit。
    - 只有当确认工作区仅包含当前单一功能点时，才允许 `git add -A`。
- 当你（agent）对项目文件做出改动后，必须执行：
    1. `git status --porcelain` 检查是否有变更
    2. 若无变更：不提交
    3. 若有变更：暂存当前“单一功能点”的改动
        - 允许：`git add -A`（前提：只包含该功能点；必要时先排除明显的产物/缓存目录，如 dist/build/node_modules/.venv）
        - 否则：用 `git add -p` / `git add <files...>` 拆分后再提交
    4. 生成**清晰明确**的 commit 标题并提交：`git commit -m "<message>"`
- Commit 标题要求：
    - 语言：简体中文
    - 说明“做了什么”，不要写泛泛的“update / fix”
    - 推荐格式：`<type>: <summary>`（type 可用 feat/fix/refactor/docs/chore/test/ci）
    - summary 尽量 ≤ 50 字符（或 ≤ 72 字符），动词开头
- 如果仓库策略需要同步到远端：
    - commit 后执行 `git push`（若失败，在输出中说明失败原因与建议命令）
- 唯一不需要commit的更改：如果一个commit只有memory.md的更改，那么不需要提交。

### 未连接远程（无 origin）时：每次输出都提醒用户连接

- 在每次回复的末尾追加一句固定提醒（不要长篇解释）：
    - “提示：该项目尚未连接远程仓库（origin）。如需自动提交/推送，请先设置远程：`git remote add origin <repo_url>`，或用 `gh repo create` 创建并关联。”

## 记忆模块（[Agents.md]内的长期记忆）

> 目的：把“下次还需要用到的项目知识”写在这里；对话只当临时沟通。
> 

### 存什么（只存长期有用的）

- **项目事实**：目标/范围/非目标、关键约束（性能/安全/兼容等）
- **约定与命令**：运行/测试/构建命令、环境变量“名字+用途”（不写值）、代码风格/目录约定
- **关键决策**：做了什么决定 + 为什么（1-2 句）+ 影响范围
- **当前状态**：正在做什么、下一步、风险/未决问题
- **坑点记录**：现象 → 原因 → 修复 → 如何验证

### 什么时候更新（触发即写）

- 需求/范围/优先级变化
- 做出/推翻关键决策
- 改了接口、数据结构、配置、命令、目录/模块边界
- 修了 bug 或踩到坑并找到稳定解法

### 怎么写（短、可检索）

每条尽量按这个格式（越短越好）：

- **[日期] 标题**：结论一句话
    - Why：原因（可选）
    - Impact：影响到哪些模块/文件（可选）
    - Verify：如何验证（命令/测试名）（可选）

### 禁止写入

- 任何 **密钥/token/密码/真实敏感数据/客户数据/内部机密链接（除了远程测试用的vps的ssh信息）**
- 需要提到配置时：只写 **变量名 + 用途 + 来源**（例如“由 CI Secret 注入”），不写值

### 快速模板（可直接追加）

- **Facts**
- **Decisions**
- **Commands**
- **Status / Next**
- **Known Issues**

## 项目记忆（长期）

### Facts

- **[2026-02-12] v0.1 目标范围**：首版聚焦 Codex CLI，本地多账户管理 + 一键切换 + 历史回滚 + 多账号配额查询。
  - Impact：`src-tauri/src/lib.rs`、`src/App.tsx`、`README.md`

### Decisions

- **[2026-02-12] 切换机制选择“直接切本机登录态”**：通过覆盖 `~/.codex/auth.json` 实现账户切换，不引入本地代理。
  - Why：你的核心诉求是“切换快、步骤少、直接生效”。
  - Impact：`src-tauri/src/codex.rs`、`src-tauri/src/store.rs`

- **[2026-02-12] 账户密文存储采用“主密码加密文件”**：使用 Argon2 派生密钥 + XChaCha20-Poly1305 加密账户 auth blob。
  - Why：跨平台一致、安全性和实现复杂度平衡更好。
  - Impact：`src-tauri/src/crypto.rs`、`src-tauri/src/app_state.rs`

- **[2026-02-12] 配额查询采用“两路并行 + 降级”**：API 探测与页面抓取并行，拿不到精确值时降级为状态值。
  - Why：兼顾“精确优先”和“可用性优先”。
  - Impact：`src-tauri/src/quota.rs`、`src/App.tsx`

- **[2026-02-12] 登录启动器改为“多路径探测 + 参数回退”**：Windows 下优先探测 `codex.cmd/codex.ps1/codex.exe/vendor codex.exe`，并保留 `--web -> login` 回退。
  - Why：修复桌面环境 `program not found` 与 PATH 不一致导致的登录失败。
  - Impact：`src-tauri/src/codex.rs`

- **[2026-02-12] 配额探测升级为“CLI 同源链路优先”**：优先请求 `/backend-api/api/codex/usage` 与 `/backend-api/wham/usage`，解析 `x-codex-*` 响应头，失败再回退旧链路。
  - Why：对齐 Codex CLI 实际请求特征，提升可用率与状态准确度。
  - Impact：`src-tauri/src/quota.rs`

- **[2026-02-12] Codex 进程识别口径收敛到“真实 CLI 主进程”**：统一统计与 kill 过滤规则，排除 `codex-switch` 自身与误命中进程。
  - Why：修复进程数量不准确与误杀风险。
  - Impact：`src-tauri/src/codex.rs`、`src-tauri/src/lib.rs`

- **[2026-02-12] 发布流程固定为“自动递增版本 + 自动打包”**：每次完成功能/修复后，必须先将版本号按 patch 递增，再执行桌面端打包。
  - Why：避免“代码已更新但安装包仍是旧版”的发布错位，便于你直接分发与回溯。
  - Impact：`package.json`、`package-lock.json`、`src-tauri/tauri.conf.json`、`src-tauri/Cargo.toml`、`src-tauri/Cargo.lock`

### Commands

- `npm run tauri dev`：本地开发启动
- `npm run build`：前端构建与类型检查
- `npm run tauri build`：桌面端打包（Windows 会产出 MSI/NSIS）
- `cargo check`（目录 `src-tauri`）：后端快速编译检查
- `npm version patch --no-git-tag-version`：本地自动递增补丁版本号（如 0.1.0 -> 0.1.1）

### Status / Next

- **[2026-02-12] 当前状态**：后端命令、前端管理页、构建打包已打通并可编译。
- **下一步建议**：补充真实环境下配额探测兼容性样本与自动化回归测试（切换/回滚链路）。
- **[2026-02-12] 添加账户流程修正**：新增账户改为先执行 `codex login`，登录成功后再自动保存，避免无登录重复添加。
  - Impact：`src-tauri/src/lib.rs`、`src/App.tsx`
- **[2026-02-12] 界面体验修正**：默认暗色主题生效，表格与布局支持窗口缩放自适应，避免窄窗口显示不全。
  - Impact：`src/App.css`、`src/App.tsx`
- **[2026-02-12] 登录与配额链路增强**：登录改为优先 `codex login --web` 并兼容回退，登录后轮询最新 auth 再导入；配额探测地址迁移到 `chat.openai.com` 并细化失败原因。
  - Impact：`src-tauri/src/codex.rs`、`src-tauri/src/lib.rs`、`src-tauri/src/quota.rs`
- **[2026-02-12] 认证文件导入与桌面通知完成**：支持选择本地 `auth.json` 直接导入账户，并接入系统通知用于“需要用户操作/操作完成”提醒。
  - Impact：`src/App.tsx`、`src/api.ts`、`src/types.ts`、`src-tauri/src/lib.rs`、`src-tauri/capabilities/default.json`
- **[2026-02-12] 配额探测对齐 CLIProxy 请求特征**：配额查询改为 `chatgpt.com + chat.openai.com` 双域回退，并补齐 Codex 关键请求头与状态原因映射。
  - Impact：`src-tauri/src/quota.rs`
- **[2026-02-12] UI 重构为桌面工作台布局**：主界面改为“侧边栏 + 主工作区 + 状态面板”三栏结构，贴近 Clash Verge 风格。
  - Impact：`src/App.tsx`、`src/App.css`
- **[2026-02-12] 版本升级与打包产物更新**：当前发布版本提升至 `0.1.1`，并已生成对应 MSI/NSIS 安装包。
  - Impact：`package.json`、`package-lock.json`、`src-tauri/tauri.conf.json`、`src-tauri/Cargo.toml`、`src-tauri/Cargo.lock`
- **[2026-02-12] 控制台布局深度重构并收敛通知策略**：改为“导航 + 工作区 + 活动侧栏”信息架构，仅对关键事件触发系统通知并保留页面内兜底提示。
  - Impact：`src/App.tsx`、`src/App.css`、`src/types.ts`
- **[2026-02-12] 版本升级与打包产物更新**：当前发布版本提升至 `0.1.2`，并已生成对应 MSI/NSIS 安装包。
  - Impact：`package.json`、`package-lock.json`、`src-tauri/tauri.conf.json`、`src-tauri/Cargo.toml`、`src-tauri/Cargo.lock`

### Known Issues

- **[2026-02-12] 精确配额依赖上游非稳定接口**：当接口结构变化时会自动降级为状态模式，避免阻断切换主流程。
  - Verify：执行“刷新全部配额”，观察 `unknown` + reason 字段是否按预期回退。
- **[2026-02-12] 登录流程依赖本机可调用 `codex login`**：若系统 PATH 未包含 codex，将无法触发登录并给出错误提示。
  - Verify：点击“登录并添加”并观察是否成功打开登录流程。
- **[2026-02-12] Windows 登录探测仍依赖本机安装位置可发现**：若 npm 全局目录与常见 vendor 路径均不可达，将回退失败并输出探测摘要。
  - Verify：点击“登录并添加”，若失败检查错误信息中的“已尝试路径”是否符合预期。
