use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static COUNTER: AtomicU64 = AtomicU64::new(1);

pub fn new_id(prefix: &str) -> String {
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let c = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{t:x}-{c:x}")
}

pub fn now_iso() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs as i64, 0)
        .map(|d| d.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".into())
}

pub fn mtime_ns(meta: &Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

pub fn metadata_any(path: &Path) -> std::io::Result<Metadata> {
    std::fs::metadata(win_extended(path)).or_else(|_| std::fs::metadata(path))
}

pub fn exists_any(path: &Path) -> bool {
    std::fs::symlink_metadata(win_extended(path)).is_ok() || path.exists()
}

pub fn is_file_any(path: &Path) -> bool {
    metadata_any(path).is_ok_and(|m| m.is_file())
}

pub fn is_dir_any(path: &Path) -> bool {
    metadata_any(path).is_ok_and(|m| m.is_dir())
}

pub fn open_any(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::File::open(win_extended(path)).or_else(|_| std::fs::File::open(path))
}

/// Windows share semantics reject existing writers and block new writers/deletion
/// until this handle closes. Never fall back to an unprotected open.
pub fn open_stable_source(path: &Path) -> std::io::Result<std::fs::File> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(win_extended(path))
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "稳定源读取仅支持 Windows",
        ))
    }
}

/// Volume serial + 128-bit filesystem ID survive rename. Creation time is
/// deliberately excluded: NTFS tunneling may restore an old target's timestamp.
pub fn file_identity(file: &std::fs::File) -> std::io::Result<String> {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            FileIdInfo, GetFileInformationByHandleEx, FILE_ID_INFO,
        };
        let mut info: FILE_ID_INFO = unsafe { std::mem::zeroed() };
        if unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle() as _,
                FileIdInfo,
                &mut info as *mut _ as _,
                std::mem::size_of_val(&info) as u32,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        if info.FileId.Identifier.iter().all(|b| *b == 0) {
            return Err(std::io::Error::other("文件系统不能提供可靠文件身份"));
        }
        Ok(format!(
            "fid2:{:x}:{}",
            info.VolumeSerialNumber,
            hex::encode(info.FileId.Identifier)
        ))
    }
    #[cfg(not(windows))]
    {
        let _ = file;
        Err(std::io::Error::other("不支持文件身份验证"))
    }
}

pub fn write_any(path: &Path, data: &[u8]) -> std::io::Result<()> {
    std::fs::write(win_extended(path), data).or_else(|_| std::fs::write(path, data))
}

pub fn create_dir_any(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir(win_extended(path)).or_else(|_| std::fs::create_dir(path))
}

pub fn read_dir_any(path: &Path) -> std::io::Result<std::fs::ReadDir> {
    std::fs::read_dir(win_extended(path)).or_else(|_| std::fs::read_dir(path))
}

pub fn remove_file_any(path: &Path) -> std::io::Result<()> {
    std::fs::remove_file(win_extended(path)).or_else(|_| std::fs::remove_file(path))
}

pub fn remove_dir_any(path: &Path) -> std::io::Result<()> {
    std::fs::remove_dir(win_extended(path)).or_else(|_| std::fs::remove_dir(path))
}

pub fn canonicalize_any(path: &Path) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(win_extended(path))
        .or_else(|_| std::fs::canonicalize(path))
        .map(|canon| plain_path(&canon))
}

pub fn win_extended(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if s.starts_with(r"\\?\") || s.starts_with(r"\\.\") {
        return path.to_path_buf();
    }
    if let Some(rest) = s.strip_prefix(r"\\") {
        let normalized = rest.replace('/', "\\");
        return PathBuf::from(format!(r"\\?\UNC\{normalized}"));
    }
    let norm = s.replace('/', "\\");
    let bytes = norm.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return PathBuf::from(format!(r"\\?\{norm}"));
    }
    path.to_path_buf()
}

/// Drop the `\\?\` prefix so stored paths stay readable and comparable.
pub fn plain_path(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = text.strip_prefix(r"\\?\") {
        return PathBuf::from(rest.to_string());
    }
    path.to_path_buf()
}

pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut file = open_any(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

#[allow(dead_code)]
pub fn path_contains(parent: &Path, child: &Path) -> bool {
    let p = normalize_for_compare(parent);
    let c = normalize_for_compare(child);
    c.starts_with(&p) && c != p
}

#[allow(dead_code)]
fn normalize_for_compare(path: &Path) -> PathBuf {
    let canon = canonicalize_any(path).unwrap_or_else(|_| plain_path(path));
    PathBuf::from(canon.to_string_lossy().to_lowercase())
}

#[allow(dead_code)]
pub fn trees_conflict(source: &Path, output: &Path) -> bool {
    if normalize_for_compare(source) == normalize_for_compare(output) {
        return true;
    }
    path_contains(source, output) || path_contains(output, source)
}

pub fn free_bytes(path: &Path) -> Option<u64> {
    let probe = if exists_any(path) {
        path.to_path_buf()
    } else {
        path.parent()?.to_path_buf()
    };
    free_bytes_win(&win_extended(&probe)).or_else(|| free_bytes_win(&probe))
}

#[cfg(windows)]
fn free_bytes_win(path: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut free = 0u64;
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut free,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        None
    } else {
        Some(free)
    }
}

#[cfg(not(windows))]
fn free_bytes_win(_path: &Path) -> Option<u64> {
    None
}

#[cfg(windows)]
fn decode_win_codepage(raw: &[u8], code_page: u32, flags: u32) -> Option<String> {
    use windows_sys::Win32::Globalization::MultiByteToWideChar;
    if raw.is_empty() {
        return Some(String::new());
    }
    let byte_len = i32::try_from(raw.len()).ok()?;
    let wide_len = unsafe {
        MultiByteToWideChar(
            code_page,
            flags,
            raw.as_ptr(),
            byte_len,
            std::ptr::null_mut(),
            0,
        )
    };
    if wide_len <= 0 {
        return None;
    }
    let mut wide = vec![0u16; wide_len as usize];
    let written = unsafe {
        MultiByteToWideChar(
            code_page,
            flags,
            raw.as_ptr(),
            byte_len,
            wide.as_mut_ptr(),
            wide_len,
        )
    };
    if written <= 0 {
        return None;
    }
    wide.truncate(written as usize);
    String::from_utf16(&wide).ok()
}

/// Decodes UTF-8 (with or without BOM), UTF-16, or legacy Windows GB18030/GBK text files.
pub fn decode_text_bytes(raw: &[u8]) -> String {
    if let Some(stripped) = raw.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        if let Ok(s) = std::str::from_utf8(stripped) {
            return s.to_owned();
        }
    }
    if let Some(stripped) = raw.strip_prefix(&[0xFF, 0xFE]) {
        let units: Vec<u16> = stripped
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        if let Ok(s) = String::from_utf16(&units) {
            return s;
        }
    }
    if let Some(stripped) = raw.strip_prefix(&[0xFE, 0xFF]) {
        let units: Vec<u16> = stripped
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        if let Ok(s) = String::from_utf16(&units) {
            return s;
        }
    }
    if let Ok(s) = std::str::from_utf8(raw) {
        return s.to_owned();
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Globalization::MB_ERR_INVALID_CHARS;
        for (cp, flags) in [(54936u32, MB_ERR_INVALID_CHARS), (936u32, 0)] {
            if let Some(decoded) = decode_win_codepage(raw, cp, flags) {
                return decoded;
            }
        }
    }
    String::from_utf8_lossy(raw).into_owned()
}

/// Own a newly created temp through write, validation and handle-based rename.
#[cfg(windows)]
pub fn create_owned_temp(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .access_mode(0xC0010000)
        .share_mode(1)
        .open(win_extended(path))
}

#[cfg(windows)]
pub fn open_owned_for_delete(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .access_mode(0x80010000)
        .share_mode(1)
        .open(win_extended(path))
}

#[cfg(windows)]
pub fn delete_owned_handle(file: &std::fs::File) -> std::io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileDispositionInfo, SetFileInformationByHandle, FILE_DISPOSITION_INFO,
    };
    let info = FILE_DISPOSITION_INFO { DeleteFile: 1 };
    let ok = unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle() as _,
            FileDispositionInfo,
            &info as *const _ as _,
            std::mem::size_of_val(&info) as u32,
        )
    };
    if ok == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(windows)]
pub fn commit_owned_handle(file: &std::fs::File, target: &Path) -> std::io::Result<()> {
    use std::os::windows::{ffi::OsStrExt, io::AsRawHandle};
    use windows_sys::Win32::Storage::FileSystem::{
        FileRenameInfo, SetFileInformationByHandle, FILE_RENAME_INFO,
    };
    let target = win_extended(target);
    let wide: Vec<u16> = target.as_os_str().encode_wide().collect();
    let offset = std::mem::offset_of!(FILE_RENAME_INFO, FileName);
    let size = offset + (wide.len() + 1) * 2;
    let mut storage = vec![0u64; size.div_ceil(8)];
    let info = storage.as_mut_ptr() as *mut FILE_RENAME_INFO;
    unsafe {
        (*info).Anonymous.ReplaceIfExists = 0;
        (*info).RootDirectory = std::ptr::null_mut();
        (*info).FileNameLength = (wide.len() * 2) as u32;
        std::ptr::copy_nonoverlapping(wide.as_ptr(), (*info).FileName.as_mut_ptr(), wide.len());
        if SetFileInformationByHandle(
            file.as_raw_handle() as _,
            FileRenameInfo,
            info as _,
            size as u32,
        ) == 0
        {
            return Err(std::io::Error::last_os_error());
        }
    }
    file.sync_all()
}

#[cfg(test)]
mod delivery_io_tests {
    use super::*;
    use std::io::Write;
    #[test]
    #[ignore = "explicit filesystem durability timing probe"]
    fn durable_handle_commit_timing() {
        let root = std::env::temp_dir().join(new_id("tuxu-io-stage"));
        std::fs::create_dir_all(&root).unwrap();
        let bytes = vec![42; 1_310_720];
        let mut sync = 0u128;
        let mut commit = 0u128;
        for i in 0..100 {
            let mut file = create_owned_temp(&root.join(format!("{i}.part"))).unwrap();
            file.write_all(&bytes).unwrap();
            let now = std::time::Instant::now();
            file.sync_all().unwrap();
            sync += now.elapsed().as_micros();
            let now = std::time::Instant::now();
            commit_owned_handle(&file, &root.join(format!("{i}.jpg"))).unwrap();
            commit += now.elapsed().as_micros();
        }
        println!("DURABLE_IO_100: data_sync_us={sync} rename_and_sync_us={commit}");
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_text_bytes_handles_utf8_and_gbk() {
        assert_eq!(decode_text_bytes("封面精选\n".as_bytes()), "封面精选\n");
        let mut with_bom = vec![0xEF, 0xBB, 0xBF];
        with_bom.extend_from_slice("风景远景".as_bytes());
        assert_eq!(decode_text_bytes(&with_bom), "风景远景");
        #[cfg(windows)]
        {
            // GBK encoding of "图序归档" is CD BC D0 F2 B9 E9 B5 B5
            let gbk_bytes = [0xCDu8, 0xBC, 0xD0, 0xF2, 0xB9, 0xE9, 0xB5, 0xB5];
            assert_eq!(decode_text_bytes(&gbk_bytes), "图序归档");
        }
    }
}
