#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const ZIP: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/payload.zip"));

fn main() {
    if let Err(err) = run() {
        write_line(&format!("error={err}\nnote=no runtime download\n"));
        tell(&err);
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe_dir = exe.parent().ok_or("无法定位程序所在目录")?.to_path_buf();
    let embedded = normalize_text(&zip_text_from(ZIP, "manifest.sha256")?);
    let fingerprint = build_fingerprint(&embedded);
    let local_base = local_data_base();
    let local_writable =
        probe_dir_creatable_writable(&local_base.join("TuXuArchive").join("runtime"));
    let exe_writable = probe_writable(&exe_dir);
    let (dest, fallback, dest_mode) = choose_dest_for(
        &exe_dir,
        &local_base,
        &std::env::temp_dir(),
        &fingerprint,
        local_writable,
        exe_writable,
    );
    let action = sync_payload(&dest, ZIP)?;
    let disclosed = if fallback {
        dest.display().to_string()
    } else {
        String::new()
    };
    let record = format!(
        "action={action}\napp_dir={}\nfallback={dest_mode}\ndisclosed={disclosed}\nfingerprint={fingerprint}\nnote=no runtime download\n",
        dest.display(),
    );
    write_line(&record);
    let _ = fs::write(dest.join("launcher-last.log"), &record);
    if fallback {
        tell(&format!(
            "默认用户运行时目录不可写，已降级到此位置运行：\n{disclosed}"
        ));
    }
    if !webview_ok()
        && !dest
            .join("WebView2Fixed")
            .join("msedgewebview2.exe")
            .is_file()
    {
        let reason = "系统未安装 WebView2 运行时，且程序未携带固定版本运行时。本程序不会自动下载运行组件。请安装 Microsoft Edge WebView2 Runtime 后重试。";
        write_startup_failure(reason);
        return Err(reason.into());
    }
    let inner = dest.join("TuXuArchive.exe");
    if !inner.is_file() {
        return Err("解压后未找到 TuXuArchive.exe".into());
    }
    spawn_detached(&inner)?;
    Ok(())
}

fn build_fingerprint(manifest_text: &str) -> String {
    let normalized = normalize_text(manifest_text);
    let digest = sha256_bytes(normalized.as_bytes()).unwrap_or_else(|_| "default".into());
    digest.chars().take(12).collect()
}

fn local_data_base() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

fn runtime_dir(base: &Path, fingerprint: &str) -> PathBuf {
    base.join("TuXuArchive")
        .join("runtime")
        .join(fingerprint)
}

fn choose_dest_for(
    exe_dir: &Path,
    local_base: &Path,
    temp_base: &Path,
    fingerprint: &str,
    local_writable: bool,
    exe_writable: bool,
) -> (PathBuf, bool, &'static str) {
    if local_writable {
        (
            runtime_dir(local_base, fingerprint),
            false,
            "LOCALAPPDATA_RUNTIME",
        )
    } else if exe_writable {
        (
            exe_dir
                .join("TuXuArchive-runtime")
                .join(fingerprint),
            true,
            "EXE_DIR_FALLBACK",
        )
    } else {
        (runtime_dir(temp_base, fingerprint), true, "TEMP_FALLBACK")
    }
}

fn probe_dir_creatable_writable(dir: &Path) -> bool {
    if fs::create_dir_all(dir).is_err() {
        return false;
    }
    probe_writable(dir)
}

fn sync_payload(dest: &Path, zip_bytes: &[u8]) -> Result<String, String> {
    let _lock = acquire_extract_lock(dest)?;
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("无法创建程序目录：{e}"))?;
    }
    let embedded = normalize_text(&zip_text_from(zip_bytes, "manifest.sha256")?);
    let on_disk =
        normalize_text(&fs::read_to_string(dest.join("manifest.sha256")).unwrap_or_default());
    if on_disk == embedded && payload_matches(dest, &embedded) {
        return Ok("skipped".into());
    }
    let exe = dest.join("TuXuArchive.exe");
    if app_exe_busy(&exe) {
        return Err("主程序正在运行，拒绝替换当前程序目录。".into());
    }
    let staging = stage_verified(dest, zip_bytes)?;
    commit_staged(dest, &staging)?;
    Ok("extracted".into())
}

fn staging_path(dest: &Path) -> PathBuf {
    sibling(dest, "staging")
}

fn previous_path(dest: &Path) -> PathBuf {
    sibling(dest, "previous")
}

fn sibling(dest: &Path, suffix: &str) -> PathBuf {
    let name = dest
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("TuXuArchive");
    dest.parent()
        .unwrap_or(Path::new("."))
        .join(format!("{name}.{suffix}"))
}

fn stage_verified(dest: &Path, zip_bytes: &[u8]) -> Result<PathBuf, String> {
    let staging = staging_path(dest);
    if staging.exists() {
        fs::remove_dir_all(&staging).map_err(|e| format!("无法清掉上次未提交的暂存目录：{e}"))?;
    }
    fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    if let Err(err) = extract_zip_to(zip_bytes, &staging) {
        let _ = fs::remove_dir_all(&staging);
        return Err(err);
    }
    let manifest = normalize_text(&zip_text_from(zip_bytes, "manifest.sha256")?);
    if !payload_matches(&staging, &manifest) {
        let _ = fs::remove_dir_all(&staging);
        return Err("暂存目录的哈希与清单不一致，已放弃本次替换。".into());
    }
    Ok(staging)
}

fn commit_staged(dest: &Path, staging: &Path) -> Result<(), String> {
    let previous = previous_path(dest);
    if previous.exists() {
        fs::remove_dir_all(&previous).map_err(|e| format!("无法清掉旧的回滚目录：{e}"))?;
    }
    let had_dest = dest.exists();
    if had_dest {
        fs::rename(dest, &previous).map_err(|e| format!("无法腾出程序目录：{e}"))?;
    }
    if let Err(err) = fs::rename(staging, dest) {
        if had_dest && !dest.exists() {
            let _ = fs::rename(&previous, dest);
        }
        return Err(format!("提交新程序目录失败，已尝试恢复上一版：{err}"));
    }
    if previous.exists() {
        let _ = fs::remove_dir_all(&previous);
    }
    Ok(())
}

struct ExtractLock {
    path: PathBuf,
}

impl Drop for ExtractLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn lock_path(dest: &Path) -> PathBuf {
    let name = dest
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("TuXuArchive");
    dest.parent()
        .unwrap_or(Path::new("."))
        .join(format!(".{name}.extract.lock"))
}

fn acquire_extract_lock(dest: &Path) -> Result<ExtractLock, String> {
    let path = lock_path(dest);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    match try_create_lock(&path) {
        Ok(lock) => Ok(lock),
        Err(LockCreate::Busy) => Err("已有一次解压正在进行，本次拒绝同时解压。".into()),
        Err(LockCreate::Dead) => {
            let _ = fs::remove_file(&path);
            try_create_lock(&path).map_err(|_| "无法取得解压锁。".to_string())
        }
        Err(LockCreate::Io(message)) => Err(message),
    }
}

enum LockCreate {
    Busy,
    Dead,
    Io(String),
}

fn try_create_lock(path: &Path) -> Result<ExtractLock, LockCreate> {
    let pid_text = format!("{}\n", std::process::id());
    let temp = path.with_file_name(format!(
        ".tuxu.lock.tmp.{}.{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let linked = (|| -> std::io::Result<()> {
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        f.write_all(pid_text.as_bytes())?;
        let _ = f.sync_all();
        drop(f);
        fs::hard_link(&temp, path)
    })();
    let _ = fs::remove_file(&temp);

    match linked {
        Ok(()) => Ok(ExtractLock {
            path: path.to_path_buf(),
        }),
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => check_existing_lock(path),
        Err(_) => match OpenOptions::new().write(true).create_new(true).open(path) {
            Ok(mut file) => {
                file.write_all(pid_text.as_bytes())
                    .map_err(|e| LockCreate::Io(e.to_string()))?;
                Ok(ExtractLock {
                    path: path.to_path_buf(),
                })
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                check_existing_lock(path)
            }
            Err(err) => Err(LockCreate::Io(err.to_string())),
        },
    }
}

fn check_existing_lock(path: &Path) -> Result<ExtractLock, LockCreate> {
    for attempt in 0..4 {
        let raw = fs::read_to_string(path).unwrap_or_default();
        if !raw.trim().is_empty() {
            let pid = raw.trim().parse::<u32>().unwrap_or(0);
            return if pid_alive(pid) {
                Err(LockCreate::Busy)
            } else {
                Err(LockCreate::Dead)
            };
        }
        if attempt < 3 {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
    Err(LockCreate::Dead)
}

fn extract_zip_to(bytes: &[u8], dest: &Path) -> Result<(), String> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).map_err(|e| e.to_string())?;
        let name = file.name().map_err(|e| e.to_string())?.replace('\\', "/");
        if !matches!(
            name.as_str(),
            "TuXuArchive.exe" | "manifest.sha256" | "README.txt"
        ) || name.contains("..")
            || name.starts_with('/')
            || name.starts_with('\\')
        {
            return Err(format!("压缩包路径非法：{name}"));
        }
        let out = dest.join(&name);
        if !out.starts_with(dest) {
            return Err(format!("压缩包路径越界：{name}"));
        }
        if file.is_dir() {
            fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut data = Vec::new();
        file.read_to_end(&mut data).map_err(|e| e.to_string())?;
        fs::write(&out, data).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn normalize_text(text: &str) -> String {
    text.replace("\r\n", "\n").trim().to_string()
}

fn zip_text_from(bytes: &[u8], name: &str) -> Result<String, String> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let mut file = archive
        .by_name(name)
        .map_err(|e| format!("压缩包缺少 {name}：{e}"))?;
    let mut text = String::new();
    file.read_to_string(&mut text).map_err(|e| e.to_string())?;
    Ok(text.replace("\r\n", "\n"))
}

fn payload_matches(dest: &Path, manifest: &str) -> bool {
    for line in manifest.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(hash) = parts.next() else { continue };
        let Some(rel) = parts.next() else {
            return false;
        };
        if rel == "manifest.sha256" {
            continue;
        }
        match sha256_file(&dest.join(rel)) {
            Ok(got) if got.eq_ignore_ascii_case(hash) => {}
            _ => return false,
        }
    }
    dest.join("TuXuArchive.exe").is_file()
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    sha256_bytes(&bytes)
}

fn sha256_bytes(bytes: &[u8]) -> Result<String, String> {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(hex::encode(hasher.finalize()))
}

fn probe_writable(dir: &Path) -> bool {
    let name = format!(
        ".tuxu-write-probe-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let probe = dir.join(name);
    match fs::File::create(&probe) {
        Ok(mut file) => {
            let ok = file.write_all(b"ok").is_ok();
            drop(file);
            let _ = fs::remove_file(&probe);
            ok
        }
        Err(_) => false,
    }
}

fn data_logs() -> PathBuf {
    if let Some(dir) = std::env::var_os("TUXU_DATA_DIR") {
        return PathBuf::from(dir).join("logs");
    }
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("TuXuArchive").join("data").join("logs")
}

fn write_line(text: &str) {
    let dir = data_logs();
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("launcher.log");
    let mut previous = fs::read_to_string(&path).unwrap_or_default();
    if !previous.is_empty() {
        previous.push_str("\n----\n");
    }
    previous.push_str(text);
    const MAX_LOG_BYTES: usize = 128 * 1024;
    if previous.len() > MAX_LOG_BYTES {
        let mut cut = previous.len() - MAX_LOG_BYTES;
        while cut < previous.len() && !previous.is_char_boundary(cut) {
            cut += 1;
        }
        previous = previous[cut..].to_string();
    }
    let _ = fs::write(path, previous);
    println!("{text}");
}

fn write_startup_failure(reason: &str) {
    let dir = data_logs();
    let _ = fs::create_dir_all(&dir);
    let text = format!(
        "TuXu Archive self-check\n\
         startup=failed\n\
         webview2=missing\n\
         webview2_version=-\n\
         fixed_runtime=absent\n\
         database_open=no\n\
         reason={reason}\n\
         note=no runtime download\n"
    );
    let _ = fs::write(dir.join("startup-last.log"), &text);
}

fn webview_ok() -> bool {
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
        let Ok(key) = winreg::RegKey::predef(hive).open_subkey(path) else {
            continue;
        };
        let Ok(pv) = key.get_value::<String, _>("pv") else {
            continue;
        };
        if !pv.is_empty() && pv != "0.0.0.0" {
            return true;
        }
    }
    false
}

fn spawn_detached(exe: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x00000008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
    Command::new(exe)
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
        .spawn()
        .map_err(|e| format!("无法启动主程序：{e}"))?;
    Ok(())
}

fn tell(text: &str) {
    if std::env::var("TUXU_NO_UI").ok().as_deref() == Some("1") {
        return;
    }
    let mut body: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let mut title: Vec<u16> = "图序归档"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            body.as_mut_ptr(),
            title.as_mut_ptr(),
            0x00000040,
        );
    }
}

fn app_exe_busy(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        let handle = CreateFileW(
            wide.as_ptr(),
            0x40000000,
            0,
            std::ptr::null_mut(),
            3,
            0x80,
            std::ptr::null_mut(),
        );
        if handle.is_null() || handle == (-1isize as *mut std::ffi::c_void) {
            return GetLastError() == 32;
        }
        CloseHandle(handle);
    }
    false
}

fn pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    if pid == std::process::id() {
        return true;
    }
    unsafe {
        let handle = OpenProcess(0x1000, 0, pid);
        if handle.is_null() {
            return GetLastError() == 5;
        }
        let mut exit_code: u32 = 0;
        let ok = GetExitCodeProcess(handle, &mut exit_code);
        CloseHandle(handle);
        ok != 0 && exit_code == 259
    }
}

extern "system" {
    fn MessageBoxW(
        hwnd: *mut std::ffi::c_void,
        text: *const u16,
        caption: *const u16,
        kind: u32,
    ) -> i32;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: *mut std::ffi::c_void,
        disposition: u32,
        flags: u32,
        template: *mut std::ffi::c_void,
    ) -> *mut std::ffi::c_void;
    fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
    fn GetLastError() -> u32;
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut std::ffi::c_void;
    fn GetExitCodeProcess(handle: *mut std::ffi::c_void, exit_code: *mut u32) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_case(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tuxu-launcher-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample_zip(exe_bytes: &[u8]) -> Vec<u8> {
        let hash = sha256_bytes(exe_bytes).unwrap();
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut cursor);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            writer.start_file("TuXuArchive.exe", opts).unwrap();
            writer.write_all(exe_bytes).unwrap();
            writer.start_file("manifest.sha256", opts).unwrap();
            writer
                .write_all(format!("{hash}  TuXuArchive.exe\n").as_bytes())
                .unwrap();
            writer.finish().unwrap();
        }
        cursor.into_inner()
    }

    fn mismatched_zip(exe_bytes: &[u8]) -> Vec<u8> {
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut cursor);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            writer.start_file("TuXuArchive.exe", opts).unwrap();
            writer.write_all(exe_bytes).unwrap();
            writer.start_file("manifest.sha256", opts).unwrap();
            writer
                .write_all(b"0000000000000000000000000000000000000000000000000000000000000000  TuXuArchive.exe\n")
                .unwrap();
            writer.finish().unwrap();
        }
        cursor.into_inner()
    }

    #[test]
    fn payload_rejects_traversal_absolute_and_unexpected_entries() {
        for name in [
            "../outside.exe",
            "C:/outside.exe",
            "dir/extra.txt",
            "readme:stream",
        ] {
            let root = temp_case("path");
            let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
            writer
                .start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(b"bad").unwrap();
            let bytes = writer.finish().unwrap().into_inner();
            assert!(extract_zip_to(&bytes, &root).is_err(), "{name}");
            assert!(fs::read_dir(&root).unwrap().next().is_none());
            let _ = fs::remove_dir_all(root);
        }
    }

    #[test]
    fn default_extracts_to_localappdata_runtime_and_handles_restricted_fallbacks() {
        let exe_dir = Path::new(r"D:\PortableApps");
        let local_base = Path::new(r"C:\SampleLocalAppData\Local");
        let temp_base = Path::new(r"C:\Temp");
        let fp = build_fingerprint("abc123  TuXuArchive.exe\n");
        assert_eq!(fp.len(), 12);

        let (dest, fallback, mode) =
            choose_dest_for(exe_dir, local_base, temp_base, &fp, true, true);
        assert!(!fallback);
        assert_eq!(mode, "LOCALAPPDATA_RUNTIME");
        assert_eq!(
            dest,
            local_base.join("TuXuArchive").join("runtime").join(&fp)
        );

        let (fallback_exe, fb1, mode1) =
            choose_dest_for(exe_dir, local_base, temp_base, &fp, false, true);
        assert!(fb1);
        assert_eq!(mode1, "EXE_DIR_FALLBACK");
        assert_eq!(fallback_exe, exe_dir.join("TuXuArchive-runtime").join(&fp));

        let (fallback_tmp, fb2, mode2) =
            choose_dest_for(exe_dir, local_base, temp_base, &fp, false, false);
        assert!(fb2);
        assert_eq!(mode2, "TEMP_FALLBACK");
        assert_eq!(
            fallback_tmp,
            temp_base.join("TuXuArchive").join("runtime").join(&fp)
        );

        let fp2 = build_fingerprint("def456  TuXuArchive.exe\n");
        assert_ne!(fp, fp2);
    }

    #[test]
    fn hash_failure_keeps_the_previous_tree() {
        let root = temp_case("hash");
        let dest = root.join("TuXuArchive");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("TuXuArchive.exe"), b"old-complete").unwrap();
        fs::write(dest.join("manifest.sha256"), "stale\n").unwrap();
        let err = sync_payload(&dest, &mismatched_zip(b"new-bytes")).unwrap_err();
        assert!(err.contains("哈希"));
        assert_eq!(
            fs::read(dest.join("TuXuArchive.exe")).unwrap(),
            b"old-complete"
        );
        assert!(!staging_path(&dest).exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn crash_before_commit_leaves_previous_tree() {
        let root = temp_case("crash");
        let dest = root.join("TuXuArchive");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("TuXuArchive.exe"), b"old-complete").unwrap();
        let zip = sample_zip(b"new-bytes");
        let staging = stage_verified(&dest, &zip).unwrap();
        assert_eq!(
            fs::read(dest.join("TuXuArchive.exe")).unwrap(),
            b"old-complete"
        );
        assert_eq!(
            fs::read(staging.join("TuXuArchive.exe")).unwrap(),
            b"new-bytes"
        );
        assert!(!previous_path(&dest).exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn commit_replaces_tree_and_removes_previous() {
        let root = temp_case("commit");
        let dest = root.join("TuXuArchive");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("TuXuArchive.exe"), b"old-complete").unwrap();
        let zip = sample_zip(b"new-bytes");
        let action = sync_payload(&dest, &zip).unwrap();
        assert_eq!(action, "extracted");
        assert_eq!(
            fs::read(dest.join("TuXuArchive.exe")).unwrap(),
            b"new-bytes"
        );
        assert!(!staging_path(&dest).exists());
        assert!(!previous_path(&dest).exists());
        let again = sync_payload(&dest, &zip).unwrap();
        assert_eq!(again, "skipped");
        assert_eq!(
            fs::read(dest.join("TuXuArchive.exe")).unwrap(),
            b"new-bytes"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn second_extract_is_refused_and_dead_lock_is_stolen() {
        let root = temp_case("lock");
        let dest = root.join("TuXuArchive");
        fs::create_dir_all(&dest).unwrap();
        let held = acquire_extract_lock(&dest).unwrap();
        let err = match acquire_extract_lock(&dest) {
            Err(message) => message,
            Ok(_) => panic!("第二把解压锁不应成功"),
        };
        assert!(err.contains("拒绝同时解压"));
        drop(held);
        fs::write(lock_path(&dest), "0\n").unwrap();
        let stolen = acquire_extract_lock(&dest).unwrap();
        drop(stolen);
        assert!(!lock_path(&dest).exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn running_app_is_not_replaced() {
        let root = temp_case("busy");
        let dest = root.join("TuXuArchive");
        fs::create_dir_all(&dest).unwrap();
        let exe = dest.join("TuXuArchive.exe");
        fs::write(&exe, b"old-complete").unwrap();
        fs::write(dest.join("manifest.sha256"), "stale\n").unwrap();
        let wide: Vec<u16> = std::os::windows::ffi::OsStrExt::encode_wide(exe.as_os_str())
            .chain(std::iter::once(0))
            .collect();
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                0x40000000,
                0,
                std::ptr::null_mut(),
                3,
                0x80,
                std::ptr::null_mut(),
            )
        };
        assert!(!handle.is_null() && handle != (-1isize as *mut std::ffi::c_void));
        let err = sync_payload(&dest, &sample_zip(b"new-bytes")).unwrap_err();
        assert!(err.contains("拒绝替换"));
        unsafe {
            CloseHandle(handle);
        }
        assert_eq!(fs::read(&exe).unwrap(), b"old-complete");
        let _ = fs::remove_dir_all(&root);
    }
}
