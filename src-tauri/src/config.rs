pub const ALIST_PORT: u16 = 5244;
pub const BORE_HOST: &str = "bore.pub";
pub const STARTUP_TIMEOUT_SECS: u64 = 15;
pub const PASSWORD_FILE: &str = "alist_password.json";

#[cfg(target_os = "windows")]
pub const ALIST_BINARY: &str = "alist.exe";
#[cfg(not(target_os = "windows"))]
pub const ALIST_BINARY: &str = "alist";

#[cfg(target_os = "windows")]
pub const BORE_BINARY: &str = "bore.exe";
#[cfg(not(target_os = "windows"))]
pub const BORE_BINARY: &str = "bore";
