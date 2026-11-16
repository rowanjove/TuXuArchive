use crate::error::{err, Result};
use crate::model::SelfCheckReport;
use crate::util::now_iso;
use std::fs;
use std::path::{Path, PathBuf};

pub fn run(data_dir: &Path, db_opened: bool) -> SelfCheckReport {
    let _ = fs::create_dir_all(data_dir);
    let writable = probe_writable(data_dir);
    let (win_ok, win_ver) = windows_info();
    let (wv, wv_ok) = webview2_version();
    let fixed = fixed_runtime_dir().is_some();
    let manifest_status = check_manifest();
    let leftover = leftover_temps(data_dir);
    let startup = if !win_ok
        || !writable
        || !db_opened
        || manifest_status == "mismatch"
        || (!wv_ok && !fixed)
    {
        "failed"
    } else {
        "ok"
    };
    let reason = if !win_ok {
        "当前系统不是受支持的 64 位 Windows。".into()
    } else if !writable {
        "用户数据目录不可写。".into()
    } else if !db_opened {
        "SQLite 数据库无法打开。".into()
    } else if manifest_status == "mismatch" {
        "程序文件与 manifest.sha256 不一致。".into()
    } else if !wv_ok && !fixed {
        "系统未安装 WebView2 运行时，且程序未携带固定版本运行时。本程序不会自动下载运行组件。请安装 Microsoft Edge WebView2 Runtime 后重试。".into()
    } else {
        String::new()
    };
    SelfCheckReport {
        windows_ok: win_ok,
        windows_version: win_ver,
        arch: std::env::consts::ARCH.to_string(),
        manifest_status,
        webview2_version: wv,
        webview2_ok: wv_ok,
        fixed_runtime: fixed,
        data_dir: data_dir.display().to_string(),
        data_dir_writable: writable,
        database_open: db_opened,
        leftover_temps: leftover,
        startup: startup.into(),
        reason,
        app_version: crate::model::APP_VERSION.into(),
        program_dir: std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(|dir| dir.display().to_string()))
            .unwrap_or_default(),
    }
}

pub fn diagnostics(data_dir: &Path, db_opened: bool) -> crate::model::Diagnostics {
    let report = run(data_dir, db_opened);
    let last = data_dir.join("logs").join("startup-last.log");
    let last_self_check = fs::read_to_string(&last).unwrap_or_else(|_| "尚未写入启动自检".into());
    crate::model::Diagnostics {
        version: crate::model::APP_VERSION.into(),
        windows_version: report.windows_version,
        webview2_ok: report.webview2_ok,
        webview2_version: report.webview2_version,
        program_dir: report.program_dir,
        data_dir: report.data_dir,
        last_self_check,
        leftover_temp_count: report.leftover_temps.len(),
        attempted_download: false,
    }
}

pub fn write_log(data_dir: &Path, report: &SelfCheckReport) -> Result<PathBuf> {
    let dir = data_dir.join("logs");
    fs::create_dir_all(&dir).map_err(|e| err("", e.to_string()))?;
    let text = format!(
        "TuXu Archive self-check\n\
         time={}\n\
         windows_suitable={}\n\
         windows_version={}\n\
         arch={}\n\
         manifest={}\n\
         webview2={}\n\
         webview2_version={}\n\
         fixed_runtime={}\n\
         data_dir={}\n\
         data_dir_writable={}\n\
         database_open={}\n\
         leftover_temps={}\n\
         startup={}\n\
         version={}\n\
         program_dir={}\n\
         reason={}\n\
         note=no runtime download\n",
        now_iso(),
        if report.windows_ok { "yes" } else { "no" },
        report.windows_version,
        report.arch,
        report.manifest_status,
        if report.webview2_ok {
            "available"
        } else {
            "missing"
        },
        report.webview2_version.as_deref().unwrap_or("-"),
        if report.fixed_runtime {
            "present"
        } else {
            "absent"
        },
        report.data_dir,
        if report.data_dir_writable {
            "yes"
        } else {
            "no"
        },
        if report.database_open { "yes" } else { "no" },
        report.leftover_temps.len(),
        report.startup,
        report.app_version,
        report.program_dir,
        report.reason,
    );
    let last = dir.join("startup-last.log");
    fs::write(&last, &text).map_err(|e| err("", e.to_string()))?;
    let hist = dir.join("self-check.log");
    let mut prev = fs::read_to_string(&hist).unwrap_or_default();
    if !prev.is_empty() {
        prev.push_str("\n----\n");
    }
    prev.push_str(&text);
    const MAX_LOG_BYTES: usize = 128 * 1024;
    if prev.len() > MAX_LOG_BYTES {
        let mut cut = prev.len() - MAX_LOG_BYTES;
        while cut < prev.len() && !prev.is_char_boundary(cut) {
            cut += 1;
        }
        prev = prev[cut..].to_string();
    }
    fs::write(&hist, prev).map_err(|e| err("", e.to_string()))?;
    Ok(last)
}

pub fn fixed_runtime_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?.join("WebView2Fixed");
    if dir.join("msedgewebview2.exe").is_file() {
        Some(dir)
    } else {
        None
    }
}

fn probe_writable(dir: &Path) -> bool {
    let probe = dir.join(".tuxu-write-probe");
    match fs::write(&probe, b"ok") {
        Ok(()) => {
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

fn windows_info() -> (bool, String) {
    let arch_ok = cfg!(target_arch = "x86_64");
    let os_ok = cfg!(target_os = "windows");
    let version = read_reg(
        winreg::enums::HKEY_LOCAL_MACHINE,
        r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
        "ProductName",
    )
    .unwrap_or_else(|| "unknown".into());
    let build = read_reg(
        winreg::enums::HKEY_LOCAL_MACHINE,
        r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
        "CurrentBuild",
    )
    .unwrap_or_default();
    let text = if build.is_empty() {
        version
    } else {
        format!("{version} build {build}")
    };
    (arch_ok && os_ok, text)
}

fn webview2_version() -> (Option<String>, bool) {
    let paths = [
        (
            winreg::enums::HKEY_LOCAL_MACHINE,
            r"SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}",
        ),
        (
            winreg::enums::HKEY_LOCAL_MACHINE,
            r"SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}",
        ),
        (
            winreg::enums::HKEY_CURRENT_USER,
            r"SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}",
        ),
    ];
    for (hive, path) in paths {
        if let Some(pv) = read_reg(hive, path, "pv") {
            if !pv.is_empty() && pv != "0.0.0.0" {
                return (Some(pv), true);
            }
        }
    }
    (None, false)
}

fn read_reg(hive: winreg::HKEY, path: &str, name: &str) -> Option<String> {
    let key = winreg::RegKey::predef(hive).open_subkey(path).ok()?;
    key.get_value(name).ok()
}

fn check_manifest() -> String {
    let Ok(exe) = std::env::current_exe() else {
        return "unknown".into();
    };
    let Some(dir) = exe.parent() else {
        return "unknown".into();
    };
    let manifest = dir.join("manifest.sha256");
    if !manifest.is_file() {
        return "absent".into();
    }
    let text = fs::read_to_string(&manifest).unwrap_or_default();
    let mut mismatch = false;
    let mut checked = 0u32;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(hash) = parts.next() else { continue };
        let Some(rel) = parts.next() else { continue };
        if rel == "manifest.sha256" {
            continue;
        }
        let path = dir.join(rel);
        checked += 1;
        match crate::util::sha256_file(&path) {
            Ok(got) if got.eq_ignore_ascii_case(hash) => {}
            _ => mismatch = true,
        }
    }
    if checked == 0 {
        "empty".into()
    } else if mismatch {
        "mismatch".into()
    } else {
        "ok".into()
    }
}

fn leftover_temps(data_dir: &Path) -> Vec<String> {
    let marker = data_dir.join("logs").join("leftover-temps.txt");
    fs::read_to_string(marker)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && Path::new(line).exists())
        .map(|line| line.to_string())
        .collect()
}
