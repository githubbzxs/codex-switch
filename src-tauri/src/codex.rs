use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    ffi::OsString,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use sysinfo::{
    get_current_pid, Pid, ProcessRefreshKind, ProcessesToUpdate, RefreshKind, Signal, System,
};

const CODEX_ENTRY_NAMES: [&str; 5] = ["codex", "codex.exe", "codex.cmd", "codex.ps1", "codex.bat"];

#[derive(Clone, Debug)]
struct CodexCommandTarget {
    program: PathBuf,
    prefix_args: Vec<OsString>,
    display: String,
}

impl CodexCommandTarget {
    fn direct(program: impl Into<PathBuf>, display: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            prefix_args: Vec::new(),
            display: display.into(),
        }
    }

    fn with_prefix_args(
        program: impl Into<PathBuf>,
        prefix_args: Vec<OsString>,
        display: impl Into<String>,
    ) -> Self {
        Self {
            program: program.into(),
            prefix_args,
            display: display.into(),
        }
    }
}

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
    let value: Value = serde_json::from_str(text).context("认证文件 JSON 解析失败")?;
    let auth_type = value
        .get("type")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|field| !field.is_empty())
        .ok_or_else(|| anyhow!("认证文件缺少 type 字段"))?;
    if !auth_type.eq_ignore_ascii_case("codex") {
        return Err(anyhow!("认证文件 type 字段必须为 codex"));
    }

    value
        .get("access_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|field| !field.is_empty())
        .ok_or_else(|| anyhow!("认证文件缺少 access_token 字段"))?;

    Ok(value)
}

pub fn compute_fingerprint(value: &Value) -> Result<String> {
    let (prefix, raw_seed) = if let Some(account_id) = value
        .get("account_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|field| !field.is_empty())
    {
        ("account", account_id)
    } else if let Some(email) = value
        .get("email")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|field| !field.is_empty())
    {
        ("email", email)
    } else if let Some(access_token) = value
        .get("access_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|field| !field.is_empty())
    {
        ("token", access_token)
    } else {
        return Err(anyhow!(
            "无法生成账号指纹：缺少 account_id、email 或 access_token 字段"
        ));
    };

    let normalized_seed = if prefix == "email" {
        raw_seed.to_lowercase()
    } else {
        raw_seed.to_string()
    };

    let mut hasher = Sha256::new();
    hasher.update(format!("{prefix}:{normalized_seed}"));
    let digest = hasher.finalize();
    Ok(format!("{prefix}:{}", &hex::encode(digest)[..16]))
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

fn refresh_processes() -> System {
    let refresh = RefreshKind::nothing().with_processes(ProcessRefreshKind::everything());
    let mut system = System::new_with_specifics(refresh);
    system.refresh_processes(ProcessesToUpdate::All, true);
    system
}

fn current_exe_name() -> Option<String> {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.file_name().map(|name| name.to_string_lossy().to_lowercase()))
}

fn normalize_file_name(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_matches('"');
    if trimmed.is_empty() {
        return None;
    }
    let file_name = Path::new(trimmed)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| trimmed.to_string());
    Some(file_name.to_lowercase())
}

fn is_switch_process_name(name: &str) -> bool {
    name.contains("codex-switch") || name.contains("codex_switch")
}

fn is_codex_entry_name(name: &str) -> bool {
    CODEX_ENTRY_NAMES.iter().any(|candidate| candidate == &name)
}

fn is_codex_cli_process_fields(
    process_name: &str,
    exe_name: Option<&str>,
    cmd_tokens: &[String],
    current_exe: Option<&str>,
) -> bool {
    let normalized_process_name = normalize_file_name(process_name);
    let normalized_exe_name = exe_name.and_then(normalize_file_name);
    let first_cmd = cmd_tokens
        .first()
        .and_then(|token| normalize_file_name(token));

    if normalized_process_name
        .as_deref()
        .map(is_switch_process_name)
        .unwrap_or(false)
        || normalized_exe_name
            .as_deref()
            .map(is_switch_process_name)
            .unwrap_or(false)
    {
        return false;
    }

    if let Some(current_exe) = current_exe.and_then(normalize_file_name) {
        if normalized_process_name.as_deref() == Some(current_exe.as_str())
            || normalized_exe_name.as_deref() == Some(current_exe.as_str())
        {
            return false;
        }
    }

    first_cmd
        .as_deref()
        .map(is_codex_entry_name)
        .unwrap_or(false)
        || normalized_exe_name
            .as_deref()
            .map(is_codex_entry_name)
            .unwrap_or(false)
        || normalized_process_name
            .as_deref()
            .map(is_codex_entry_name)
            .unwrap_or(false)
}

fn collect_codex_cli_pids(system: &System) -> Vec<Pid> {
    let current_pid = get_current_pid().ok();
    let current_exe = current_exe_name();
    system
        .processes()
        .iter()
        .filter_map(|(pid, process)| {
            if current_pid.map(|value| value == *pid).unwrap_or(false) {
                return None;
            }

            let process_name = process.name().to_string_lossy().to_string();
            let exe_name = process
                .exe()
                .and_then(|path| path.file_name())
                .map(|name| name.to_string_lossy().to_string());
            let cmd_tokens = process
                .cmd()
                .iter()
                .map(|part| part.to_string_lossy().to_string())
                .collect::<Vec<_>>();

            if is_codex_cli_process_fields(
                &process_name,
                exe_name.as_deref(),
                &cmd_tokens,
                current_exe.as_deref(),
            ) {
                Some(*pid)
            } else {
                None
            }
        })
        .collect()
}

pub fn count_codex_processes() -> usize {
    let system = refresh_processes();
    collect_codex_cli_pids(&system).len()
}

pub fn kill_codex_processes() -> usize {
    let system = refresh_processes();
    let target_pids = collect_codex_cli_pids(&system);
    let mut killed = 0usize;
    for pid in target_pids {
        if let Some(process) = system.process(pid) {
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

#[cfg(target_os = "windows")]
fn find_files_on_path(file_name: &str) -> Vec<PathBuf> {
    let Some(path_env) = std::env::var_os("PATH") else {
        return Vec::new();
    };
    std::env::split_paths(&path_env)
        .map(|path| path.join(file_name))
        .filter(|path| path.is_file())
        .collect()
}

#[cfg(target_os = "windows")]
fn candidate_vendor_codex_paths() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Ok(current_dir) = std::env::current_dir() {
        candidates.push(current_dir.join("vendor").join("codex.exe"));
        candidates.push(current_dir.join("src-tauri").join("vendor").join("codex.exe"));
    }

    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(dir) = current_exe.parent() {
            candidates.push(dir.join("vendor").join("codex.exe"));
            candidates.push(dir.join("resources").join("vendor").join("codex.exe"));
            candidates.push(dir.join("..").join("vendor").join("codex.exe"));
            candidates.push(dir.join("..").join("resources").join("vendor").join("codex.exe"));
        }
    }

    candidates
}

fn dedupe_command_targets(targets: Vec<CodexCommandTarget>) -> Vec<CodexCommandTarget> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::new();
    for target in targets {
        let key = format!(
            "{}|{}",
            target.program.to_string_lossy().to_lowercase(),
            target
                .prefix_args
                .iter()
                .map(|value| value.to_string_lossy().to_lowercase())
                .collect::<Vec<_>>()
                .join("|")
        );
        if seen.insert(key) {
            deduped.push(target);
        }
    }
    deduped
}

#[cfg(target_os = "windows")]
fn collect_codex_login_targets() -> Vec<CodexCommandTarget> {
    let mut targets = vec![
        CodexCommandTarget::direct("codex.cmd", "codex.cmd (PATH)"),
        CodexCommandTarget::direct("codex.exe", "codex.exe (PATH)"),
        CodexCommandTarget::direct("codex", "codex (PATH)"),
    ];

    for path in find_files_on_path("codex.cmd") {
        targets.push(CodexCommandTarget::direct(
            &path,
            format!("codex.cmd ({})", path.display()),
        ));
    }

    for path in find_files_on_path("codex.exe") {
        targets.push(CodexCommandTarget::direct(
            &path,
            format!("codex.exe ({})", path.display()),
        ));
    }

    for path in find_files_on_path("codex.ps1") {
        targets.push(CodexCommandTarget::with_prefix_args(
            "powershell",
            vec![
                OsString::from("-NoProfile"),
                OsString::from("-ExecutionPolicy"),
                OsString::from("Bypass"),
                OsString::from("-File"),
                path.as_os_str().to_os_string(),
            ],
            format!("powershell -File {}", path.display()),
        ));
    }

    for path in candidate_vendor_codex_paths() {
        targets.push(CodexCommandTarget::direct(
            &path,
            format!("vendor codex.exe ({})", path.display()),
        ));
    }

    dedupe_command_targets(targets)
}

#[cfg(not(target_os = "windows"))]
fn collect_codex_login_targets() -> Vec<CodexCommandTarget> {
    vec![CodexCommandTarget::direct("codex", "codex (PATH)")]
}

fn format_login_command(args: &[&str]) -> String {
    format!("codex {}", args.join(" "))
}

fn spawn_codex_login_process(
    target: &CodexCommandTarget,
    args: &[&str],
) -> std::result::Result<Child, String> {
    let mut command = Command::new(&target.program);
    command
        .args(&target.prefix_args)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    command.spawn().map_err(|error| format!("启动失败：{error}"))
}

fn wait_for_login_completion(
    child: &mut Child,
    command_text: &str,
    timeout_seconds: u64,
) -> std::result::Result<(), String> {
    let started_at = Instant::now();
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("等待 `{command_text}` 进程失败：{error}"))?
        {
            if status.success() {
                return Ok(());
            }

            let output = capture_child_output(child);
            return Err(if output.is_empty() {
                format!("`{command_text}` 未成功完成（退出码：{status}）")
            } else {
                format!("`{command_text}` 未成功完成（退出码：{status}，输出：{output}）")
            });
        }

        if started_at.elapsed() > Duration::from_secs(timeout_seconds) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "`{command_text}` 登录超时（{timeout_seconds}s），请在浏览器完成授权后重试。"
            ));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

pub fn run_codex_login(timeout_seconds: u64) -> Result<()> {
    match run_codex_login_once(&["login", "--web"], timeout_seconds) {
        Ok(()) => Ok(()),
        Err(web_error) => {
            if !is_web_login_unsupported(&web_error) {
                return Err(anyhow!("`codex login --web` 执行失败：{web_error}"));
            }

            run_codex_login_once(&["login"], timeout_seconds).map_err(|fallback_error| {
                anyhow!(
                    "当前 Codex CLI 不支持 `--web`，已自动回退到 `codex login`，但仍失败：{fallback_error}"
                )
            })
        }
    }
}

fn run_codex_login_once(args: &[&str], timeout_seconds: u64) -> std::result::Result<(), String> {
    let command_text = format_login_command(args);
    let targets = collect_codex_login_targets();
    if targets.is_empty() {
        return Err(format!("`{command_text}` 执行失败：未找到可用的 Codex CLI 启动入口"));
    }

    let mut attempts = Vec::new();
    for target in targets {
        let mut child = match spawn_codex_login_process(&target, args) {
            Ok(child) => child,
            Err(error) => {
                attempts.push(format!("{} => {}", target.display, error));
                continue;
            }
        };

        match wait_for_login_completion(&mut child, &command_text, timeout_seconds) {
            Ok(()) => return Ok(()),
            Err(error) => attempts.push(format!("{} => {}", target.display, error)),
        }
    }

    Err(format!(
        "`{command_text}` 执行失败，已尝试路径：{}",
        attempts.join(" | ")
    ))
}

fn capture_child_output(child: &mut Child) -> String {
    let stderr = read_pipe_to_string(&mut child.stderr);
    truncate_for_error(stderr.trim(), 400)
}

fn read_pipe_to_string<R: Read>(pipe: &mut Option<R>) -> String {
    let Some(stream) = pipe.as_mut() else {
        return String::new();
    };
    let mut text = String::new();
    let _ = stream.read_to_string(&mut text);
    text
}

fn truncate_for_error(text: &str, max_len: usize) -> String {
    let cleaned = text.replace('\n', " ").replace('\r', " ").trim().to_string();
    if cleaned.chars().count() <= max_len {
        return cleaned;
    }
    cleaned.chars().take(max_len).collect::<String>() + "..."
}

fn is_web_login_unsupported(message: &str) -> bool {
    let lower = message.to_lowercase();
    let has_web_flag = lower.contains("--web");
    let unsupported = lower.contains("unexpected argument")
        || lower.contains("wasn't expected")
        || lower.contains("unknown option")
        || lower.contains("unrecognized option")
        || lower.contains("no such option");
    has_web_flag && unsupported
}

#[cfg(test)]
mod tests {
    use super::{
        compute_fingerprint, is_codex_cli_process_fields, is_web_login_unsupported,
        validate_auth_json,
    };
    use serde_json::json;

    #[test]
    fn detects_real_codex_cli_process() {
        let cmd = vec!["C:\\Tools\\codex.exe".to_string()];
        assert!(is_codex_cli_process_fields(
            "codex.exe",
            Some("C:\\Tools\\codex.exe"),
            &cmd,
            Some("codex-switch-app.exe")
        ));
    }

    #[test]
    fn ignores_switch_process_itself() {
        let cmd = vec!["C:\\Apps\\codex-switch-app.exe".to_string()];
        assert!(!is_codex_cli_process_fields(
            "codex-switch-app.exe",
            Some("C:\\Apps\\codex-switch-app.exe"),
            &cmd,
            Some("codex-switch-app.exe")
        ));
    }

    #[test]
    fn ignores_non_cli_process_with_codex_argument() {
        let cmd = vec![
            "node.exe".to_string(),
            "worker.js".to_string(),
            "--project=codex-switch".to_string(),
        ];
        assert!(!is_codex_cli_process_fields(
            "node.exe",
            Some("node.exe"),
            &cmd,
            Some("codex-switch-app.exe")
        ));
    }

    #[test]
    fn detects_web_flag_not_supported_error() {
        assert!(is_web_login_unsupported(
            "error: unexpected argument '--web' found"
        ));
    }

    #[test]
    fn validates_codex_auth_json() {
        let text = r#"{
            "type": "  CoDeX  ",
            "access_token": "token-123",
            "account_id": "acc-1"
        }"#;
        assert!(validate_auth_json(text).is_ok());
    }

    #[test]
    fn rejects_auth_json_without_type() {
        let text = r#"{
            "access_token": "token-123",
            "account_id": "acc-1"
        }"#;
        let error = validate_auth_json(text).unwrap_err().to_string();
        assert!(error.contains("type"));
    }

    #[test]
    fn rejects_non_codex_type() {
        let text = r#"{
            "type": "chatgpt",
            "access_token": "token-123",
            "account_id": "acc-1"
        }"#;
        let error = validate_auth_json(text).unwrap_err().to_string();
        assert!(error.contains("codex"));
    }

    #[test]
    fn rejects_auth_json_without_access_token() {
        let text = r#"{
            "type": "codex",
            "account_id": "acc-1"
        }"#;
        let error = validate_auth_json(text).unwrap_err().to_string();
        assert!(error.contains("access_token"));
    }

    #[test]
    fn fingerprint_is_stable_and_distinct() {
        let auth_a = json!({
            "type": "codex",
            "access_token": "token-a",
            "account_id": "account-a",
            "email": "alice@example.com"
        });
        let auth_a_copy = json!({
            "type": "codex",
            "access_token": "token-a",
            "account_id": "account-a",
            "email": "alice@example.com"
        });
        let auth_b = json!({
            "type": "codex",
            "access_token": "token-b",
            "account_id": "account-b",
            "email": "bob@example.com"
        });

        let fp_a = compute_fingerprint(&auth_a).expect("应生成指纹");
        let fp_a_copy = compute_fingerprint(&auth_a_copy).expect("应生成指纹");
        let fp_b = compute_fingerprint(&auth_b).expect("应生成指纹");

        assert_eq!(fp_a, fp_a_copy);
        assert_ne!(fp_a, fp_b);
        assert!(fp_a.starts_with("account:"));
    }
}
