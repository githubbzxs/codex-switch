use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, RefreshKind, Signal, System};

pub fn codex_auth_path() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("无法定位当前用户 Home 目录"))?;
    Ok(home.join(".codex").join("auth.json"))
}

pub fn read_and_validate_auth_json(path: &Path) -> Result<Value> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("读取登录文件失败: {}", path.display()))?;
    validate_auth_json(&text)
}

pub fn validate_auth_json(text: &str) -> Result<Value> {
    let value: Value = serde_json::from_str(text).context("登录文件 JSON 解析失败")?;
    let _auth_mode = value
        .get("auth_mode")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("登录文件缺少 auth_mode 字段"))?;
    let _account_id = value
        .pointer("/tokens/account_id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("登录文件缺少 tokens.account_id 字段"))?;
    let _access_token = value
        .pointer("/tokens/access_token")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("登录文件缺少 tokens.access_token 字段"))?;
    Ok(value)
}

pub fn compute_fingerprint(value: &Value) -> Result<String> {
    let auth_mode = value
        .get("auth_mode")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("无法读取 auth_mode"))?;
    let account_id = value
        .pointer("/tokens/account_id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("无法读取 account_id"))?;
    let mut hasher = Sha256::new();
    hasher.update(format!("{auth_mode}:{account_id}"));
    let digest = hasher.finalize();
    Ok(format!(
        "{account_id}:{}",
        hex::encode(digest)[..16].to_string()
    ))
}

pub fn atomic_write(path: &Path, content: &str) -> Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| anyhow!("登录文件路径没有父目录"))?;
    fs::create_dir_all(dir).with_context(|| format!("创建目录失败: {}", dir.display()))?;

    let tmp_path = path.with_extension("json.tmp");
    {
        let mut file = fs::File::create(&tmp_path)
            .with_context(|| format!("创建临时文件失败: {}", tmp_path.display()))?;
        file.write_all(content.as_bytes())
            .with_context(|| format!("写入临时文件失败: {}", tmp_path.display()))?;
        file.sync_all()
            .with_context(|| format!("同步临时文件失败: {}", tmp_path.display()))?;
    }
    fs::rename(&tmp_path, path).with_context(|| format!("替换登录文件失败: {}", path.display()))?;
    Ok(())
}

pub fn create_snapshot(auth_path: &Path, snapshot_dir: &Path) -> Result<Option<PathBuf>> {
    if !auth_path.exists() {
        return Ok(None);
    }
    fs::create_dir_all(snapshot_dir)
        .with_context(|| format!("创建快照目录失败: {}", snapshot_dir.display()))?;
    let snapshot_name = format!("snapshot-{}.json", Utc::now().format("%Y%m%d%H%M%S%.3f"));
    let snapshot_path = snapshot_dir.join(snapshot_name);
    fs::copy(auth_path, &snapshot_path).with_context(|| {
        format!(
            "备份登录文件失败: {} -> {}",
            auth_path.display(),
            snapshot_path.display()
        )
    })?;
    Ok(Some(snapshot_path))
}

pub fn kill_codex_processes() -> usize {
    let refresh = RefreshKind::nothing().with_processes(ProcessRefreshKind::everything());
    let mut system = System::new_with_specifics(refresh);
    system.refresh_processes(ProcessesToUpdate::All, true);
    let mut killed = 0usize;
    for process in system.processes().values() {
        let name = process.name().to_string_lossy().to_lowercase();
        let cmdline = process
            .cmd()
            .iter()
            .map(|s| s.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        if name.contains("codex") || cmdline.contains("codex") {
            if process
                .kill_with(Signal::Kill)
                .unwrap_or_else(|| process.kill())
            {
                killed += 1;
            }
        }
    }
    killed
}

pub fn restart_codex() -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        Command::new("cmd")
            .args(["/C", "start", "", "codex"])
            .spawn()
            .context("重启 Codex CLI 失败")?;
    }
    #[cfg(not(target_os = "windows"))]
    {
        Command::new("sh")
            .arg("-lc")
            .arg("codex >/dev/null 2>&1 &")
            .spawn()
            .context("重启 Codex CLI 失败")?;
    }
    Ok(())
}

pub fn run_codex_login(timeout_seconds: u64) -> Result<()> {
    let mut child = Command::new("codex")
        .arg("login")
        .spawn()
        .context("启动 `codex login` 失败，请确认已安装 Codex CLI")?;
    let started_at = Instant::now();
    loop {
        if let Some(status) = child.try_wait().context("等待登录进程失败")? {
            if status.success() {
                return Ok(());
            }
            return Err(anyhow!("`codex login` 未成功完成（退出码：{status}）"));
        }
        if started_at.elapsed() > Duration::from_secs(timeout_seconds) {
            let _ = child.kill();
            return Err(anyhow!("登录超时，请重试并在浏览器完成授权"));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}
