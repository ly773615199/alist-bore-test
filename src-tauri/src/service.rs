use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

use crate::config::*;

// ==================== 状态 ====================

pub struct AppState {
    pub alist_child: Option<Child>,
    pub bore_child: Option<Child>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            alist_child: None,
            bore_child: None,
        }
    }
}

impl Drop for AppState {
    fn drop(&mut self) {
        if let Some(mut child) = self.alist_child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(mut child) = self.bore_child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

// ==================== 密码结构 ====================

#[derive(Serialize, Deserialize)]
pub struct PasswordData {
    pub password: String,
    pub saved_at: String,
    pub note: String,
}

// ==================== 工具函数 ====================

/// 查找二进制文件，优先级：app_data > CWD > resource_dir
fn find_binary(app: &AppHandle, name: &str) -> Result<PathBuf, String> {
    // 1. app_data_dir
    if let Some(dir) = app.path_resolver().app_data_dir() {
        let p = dir.join(name);
        if p.exists() {
            return Ok(p);
        }
    }

    // 2. 当前工作目录
    if let Ok(cwd) = std::env::current_dir() {
        let p = cwd.join(name);
        if p.exists() {
            return copy_to_app_data(app, &p, name);
        }
    }

    // 3. resource_dir (Tauri 嵌入资源)
    if let Some(res_dir) = app.path_resolver().resource_dir() {
        let p = res_dir.join(name);
        if p.exists() {
            return copy_to_app_data(app, &p, name);
        }
    }

    Err(format!("找不到 {}，请将其放在程序同目录", name))
}

/// 复制二进制到 app_data_dir
fn copy_to_app_data(app: &AppHandle, src: &PathBuf, name: &str) -> Result<PathBuf, String> {
    if let Some(app_dir) = app.path_resolver().app_data_dir() {
        let dest = app_dir.join(name);
        if !dest.exists() {
            std::fs::create_dir_all(&app_dir).map_err(|e| e.to_string())?;
            std::fs::copy(src, &dest).map_err(|e| e.to_string())?;
        }
        return Ok(dest);
    }
    Ok(src.clone())
}

/// 获取密码文件路径
fn password_file_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path_resolver()
        .app_data_dir()
        .map(|d| d.join(PASSWORD_FILE))
        .ok_or_else(|| "无法获取数据目录".to_string())
}

/// 启动子进程，等待指定输出或超时，返回匹配行
fn spawn_and_wait_output(
    cmd: &mut Command,
    timeout_secs: u64,
    match_fn: impl Fn(&str) -> Option<String> + Send + 'static,
) -> Result<String, String> {
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::null()) // 修复：不 pipe stderr，避免死锁
        .spawn()
        .map_err(|e| format!("启动进程失败: {}", e))?;

    let stdout = child.stdout.take().ok_or("无法获取 stdout")?;

    let handle = thread::spawn(move || {
        let reader = BufReader::new(stdout);
        let start = Instant::now();

        for line in reader.lines() {
            if start.elapsed() > Duration::from_secs(timeout_secs) {
                return Err("启动超时".to_string());
            }

            match line {
                Ok(l) => {
                    if let Some(result) = match_fn(&l) {
                        return Ok(result);
                    }
                }
                Err(_) => return Err("进程异常退出".to_string()),
            }
        }

        Err("进程异常退出".to_string())
    });

    // 同时等待进程退出或输出匹配，带超时
    let deadline = Instant::now() + Duration::from_secs(timeout_secs + 2);
    loop {
        if handle.is_finished() {
            return handle.join().map_err(|_| "线程 panic".to_string())?;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("启动超时".to_string());
        }
        thread::sleep(Duration::from_millis(100));
    }
}

// ==================== Tauri 命令 ====================

/// 启动 AList 服务
#[tauri::command]
pub async fn cmd_start_alist(
    state: State<'_, Mutex<AppState>>,
    app: AppHandle,
) -> Result<(String, bool), String> {
    let path = find_binary(&app, ALIST_BINARY)?;

    let mut cmd = Command::new(&path);
    cmd.arg("server");

    // 先保存 child 到 state
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("启动 AList 失败: {}", e))?;

    let stdout = child.stdout.take().ok_or("无法获取 stdout")?;

    {
        let mut state = state.lock().map_err(|e| e.to_string())?;
        state.alist_child = Some(child);
    }

    let read_handle = tokio::task::spawn_blocking(move || {
        let reader = BufReader::new(stdout);
        let mut password = String::new();
        let mut is_new = false;
        let start = Instant::now();

        for line in reader.lines() {
            // 修复：每读一行都检查超时（但 lines() 阻塞时不会检查）
            // 所以外层加了 deadline 检查

            match line {
                Ok(l) => {
                    // 修复：用 "initial admin password is:" 作为分隔符
                    if let Some(idx) = l.find("initial admin password is:") {
                        let pw_part = &l[idx + "initial admin password is:".len()..];
                        let pw = pw_part.trim();
                        if !pw.is_empty() {
                            password = pw.to_string();
                            is_new = true;
                        }
                    }
                    // 兼容旧格式 "initial password is:"
                    else if let Some(idx) = l.find("initial password is:") {
                        let pw_part = &l[idx + "initial password is:".len()..];
                        let pw = pw_part.trim();
                        if !pw.is_empty() {
                            password = pw.to_string();
                            is_new = true;
                        }
                    }
                    if l.contains("start HTTP server @ 0.0.0.0:") {
                        return Ok((password, is_new, start.elapsed()));
                    }
                }
                Err(_) => return Err("AList 进程异常退出".to_string()),
            }
        }

        Err("AList 进程异常退出".to_string())
    });

    // 外层超时保护：防止 spawn_blocking 永远阻塞
    let timeout = Duration::from_secs(STARTUP_TIMEOUT_SECS + 5);
    let result = tokio::time::timeout(timeout, read_handle)
        .await
        .map_err(|_| {
            // 超时，清理进程
            kill_alist(&state);
            "AList 启动超时".to_string()
        })?
        .map_err(|_| "AList 启动线程异常".to_string())?;

    match result {
        Ok((mut password, is_new, _elapsed)) => {
            if password.is_empty() {
                password = load_password(&app).unwrap_or_default();
            }
            if !password.is_empty() && is_new {
                save_password(&app, &password).ok();
            }
            Ok((password, is_new))
        }
        Err(e) => {
            kill_alist(&state);
            Err(e)
        }
    }
}

/// 启动 bore 穿透
#[tauri::command]
pub async fn cmd_start_bore(
    state: State<'_, Mutex<AppState>>,
    _app: AppHandle,
) -> Result<String, String> {
    let path = find_binary(&_app, BORE_BINARY)?;

    let mut child = Command::new(&path)
        .arg("local")
        .arg(ALIST_PORT.to_string())
        .arg("--to")
        .arg(BORE_HOST)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("启动 bore 失败: {}", e))?;

    let stdout = child.stdout.take().ok_or("无法获取 stdout")?;

    {
        let mut state = state.lock().map_err(|e| e.to_string())?;
        state.bore_child = Some(child);
    }

    let read_handle = tokio::task::spawn_blocking(move || {
        let reader = BufReader::new(stdout);

        for line in reader.lines() {
            match line {
                Ok(l) => {
                    // bore 输出格式: "listening at bore.pub:PORT"
                    if let Some(idx) = l.find(&format!("listening at {}:", BORE_HOST)) {
                        let after = &l[idx + format!("listening at {}:", BORE_HOST).len()..];
                        let port: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
                        if !port.is_empty() {
                            return Ok(format!("http://{}:{}", BORE_HOST, port));
                        }
                    }
                }
                Err(_) => return Err("bore 进程异常退出".to_string()),
            }
        }

        Err("bore 进程异常退出".to_string())
    });

    // 外层超时保护
    let timeout = Duration::from_secs(STARTUP_TIMEOUT_SECS + 5);
    tokio::time::timeout(timeout, read_handle)
        .await
        .map_err(|_| {
            kill_bore(&state);
            "bore 穿透超时".to_string()
        })?
        .map_err(|_| "bore 启动线程异常".to_string())?
}

/// 停止所有服务
#[tauri::command]
pub fn cmd_stop_services(state: State<'_, Mutex<AppState>>) -> Result<(), String> {
    kill_alist(&state);
    kill_bore(&state);
    Ok(())
}

fn kill_alist(state: &Mutex<AppState>) {
    if let Ok(mut s) = state.lock() {
        if let Some(mut child) = s.alist_child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn kill_bore(state: &Mutex<AppState>) {
    if let Ok(mut s) = state.lock() {
        if let Some(mut child) = s.bore_child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// 获取本地保存的密码
#[tauri::command]
pub fn cmd_get_password(app: AppHandle) -> Result<(String, String, bool), String> {
    let is_first = password_file_path(&app)
        .map(|p| !p.exists())
        .unwrap_or(true);

    let password = load_password(&app).unwrap_or_default();
    let saved_at = load_saved_time(&app).unwrap_or_default();

    Ok((password, saved_at, is_first))
}

/// 打开 URL
#[tauri::command]
pub fn cmd_open_url(app: AppHandle, url: String) -> Result<(), String> {
    tauri::api::shell::open(&app.shell_scope(), &url, None)
        .map_err(|e| format!("打开 URL 失败: {}", e))
}

/// 退出应用
#[tauri::command]
pub fn cmd_quit_app(state: State<'_, Mutex<AppState>>, app: AppHandle) {
    kill_alist(&state);
    kill_bore(&state);
    app.exit(0);
}

// ==================== 密码持久化 ====================

fn save_password(app: &AppHandle, pw: &str) -> Result<(), String> {
    let path = password_file_path(app)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    // base64 编码，避免明文存储
    let encoded_pw = base64_encode(pw);

    let data = PasswordData {
        password: encoded_pw,
        saved_at: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        note: "AList 初始密码（base64编码），登录后请立即修改".to_string(),
    };

    let json = serde_json::to_string_pretty(&data).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())
}

fn load_password(app: &AppHandle) -> Option<String> {
    let path = password_file_path(app).ok()?;
    let content = std::fs::read_to_string(path).ok()?;
    let data: PasswordData = serde_json::from_str(&content).ok()?;
    // 尝试 base64 解码，失败则当明文处理（兼容旧版本）
    Some(base64_decode(&data.password).unwrap_or(data.password))
}

fn load_saved_time(app: &AppHandle) -> Option<String> {
    let path = password_file_path(app).ok()?;
    let content = std::fs::read_to_string(path).ok()?;
    let data: PasswordData = serde_json::from_str(&content).ok()?;
    Some(data.saved_at)
}

// ==================== Base64 编解码（简易实现） ====================

const B64_CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut result = String::with_capacity((bytes.len() + 2) / 3 * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        result.push(B64_CHARS[((triple >> 18) & 0x3F) as usize] as char);
        result.push(B64_CHARS[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            result.push(B64_CHARS[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
        if chunk.len() > 2 {
            result.push(B64_CHARS[(triple & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
    }
    result
}

fn base64_decode(input: &str) -> Option<String> {
    let input = input.trim_end_matches('=');
    let mut bytes = Vec::with_capacity(input.len() * 3 / 4);
    for chunk in input.as_bytes().chunks(4) {
        let mut triple: u32 = 0;
        let mut valid_bits = 0;
        for &c in chunk {
            let val = B64_CHARS.iter().position(|&b| b == c)? as u32;
            triple = (triple << 6) | val;
            valid_bits += 6;
        }
        while valid_bits >= 8 {
            valid_bits -= 8;
            bytes.push((triple >> valid_bits & 0xFF) as u8);
        }
    }
    String::from_utf8(bytes).ok()
}
