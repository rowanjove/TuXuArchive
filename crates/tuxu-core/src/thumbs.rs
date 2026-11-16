use crate::codec::decode_preview_image;
use crate::model::{Photo, ThumbWindow};
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, SystemTime};

pub const THUMB_CACHE_CAP_BYTES: u64 = 256 * 1024 * 1024;
const LRU_TOUCH_MIN_AGE: Duration = Duration::from_secs(60);
static CACHE_WRITES: AtomicU32 = AtomicU32::new(0);

pub fn retained_payload_window(
    total: usize,
    scroll_index: usize,
    visible: usize,
    neighbor: usize,
) -> ThumbWindow {
    if total == 0 || visible == 0 {
        return ThumbWindow { start: 0, end: 0 };
    }
    let visible = visible.min(total);
    let neighbor = neighbor.min(64);
    let scroll = scroll_index.min(total - 1);
    let start = scroll.saturating_sub(neighbor);
    let end = scroll
        .saturating_add(visible)
        .saturating_add(neighbor)
        .min(total);
    ThumbWindow { start, end }
}

pub fn preview_archive_label(preview_state: &str) -> &'static str {
    if preview_state == "available" {
        "可预览"
    } else {
        "可归档／预览可能不可用"
    }
}

pub fn evict_cache(cache_root: &Path, cap: u64) -> u32 {
    let Ok(rd) = fs::read_dir(cache_root) else {
        return 0;
    };
    let mut files = Vec::new();
    for entry in rd.filter_map(|item| item.ok()) {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let len = entry.metadata().map(|meta| meta.len()).unwrap_or(0);
        let modified = entry.metadata().ok().and_then(|meta| meta.modified().ok());
        files.push((modified, len, path));
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut total: u64 = files.iter().map(|file| file.1).sum();
    let mut removed = 0u32;
    for (_, len, path) in files {
        if total <= cap {
            break;
        }
        if fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(len);
            removed += 1;
        }
    }
    removed
}

pub enum ThumbResult {
    Jpeg(Vec<u8>),
    Unavailable { reason: String },
}

pub fn render_thumbnail_standalone(
    cache_root: &Path,
    photo: &Photo,
    max_edge: u32,
) -> crate::app::ResultThumb {
    match render(cache_root, photo, max_edge) {
        ThumbResult::Jpeg(data) => crate::app::ResultThumb::Jpeg(data),
        ThumbResult::Unavailable { reason } => crate::app::ResultThumb::Unavailable { reason },
    }
}

pub fn render(cache_root: &Path, photo: &Photo, max_edge: u32) -> ThumbResult {
    let max_edge = max_edge.clamp(64, 2048);
    if photo.corrupt || photo.preview_state == "unavailable" {
        return ThumbResult::Unavailable {
            reason: if photo.corrupt {
                "预览不可用：文件已损坏或不是可解码的图片，仍可审查并选择是否排除".into()
            } else {
                "预览不可用（系统可能缺少对应扩展或内嵌缩略图）。若选择归档，将按原始字节复制，不转码。".into()
            },
        };
    }
    let cache = cache_root.join(cache_file_name(photo, max_edge));
    if let Ok(bytes) = fs::read(&cache) {
        if !bytes.is_empty() {
            touch_cache_lru_throttled(&cache);
            return ThumbResult::Jpeg(bytes);
        }
    }
    let source = Path::new(&photo.source_path);
    let img = match decode_preview_image(source, max_edge.max(320)) {
        Ok(img) => img,
        Err(reason) => {
            return ThumbResult::Unavailable {
                reason: format!("预览不可用：{reason}"),
            };
        }
    };
    let thumb = if img.width() > max_edge || img.height() > max_edge {
        img.thumbnail(max_edge, max_edge)
    } else {
        img
    };
    let mut bytes = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut bytes);
    if thumb
        .write_to(&mut cursor, image::ImageFormat::Jpeg)
        .is_err()
    {
        return ThumbResult::Unavailable {
            reason: "预览不可用：无法编码缩略图".into(),
        };
    }
    if let Some(parent) = cache.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let tmp_name = format!(
        ".{}.tmp-{}",
        cache
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("thumb"),
        CACHE_WRITES.fetch_add(1, Ordering::Relaxed)
    );
    let tmp_path = cache.with_file_name(tmp_name);
    if fs::write(&tmp_path, &bytes).is_ok() && fs::rename(&tmp_path, &cache).is_err() {
        let _ = fs::remove_file(&tmp_path);
        let _ = fs::write(&cache, &bytes);
    }
    if CACHE_WRITES.load(Ordering::Relaxed).is_multiple_of(24) {
        let _ = evict_cache(cache_root, THUMB_CACHE_CAP_BYTES);
    }
    ThumbResult::Jpeg(bytes)
}

fn touch_cache_lru_throttled(path: &Path) {
    let Ok(meta) = fs::metadata(path) else { return };
    let Ok(modified) = meta.modified() else {
        return;
    };
    let Ok(age) = SystemTime::now().duration_since(modified) else {
        return;
    };
    if age < LRU_TOUCH_MIN_AGE {
        return;
    }
    if let Ok(file) = fs::OpenOptions::new().write(true).open(path) {
        let _ = file.set_modified(SystemTime::now());
    }
}

pub fn cache_file_name(photo: &Photo, max_edge: u32) -> String {
    use sha2::{Digest, Sha256};
    let path = Path::new(&photo.source_path);
    let canon =
        crate::util::canonicalize_any(path).unwrap_or_else(|_| crate::util::plain_path(path));
    let norm = canon.to_string_lossy().replace('/', "\\").to_lowercase();
    let mut hasher = Sha256::new();
    hasher.update(norm.as_bytes());
    hasher.update(b"|");
    hasher.update(photo.size.to_le_bytes());
    hasher.update(b"|");
    hasher.update(photo.mtime_ns.to_le_bytes());
    let digest = hex::encode(hasher.finalize());
    format!("th-{}-{max_edge}.jpg", &digest[..32])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Photo, PhotoQuality};
    use std::time::UNIX_EPOCH;

    #[test]
    fn stable_cache_key_reuses_thumbnails_across_rescans() {
        let dir = std::env::temp_dir().join(format!(
            "tuxu-thumb-c4-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let cache = dir.join("cache");
        fs::create_dir_all(&dir).unwrap();
        let src = dir.join("sample.jpg");
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            120,
            80,
            image::Rgb([100, 120, 140]),
        ));
        img.write_to(
            &mut fs::File::create(&src).unwrap(),
            image::ImageFormat::Jpeg,
        )
        .unwrap();
        let meta = fs::metadata(&src).unwrap();
        let mtime_ns = crate::util::mtime_ns(&meta);

        let mut photo = Photo {
            id: "pho-scan-1".into(),
            file_name: "sample.jpg".into(),
            source_path: src.display().to_string(),
            extension: ".jpg".into(),
            size: meta.len(),
            mtime_ns,
            capture_text: None,
            capture_sort_key: None,
            exif_offset: None,
            sort_time_text: None,
            time_uncertain: false,
            time_source: "exif_original".into(),
            time_note: String::new(),
            tie_break: String::new(),
            tie_break_is_capture_order: false,
            equal_timestamp: false,
            sort_index: 0,
            original_scan_index: 0,
            group_id: None,
            tag_id: None,
            custom_name: None,
            confirmed: false,
            needs_review: false,
            excluded: false,
            preview_state: "available".into(),
            corrupt: false,
            quality: PhotoQuality {
                status: "unchecked".into(),
                width: Some(120),
                height: Some(80),
                issues: Vec::new(),
                summary: String::new(),
                luma_mean: None,
                sharpness: None,
                pixel_checked: false,
            },
        };
        let key1 = cache_file_name(&photo, 220);
        assert!(matches!(render(&cache, &photo, 220), ThumbResult::Jpeg(_)));
        assert!(cache.join(&key1).is_file());

        // Rescan creates new photo.id while path/size/mtime stay unchanged -> same key
        photo.id = "pho-scan-2".into();
        let key2 = cache_file_name(&photo, 220);
        assert_eq!(key1, key2);

        // File content modified -> new cache key invalidates old thumbnail
        photo.mtime_ns += 1_000_000;
        let key3 = cache_file_name(&photo, 220);
        assert_ne!(key1, key3);

        let _ = fs::remove_dir_all(&dir);
    }
}
