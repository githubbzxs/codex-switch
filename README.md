# Codex Switch

一个面向 `Codex CLI` 的跨平台桌面应用（Windows / macOS / Linux），用于本地管理多个 ChatGPT 账户登录态，并实现一键切换与配额查询。

## 核心能力

- 保险库模式：主密码加密保存多账户登录数据
- 账户管理：点击“登录并添加”触发 `codex login`，成功后自动保存账号；支持标签分组、编辑、删除
- 一键切换：替换 `Codex CLI` 登录文件并可强制重启进程
- 历史回滚：保存切换快照，支持一键恢复到历史版本
- 配额看板：支持多账号一键刷新，优先显示精确值，失败自动降级到状态模式
- 本地优先：默认零遥测，不上传账号令牌

## 技术栈

- 桌面框架：Tauri 2
- 前端：React + TypeScript + Vite
- 后端：Rust
- 本地存储：SQLite（`rusqlite`）
- 加密：Argon2 + XChaCha20-Poly1305

## 开发环境

- Node.js 20+
- Rust 1.80+
- Tauri CLI 2.x

## 本地运行

```bash
npm install
npm run tauri dev
```

## 打包构建

```bash
npm run tauri build
```

构建产物位于 `src-tauri/target/release/bundle`。

## 后端命令接口（前端通过 `invoke` 调用）

- 保险库：`init_vault`、`unlock_vault`、`lock_vault`、`vault_status`
- 账户：`import_current_codex_auth`、`list_accounts`、`update_account_meta`、`delete_account`
- 切换：`switch_account`、`rollback_to_history`、`list_switch_history`
- 配额：`refresh_quota`、`get_quota_dashboard`、`list_quota_snapshots`、`set_quota_refresh_policy`
- 诊断：`get_runtime_diagnostics`

## 数据目录

应用默认使用系统本地数据目录下的 `codex-switch`：

- Windows：`%LOCALAPPDATA%/codex-switch`
- macOS：`~/Library/Application Support/codex-switch`
- Linux：`~/.local/share/codex-switch`

目录内包含：

- `codex-switch.db`：账户、历史、配额快照数据库
- `snapshots/`：切换前的 `auth.json` 快照

## 安全说明

- 主密码仅用于本地派生加密密钥，不上传网络
- 账户登录数据以密文存储在本地 SQLite
- 配额查询过程仅向官方相关站点发起请求，不将令牌发送到第三方服务
- 切换与回滚会写入本地历史，便于追踪与恢复
