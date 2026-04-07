use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
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
            // 复制到 app_data_dir
            if let Some(app_dir) = app.path_resolver().app_data_dir() {
                let dest = app_dir.join(name);
                if !dest.exists() {
                    std::fs::create_dir_all(&app_dir).ok();
                    std::fs::copy(&p, &dest).ok();
                }
                return Ok(dest);
            }
            return Ok(p);
        }
    }

    // 3. resource_dir (Tauri 嵌入资源)
    if let Some(res_dir) = app.path_resolver().resource_dir() {
        let p = res_dir.join(name);
        if p.exists() {
            if let Some(app_dir) = app.path_resolver().app_data_dir() {
                let dest = app_dir.join(name);
                if !dest.exists() {
                    std::fs::create_dir_all(&app_dir).ok();
                    std::fs::copy(&p, &dest).ok();
                }
                return Ok(dest);
            }
            return Ok(p);
        }
    }

    Err(format!("找不到 {}，请将其放在程序同目录", name))
}

/// 获取密码文件路径
fn password_file_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path_resolver()
        .app_data_dir()
        .map(|d| d.join(PASSWORD_FILE))
        .ok_or_else(|| "无法获取数据目录".to_string())
}

// ==================== Tauri 命令 ====================

/// 启动 AList 服务
#[tauri::command]
pub async fn cmd_start_alist(
    state: State<'_, Mutex<AppState>>,
    app: AppHandle,
) -> Result<(String, bool), String> {
    let path = find_binary(&app, ALIST_BINARY)?;

    let mut child = Command::new(&path)
        .arg("server")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("启动 AList 失败: {}", e))?;

    let stdout = child.stdout.take().ok_or("无法获取 stdout")?;
    // let app_handle = app.clone();

    // 保存子进程引用
    {
        let mut state = state.lock().map_err(|e| e.to_string())?;
        state.alist_child = Some(child);
    }

    // 在阻塞线程中读取 stdout
    // let app2 = app_handle.clone();
    let read_handle = tokio::task::spawn_blocking(move || {
        let reader = BufReader::new(stdout);
        let mut password = String::new();
        let mut is_new = false;
        let start = Instant::now();

        for line in reader.lines() {
            // 检查超时
            if start.elapsed() > Duration::from_secs(STARTUP_TIMEOUT_SECS) {
                return Err("AList 启动超时".to_string());
            }

            match line {
                Ok(l) => {
                    // 发送事件到前端
                    // app2.emit("alist-line", l.clone()).ok();

                    if l.contains("initial password is:") {
                        if let Some(pw) = l.split(':').last() {
                            password = pw.trim().to_string();
                            is_new = true;
                        }
                    }
                    if l.contains("start HTTP server @ 0.0.0.0:") {
                        return Ok((password, is_new));
                    }
                }
                Err(_) => return Err("AList 进程异常退出".to_string()),
            }
        }

        Err("AList 进程异常退出".to_string())
    });

    let result = read_handle.await.map_err(|e| e.to_string())?;

    match result {
        Ok((mut password, is_new)) => {
            if password.is_empty() {
                password = load_password(&app).unwrap_or_default();
            }
            if !password.is_empty() && is_new {
                save_password(&app, &password).ok();
            }
            Ok((password, is_new))
        }
        Err(e) => {
            // 失败时清理进程
            cmd_stop_services_internal(&state);
            Err(e)
        }
    }
}

/// 启动 bore 穿透
#[tauri::command]
pub async fn cmd_start_bore(
    state: State<'_, Mutex<AppState>>,
    app: AppHandle,
) -> Result<String, String> {
    let path = find_binary(&app, BORE_BINARY)?;

    let mut child = Command::new(&path)
        .arg("local")
        .arg(ALIST_PORT.to_string())
        .arg("--to")
        .arg(BORE_HOST)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("启动 bore 失败: {}", e))?;

    let stdout = child.stdout.take().ok_or("无法获取 stdout")?;
    // let app_handle = app.clone();

    {
        let mut state = state.lock().map_err(|e| e.to_string())?;
        state.bore_child = Some(child);
    }

    let read_handle = tokio::task::spawn_blocking(move || {
        let reader = BufReader::new(stdout);
        let start = Instant::now();

        for line in reader.lines() {
            if start.elapsed() > Duration::from_secs(STARTUP_TIMEOUT_SECS) {
                return Err("bore 穿透超时".to_string());
            }

            match line {
                Ok(l) => {
                    // app_handle.emit("bore-line", l.clone()).ok();

                    if l.contains(&format!("listening at {}:", BORE_HOST)) {
                        if let Some(port) = l.split(':').last() {
                            let url = format!("http://{}:{}", BORE_HOST, port.trim());
                            return Ok(url);
                        }
                    }
                }
                Err(_) => return Err("bore 进程异常退出".to_string()),
            }
        }

        Err("bore 进程异常退出".to_string())
    });

    read_handle.await.map_err(|e| e.to_string())?
}

/// 停止所有服务
#[tauri::command]
pub fn cmd_stop_services(state: State<'_, Mutex<AppState>>) -> Result<(), String> {
    cmd_stop_services_internal(&state);
    Ok(())
}

fn cmd_stop_services_internal(state: &Mutex<AppState>) {
    if let Ok(mut s) = state.lock() {
        if let Some(mut child) = s.alist_child.take() {
            child.kill().ok();
            child.wait().ok();
        }
        if let Some(mut child) = s.bore_child.take() {
            child.kill().ok();
            child.wait().ok();
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
    cmd_stop_services_internal(&state);
    app.exit(0);
}

// ==================== 密码持久化 ====================

fn save_password(app: &AppHandle, pw: &str) -> Result<(), String> {
    let path = password_file_path(app)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let data = PasswordData {
        password: pw.to_string(),
        saved_at: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        note: "AList 初始密码，登录后请立即修改".to_string(),
    };

    let json = serde_json::to_string_pretty(&data).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())
}

fn load_password(app: &AppHandle) -> Option<String> {
    let path = password_file_path(app).ok()?;
    let content = std::fs::read_to_string(path).ok()?;
    let data: PasswordData = serde_json::from_str(&content).ok()?;
    Some(data.password)
}

fn load_saved_time(app: &AppHandle) -> Option<String> {
    let path = password_file_path(app).ok()?;
    let content = std::fs::read_to_string(path).ok()?;
    let data: PasswordData = serde_json::from_str(&content).ok()?;
    Some(data.saved_at)
}