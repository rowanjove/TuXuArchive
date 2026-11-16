use crate::codec::{classify_extension, probe_photo, PhotoKind};
use crate::error::{err, Result};
use crate::model::{Photo, PhotoQuality, ScanControl, ScanTick};
use chrono::{
    DateTime, Datelike, FixedOffset, Local, NaiveDateTime, Offset, TimeZone, Timelike, Utc,
};
use std::fs::{self, File};
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use walkdir::WalkDir;

#[derive(Debug)]
pub struct ScanOutput {
    pub photos: Vec<Photo>,
    pub ignored_count: u32,
    pub ignored_reasons: std::collections::BTreeMap<String, u32>,
    pub cancelled: bool,
    /// Directories or entries that could not be listed. Those scans stay incomplete.
    pub read_errors: u32,
}

#[allow(dead_code)]
pub fn scan_tree(root: &Path, recursive: bool) -> Result<ScanOutput> {
    scan_tree_controlled(root, recursive, None, |_| ScanControl::Continue)
}

pub fn scan_tree_controlled(
    root: &Path,
    recursive: bool,
    max_photos: Option<u32>,
    mut on_tick: impl FnMut(&ScanTick) -> ScanControl,
) -> Result<ScanOutput> {
    let root_meta =
        crate::util::metadata_any(root).map_err(|_| err("E003", "源文件夹不存在或不是目录"))?;
    if !root_meta.is_dir() {
        return Err(err("E003", "源文件夹不存在或不是目录"));
    }
    let mut files = Vec::new();
    let mut ignored_reasons = std::collections::BTreeMap::new();
    let mut read_errors = 0u32;
    if recursive {
        let walk_root = crate::util::win_extended(root);
        for entry in WalkDir::new(walk_root).follow_links(false) {
            match entry {
                Ok(entry) => {
                    if entry.file_type().is_file() {
                        files.push(crate::util::plain_path(entry.path()));
                    }
                }
                Err(_) => note_unreadable(&mut ignored_reasons, &mut read_errors),
            }
        }
    } else {
        let rd = fs::read_dir(crate::util::win_extended(root))
            .or_else(|_| fs::read_dir(root))
            .map_err(|_| err("E003", "无法读取源文件夹"))?;
        for entry in rd {
            match entry {
                Ok(entry) => match entry.file_type() {
                    Ok(kind) if kind.is_file() => {
                        files.push(crate::util::plain_path(&entry.path()))
                    }
                    Ok(_) => {}
                    Err(_) => note_unreadable(&mut ignored_reasons, &mut read_errors),
                },
                Err(_) => note_unreadable(&mut ignored_reasons, &mut read_errors),
            }
        }
    }
    files.sort();

    let mut photos = Vec::new();
    let mut ignored_count = 0u32;
    let mut cancelled = false;
    let mut candidate_photos: Vec<(PathBuf, PhotoKind)> = Vec::new();
    for path in files {
        match classify(&path) {
            Class::Photo { kind } => candidate_photos.push((path, kind)),
            Class::Ignore(reason) => {
                ignored_count += 1;
                *ignored_reasons.entry(reason).or_insert(0) += 1;
            }
        }
    }

    let use_batch_parallel = candidate_photos.len() >= 24 && max_photos.is_none();
    if !use_batch_parallel {
        for (path, kind) in candidate_photos {
            let name = path
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            let tick = ScanTick {
                discovered: photos.len() as u32,
                ignored: ignored_count,
                current_file: name,
                cancelled: false,
            };
            if on_tick(&tick) == ScanControl::Cancel {
                cancelled = true;
                break;
            }
            if max_photos.is_some_and(|limit| photos.len() as u32 >= limit) {
                cancelled = true;
                break;
            }
            match read_photo(&path, kind) {
                Ok(photo) => photos.push(photo),
                Err(e) => photos.push(fallback_photo(&path, &e.message)),
            }
        }
    } else {
        let workers = std::thread::available_parallelism()
            .map(|n| n.get().clamp(2, 6))
            .unwrap_or(4);
        let chunk_size = workers * 4;
        for chunk in candidate_photos.chunks(chunk_size) {
            let first_name = chunk
                .first()
                .and_then(|(p, _)| p.file_name())
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            let tick = ScanTick {
                discovered: photos.len() as u32,
                ignored: ignored_count,
                current_file: first_name,
                cancelled: false,
            };
            if on_tick(&tick) == ScanControl::Cancel {
                cancelled = true;
                break;
            }
            let batch_out = scan_chunk_parallel(chunk, workers);
            photos.extend(batch_out);
        }
        if !cancelled {
            let tick = ScanTick {
                discovered: photos.len() as u32,
                ignored: ignored_count,
                current_file: String::new(),
                cancelled: false,
            };
            let _ = on_tick(&tick);
        }
    }
    sort_photos(&mut photos);
    for photo in &mut photos {
        photo.original_scan_index = photo.sort_index;
    }
    Ok(ScanOutput {
        photos,
        ignored_count,
        ignored_reasons,
        cancelled,
        read_errors,
    })
}

fn scan_chunk_parallel(chunk: &[(PathBuf, PhotoKind)], workers: usize) -> Vec<Photo> {
    if chunk.len() <= 1 || workers <= 1 {
        return chunk
            .iter()
            .map(|(path, kind)| match read_photo(path, *kind) {
                Ok(photo) => photo,
                Err(e) => fallback_photo(path, &e.message),
            })
            .collect();
    }
    let slice_len = chunk.len().div_ceil(workers);
    let mut results = Vec::with_capacity(chunk.len());
    std::thread::scope(|scope| {
        let handles: Vec<_> = chunk
            .chunks(slice_len)
            .map(|sub| {
                scope.spawn(move || {
                    sub.iter()
                        .map(|(path, kind)| match read_photo(path, *kind) {
                            Ok(photo) => photo,
                            Err(e) => fallback_photo(path, &e.message),
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        for handle in handles {
            if let Ok(part) = handle.join() {
                results.extend(part);
            }
        }
    });
    results
}

fn note_unreadable(reasons: &mut std::collections::BTreeMap<String, u32>, errors: &mut u32) {
    *errors += 1;
    *reasons.entry("unreadable".into()).or_insert(0) += 1;
}

fn open_source(path: &Path) -> std::io::Result<File> {
    File::open(crate::util::win_extended(path)).or_else(|_| File::open(path))
}

enum Class {
    Photo { kind: PhotoKind },
    Ignore(String),
}

fn classify(path: &Path) -> Class {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if let Some(kind) = classify_extension(&ext) {
        return Class::Photo { kind };
    }
    match ext.as_str() {
        "mp4" | "mov" | "avi" | "mkv" | "wmv" | "mts" | "m2ts" | "lrv" | "insv" => {
            Class::Ignore("video".into())
        }
        "log" | "txt" => Class::Ignore("log".into()),
        "xmp" | "aae" | "thm" | "srt" => Class::Ignore("sidecar".into()),
        _ => Class::Ignore("other".into()),
    }
}

fn read_photo(path: &Path, kind: PhotoKind) -> Result<Photo> {
    let meta = crate::util::metadata_any(path)
        .map_err(|e| err("E003", format!("{}：{e}", path.display())))?;
    let file_name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let extension = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let probe = probe_photo(path, kind);
    let corrupt = probe.corrupt;
    let preview_state = if corrupt || !probe.preview_available {
        "unavailable"
    } else {
        "available"
    };
    let times = read_times(path);
    let (capture_text, capture_sort_key, exif_offset, sort_time_text, time_source, time_note) =
        if let Some(t) = times {
            (
                Some(t.text.clone()),
                Some(t.sort_key),
                t.exif_offset,
                Some(t.text),
                t.source,
                t.note,
            )
        } else {
            let wall = filesystem_wall(meta.modified().ok());
            (
                Some(wall.0.clone()),
                Some(wall.1),
                None,
                Some(wall.0),
                "filesystem_fallback".to_string(),
                "无嵌入拍摄时间，已用文件修改时间。这不是相机原始时间。".into(),
            )
        };
    Ok(Photo {
        id: crate::util::new_id("photo"),
        source_path: path.to_string_lossy().to_string(),
        file_name,
        extension,
        size: meta.len(),
        mtime_ns: crate::util::mtime_ns(&meta),
        capture_text,
        capture_sort_key,
        exif_offset,
        sort_time_text,
        time_uncertain: false,
        time_source,
        time_note,
        equal_timestamp: false,
        tie_break: "path".into(),
        tie_break_is_capture_order: false,
        corrupt,
        preview_state: preview_state.into(),
        quality: probe.quality,
        sort_index: 0,
        original_scan_index: 0,
        group_id: None,
        tag_id: None,
        custom_name: None,
        confirmed: false,
        needs_review: false,
        excluded: false,
    })
}

fn fallback_photo(path: &Path, message: &str) -> Photo {
    let file_name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let extension = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    Photo {
        id: crate::util::new_id("photo"),
        source_path: path.to_string_lossy().to_string(),
        file_name,
        extension,
        size: crate::util::metadata_any(path)
            .map(|m| m.len())
            .unwrap_or(0),
        mtime_ns: crate::util::metadata_any(path)
            .map(|m| crate::util::mtime_ns(&m))
            .unwrap_or(0),
        capture_text: None,
        capture_sort_key: None,
        exif_offset: None,
        sort_time_text: None,
        time_uncertain: false,
        time_source: "filesystem_fallback".into(),
        time_note: format!("读取失败，已保留为可审查项：{message}"),
        equal_timestamp: false,
        tie_break: "path".into(),
        tie_break_is_capture_order: false,
        corrupt: true,
        preview_state: "unavailable".into(),
        quality: PhotoQuality {
            status: "bad".into(),
            width: None,
            height: None,
            issues: vec!["corrupt".into()],
            summary: format!("文件无法读取：{message}"),
            luma_mean: None,
            sharpness: None,
            pixel_checked: false,
        },
        sort_index: 0,
        original_scan_index: 0,
        group_id: None,
        tag_id: None,
        custom_name: None,
        confirmed: false,
        needs_review: true,
        excluded: false,
    }
}

struct ParsedTime {
    text: String,
    exif_offset: Option<String>,
    sort_key: i64,
    source: String,
    note: String,
}

fn read_times(path: &Path) -> Option<ParsedTime> {
    let file = open_source(path).ok()?;
    let mut reader = BufReader::new(file);
    let exif = exif::Reader::new().read_from_container(&mut reader).ok()?;
    if let Some(parsed) = field_time(&exif, exif::Tag::DateTimeOriginal, "exif_original") {
        return Some(parsed);
    }
    if let Some(parsed) = field_time(&exif, exif::Tag::DateTimeDigitized, "metadata_fallback") {
        return Some(parsed);
    }
    field_time(&exif, exif::Tag::DateTime, "metadata_fallback")
}

fn field_time(exif: &exif::Exif, tag: exif::Tag, source: &str) -> Option<ParsedTime> {
    let field = exif.get_field(tag, exif::In::PRIMARY)?;
    let raw = ascii_value(field)?;
    let raw = raw.trim().trim_matches('\0').trim();
    let text = raw.get(..19)?;
    let text = text.to_string();
    let naive = NaiveDateTime::parse_from_str(&text, "%Y:%m:%d %H:%M:%S").ok()?;
    let sub = subsec_ms(exif, tag);
    let offset = offset_for(exif, tag);
    let exif_offset = offset.map(format_offset);
    let note = if let Some(ref off_str) = exif_offset {
        let utc_str = FixedOffset::east_opt(offset.unwrap_or(0))
            .and_then(|o| o.from_local_datetime(&naive).single())
            .map(|dt| dt.naive_utc().format("%Y:%m:%d %H:%M:%S").to_string())
            .unwrap_or_else(|| text.clone());
        format!("存在时区（{off_str}），已换算为 UTC（{utc_str}）比较")
    } else {
        "EXIF 无时区，按原始墙上时间比较，未做 UTC 平移".to_string()
    };
    let mut note = note;
    if source != "exif_original" {
        note = format!("使用备用嵌入时间。{note}");
    }
    let sort_key = wall_key(naive, sub);
    Some(ParsedTime {
        text,
        exif_offset,
        sort_key,
        source: source.to_string(),
        note,
    })
}

fn ascii_value(field: &exif::Field) -> Option<String> {
    match &field.value {
        exif::Value::Ascii(chunks) => {
            let bytes = chunks.first()?;
            Some(String::from_utf8_lossy(bytes).to_string())
        }
        _ => Some(field.display_value().to_string()),
    }
}

fn subsec_ms(exif: &exif::Exif, tag: exif::Tag) -> i64 {
    let sub_tag = if tag == exif::Tag::DateTimeOriginal {
        exif::Tag::SubSecTimeOriginal
    } else if tag == exif::Tag::DateTimeDigitized {
        exif::Tag::SubSecTimeDigitized
    } else {
        exif::Tag::SubSecTime
    };
    let Some(field) = exif.get_field(sub_tag, exif::In::PRIMARY) else {
        return 0;
    };
    let Some(text) = ascii_value(field) else {
        return 0;
    };
    let digits: String = text
        .chars()
        .filter(|c| c.is_ascii_digit())
        .take(3)
        .collect();
    if digits.is_empty() {
        return 0;
    }
    let mut v: i64 = digits.parse().unwrap_or(0);
    if digits.len() == 1 {
        v *= 100;
    } else if digits.len() == 2 {
        v *= 10;
    }
    v
}

fn offset_for(exif: &exif::Exif, tag: exif::Tag) -> Option<i32> {
    let number = if tag == exif::Tag::DateTimeOriginal {
        0x9011
    } else if tag == exif::Tag::DateTimeDigitized {
        0x9012
    } else {
        0x9010
    };
    let field = exif.fields().find(|f| f.tag.number() == number)?;
    let text = ascii_value(field)?;
    parse_offset(&text)
}

fn parse_offset(text: &str) -> Option<i32> {
    let t = text.trim().trim_matches('\0').trim();
    if t.eq_ignore_ascii_case("z") {
        return Some(0);
    }
    let head = t.get(..5)?;
    let sign = match head.as_bytes()[0] {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let hours: i32 = head.get(1..3)?.parse().ok()?;
    let mins: i32 = if head.as_bytes().get(3) == Some(&b':') {
        t.get(4..6)?.parse().ok()?
    } else {
        head.get(3..5)?.parse().ok()?
    };
    if hours > 18 || mins >= 60 {
        return None;
    }
    let total = sign * (hours * 3600 + mins * 60);
    FixedOffset::east_opt(total).map(|_| total)
}

fn format_offset(secs: i32) -> String {
    let sign = if secs >= 0 { '+' } else { '-' };
    let abs = secs.abs();
    let hours = abs / 3600;
    let mins = (abs % 3600) / 60;
    format!("{sign}{hours:02}:{mins:02}")
}

pub fn wall_key(dt: NaiveDateTime, sub_ms: i64) -> i64 {
    let date = dt.date();
    let time = dt.time();
    ((((((date.year() as i64) * 100 + date.month() as i64) * 100 + date.day() as i64) * 100
        + time.hour() as i64)
        * 100
        + time.minute() as i64)
        * 100
        + time.second() as i64)
        * 1000
        + sub_ms.clamp(0, 999)
}

fn filesystem_wall(modified: Option<SystemTime>) -> (String, i64) {
    let st = modified.unwrap_or(UNIX_EPOCH);
    let dur = st.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = dur.as_secs() as i64;
    let ms = dur.subsec_millis() as i64;
    let utc = DateTime::<Utc>::from_timestamp(secs, 0)
        .unwrap_or_else(|| DateTime::<Utc>::from_timestamp(0, 0).unwrap());
    let local = utc.with_timezone(&Local);
    let text = local.format("%Y:%m:%d %H:%M:%S").to_string();
    let key = wall_key(local.naive_local(), ms);
    (text, key)
}

fn fallback_utc_from_photo(photo: &Photo) -> Option<(NaiveDateTime, i64)> {
    if photo.mtime_ns > 0 {
        let secs = photo.mtime_ns.div_euclid(1_000_000_000);
        let sub_ns = photo.mtime_ns.rem_euclid(1_000_000_000) as u32;
        let ms = (sub_ns / 1_000_000) as i64;
        if let Some(utc) = DateTime::<Utc>::from_timestamp(secs, 0) {
            return Some((utc.naive_utc(), ms));
        }
    }
    let text = photo.capture_text.as_deref()?;
    let naive = NaiveDateTime::parse_from_str(text, "%Y:%m:%d %H:%M:%S").ok()?;
    let ms = photo
        .capture_sort_key
        .map(|k| k.rem_euclid(1000))
        .unwrap_or(0);
    let local_dt = Local
        .from_local_datetime(&naive)
        .earliest()
        .or_else(|| Local.from_local_datetime(&naive).latest())?;
    Some((local_dt.naive_utc(), ms))
}

fn is_exif_source(source: &str) -> bool {
    source == "exif_original" || source == "metadata_fallback"
}

fn dominant_batch_offset(offsets: &[i32]) -> Option<i32> {
    let mut counts = std::collections::HashMap::<i32, usize>::new();
    for &off in offsets {
        *counts.entry(off).or_insert(0) += 1;
    }
    let local_secs = Local::now().offset().fix().local_minus_utc();
    offsets.iter().copied().max_by(|a, b| {
        let ca = counts.get(a).copied().unwrap_or(0);
        let cb = counts.get(b).copied().unwrap_or(0);
        ca.cmp(&cb)
            .then_with(|| (b - local_secs).abs().cmp(&(a - local_secs).abs()))
            .then_with(|| b.cmp(a))
    })
}

/// Normalize batch timestamps onto a consistent axis so mixed timezone-aware and naive photos never invert order.
pub fn normalize_batch_times(photos: &mut [Photo]) {
    let explicit_offsets: Vec<i32> = photos
        .iter()
        .filter(|p| is_exif_source(&p.time_source) && p.capture_text.is_some())
        .filter_map(|p| p.exif_offset.as_deref().and_then(parse_offset))
        .collect();

    if let Some(dominant_secs) = dominant_batch_offset(&explicit_offsets) {
        let dominant_label = format_offset(dominant_secs);
        for photo in photos.iter_mut() {
            let sub_ms = photo
                .capture_sort_key
                .map(|k| k.rem_euclid(1000))
                .unwrap_or(0);
            if is_exif_source(&photo.time_source) {
                let Some(naive) = photo
                    .capture_text
                    .as_deref()
                    .and_then(|t| NaiveDateTime::parse_from_str(t, "%Y:%m:%d %H:%M:%S").ok())
                else {
                    continue;
                };
                if let Some(secs) = photo.exif_offset.as_deref().and_then(parse_offset) {
                    if let Some(off) = FixedOffset::east_opt(secs)
                        .and_then(|o| o.from_local_datetime(&naive).single())
                    {
                        let utc_naive = off.naive_utc();
                        let utc_str = utc_naive.format("%Y:%m:%d %H:%M:%S").to_string();
                        photo.capture_sort_key = Some(wall_key(utc_naive, sub_ms));
                        photo.sort_time_text = Some(format!("{utc_str} UTC"));
                        photo.time_uncertain = false;
                        let off_label = photo
                            .exif_offset
                            .clone()
                            .unwrap_or_else(|| format_offset(secs));
                        let base =
                            format!("存在时区（{off_label}），已换算为 UTC（{utc_str}）比较");
                        photo.time_note = if photo.time_source == "exif_original" {
                            base
                        } else {
                            format!("使用备用嵌入时间。{base}")
                        };
                    }
                } else if let Some(off) = FixedOffset::east_opt(dominant_secs)
                    .and_then(|o| o.from_local_datetime(&naive).single())
                {
                    let assumed_utc = off.naive_utc();
                    let utc_str = assumed_utc.format("%Y:%m:%d %H:%M:%S").to_string();
                    photo.capture_sort_key = Some(wall_key(assumed_utc, sub_ms));
                    photo.sort_time_text = Some(format!("{utc_str} UTC (按{dominant_label}假定)"));
                    photo.time_uncertain = true;
                    let base = format!(
                        "同一批次存在带时区与无时区混排，此照片缺少 EXIF 时区，已按同批参考时区 {dominant_label} 换算为 UTC（{utc_str}）排序，建议核对顺序"
                    );
                    photo.time_note = if photo.time_source == "exif_original" {
                        base
                    } else {
                        format!("使用备用嵌入时间。{base}")
                    };
                }
            } else if photo.time_source == "filesystem_fallback"
                && !photo.corrupt
                && photo.capture_text.is_some()
            {
                if let Some((utc_naive, fs_ms)) = fallback_utc_from_photo(photo) {
                    let utc_str = utc_naive.format("%Y:%m:%d %H:%M:%S").to_string();
                    photo.capture_sort_key = Some(wall_key(utc_naive, fs_ms));
                    photo.sort_time_text = Some(format!("{utc_str} UTC"));
                    photo.time_uncertain = true;
                    if !photo.time_note.starts_with("已按文件修改时间重排") {
                        photo.time_note = format!(
                            "无嵌入拍摄时间，已将文件修改时间换算为 UTC（{utc_str}）与带时区 EXIF 对齐排序，建议核对"
                        );
                    }
                }
            }
        }
    } else {
        for photo in photos.iter_mut() {
            let sub_ms = photo
                .capture_sort_key
                .map(|k| k.rem_euclid(1000))
                .unwrap_or(0);
            if is_exif_source(&photo.time_source) {
                if let Some(naive) = photo
                    .capture_text
                    .as_deref()
                    .and_then(|t| NaiveDateTime::parse_from_str(t, "%Y:%m:%d %H:%M:%S").ok())
                {
                    photo.capture_sort_key = Some(wall_key(naive, sub_ms));
                    photo.sort_time_text = photo.capture_text.clone();
                    photo.time_uncertain = false;
                    let base = "EXIF 无时区，按原始墙上时间比较，未做 UTC 平移";
                    photo.time_note = if photo.time_source == "exif_original" {
                        base.to_string()
                    } else {
                        format!("使用备用嵌入时间。{base}")
                    };
                }
            } else if photo.time_source == "filesystem_fallback"
                && !photo.corrupt
                && photo.capture_text.is_some()
            {
                photo.sort_time_text = photo.capture_text.clone();
                photo.time_uncertain = false;
            }
        }
    }
}

pub fn sort_photos(photos: &mut [Photo]) {
    normalize_batch_times(photos);
    photos.sort_by(|a, b| {
        match (a.capture_sort_key, b.capture_sort_key) {
            (Some(ka), Some(kb)) => ka.cmp(&kb),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        }
        .then_with(|| {
            a.source_path
                .to_lowercase()
                .cmp(&b.source_path.to_lowercase())
        })
    });
    let mut counts = std::collections::HashMap::<i64, u32>::new();
    for photo in photos.iter() {
        if let Some(key) = photo.capture_sort_key {
            *counts.entry(key).or_insert(0) += 1;
        }
    }
    for (i, photo) in photos.iter_mut().enumerate() {
        photo.sort_index = i as u32;
        photo.equal_timestamp = photo
            .capture_sort_key
            .map(|k| counts.get(&k).copied().unwrap_or(0) > 1)
            .unwrap_or(false);
        photo.tie_break = "path".into();
        photo.tie_break_is_capture_order = false;
    }
}

pub fn list_part_files(root: &Path) -> Vec<PathBuf> {
    if crate::util::metadata_any(root).is_err() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for entry in WalkDir::new(crate::util::win_extended(root))
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy();
        if name.ends_with(".tuxu-part") {
            out.push(crate::util::plain_path(entry.path()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_offset_and_unicode_datetime_never_panic() {
        assert_eq!(parse_offset("+08:00"), Some(8 * 3600));
        assert_eq!(parse_offset("-05:30"), Some(-(5 * 3600 + 1800)));
        assert_eq!(parse_offset("+0中文"), None);
        assert_eq!(parse_offset("中"), None);

        let multi_byte = "2024:05:05 中文字符测试EXIF非ASCII边界";
        assert_eq!(multi_byte.get(..19), None);
        let ok = "2024:05:05 12:34:56中文尾部";
        assert_eq!(ok.get(..19), Some("2024:05:05 12:34:56"));
    }

    fn jpeg_with_exif_and_offset(date_time: &str, offset: Option<&str>) -> Vec<u8> {
        assert_eq!(date_time.len(), 19);
        let mut dt_bytes = date_time.as_bytes().to_vec();
        dt_bytes.push(0);
        let off_bytes = offset.map(|s| {
            let mut v = s.as_bytes().to_vec();
            v.push(0);
            v
        });
        let count: u16 = if off_bytes.is_some() { 2 } else { 1 };
        let data_start: u32 = 26 + 2 + u32::from(count) * 12 + 4;
        let mut tiff = Vec::new();
        tiff.extend_from_slice(b"II");
        tiff.extend_from_slice(&42u16.to_le_bytes());
        tiff.extend_from_slice(&8u32.to_le_bytes());
        tiff.extend_from_slice(&1u16.to_le_bytes());
        tiff.extend_from_slice(&0x8769u16.to_le_bytes());
        tiff.extend_from_slice(&4u16.to_le_bytes());
        tiff.extend_from_slice(&1u32.to_le_bytes());
        tiff.extend_from_slice(&26u32.to_le_bytes());
        tiff.extend_from_slice(&0u32.to_le_bytes());
        tiff.extend_from_slice(&count.to_le_bytes());
        tiff.extend_from_slice(&0x9003u16.to_le_bytes());
        tiff.extend_from_slice(&2u16.to_le_bytes());
        tiff.extend_from_slice(&20u32.to_le_bytes());
        tiff.extend_from_slice(&data_start.to_le_bytes());
        if let Some(ref ob) = off_bytes {
            let off_start = data_start + 20;
            tiff.extend_from_slice(&0x9011u16.to_le_bytes());
            tiff.extend_from_slice(&2u16.to_le_bytes());
            tiff.extend_from_slice(&(ob.len() as u32).to_le_bytes());
            tiff.extend_from_slice(&off_start.to_le_bytes());
        }
        tiff.extend_from_slice(&0u32.to_le_bytes());
        tiff.extend_from_slice(&dt_bytes);
        if let Some(ref ob) = off_bytes {
            tiff.extend_from_slice(ob);
        }
        let mut app1 = Vec::new();
        app1.extend_from_slice(b"Exif\0\0");
        app1.extend_from_slice(&tiff);
        let len = (app1.len() + 2) as u16;
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE1];
        jpeg.extend_from_slice(&len.to_be_bytes());
        jpeg.extend_from_slice(&app1);
        jpeg.extend_from_slice(&[0xFF, 0xD9]);
        jpeg
    }

    #[test]
    fn mixed_offset_and_naive_exif_never_inverts_order() {
        let dir = std::env::temp_dir().join(format!("tuxu-scan-tz-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        // Minimal 09:00 naive vs 10:00 +08:00 counterexample
        fs::write(
            dir.join("a_1000_plus08.jpg"),
            jpeg_with_exif_and_offset("2026:03:01 10:00:00", Some("+08:00")),
        )
        .unwrap();
        fs::write(
            dir.join("b_0900_naive.jpg"),
            jpeg_with_exif_and_offset("2026:03:01 09:00:00", None),
        )
        .unwrap();
        // Cross-day mixed: 23:30 naive day 1 vs 00:30 +08:00 day 2
        fs::write(
            dir.join("c_2330_naive.jpg"),
            jpeg_with_exif_and_offset("2026:03:01 23:30:00", None),
        )
        .unwrap();
        fs::write(
            dir.join("d_0030_plus08.jpg"),
            jpeg_with_exif_and_offset("2026:03:02 00:30:00", Some("+08:00")),
        )
        .unwrap();

        let out = scan_tree(&dir, false).unwrap();
        let names: Vec<_> = out.photos.iter().map(|p| p.file_name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "b_0900_naive.jpg",
                "a_1000_plus08.jpg",
                "c_2330_naive.jpg",
                "d_0030_plus08.jpg"
            ]
        );
        assert!(out.photos[0].time_uncertain);
        assert!(!out.photos[1].time_uncertain);
        assert!(out.photos[2].time_uncertain);
        assert!(!out.photos[3].time_uncertain);
        assert_eq!(out.photos[0].original_scan_index, 0);
        assert_eq!(out.photos[1].original_scan_index, 1);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn distinct_offsets_dst_and_equal_utc_tie_work_accurately() {
        let dir = std::env::temp_dir().join(format!("tuxu-scan-dst-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        // 22:00 -04:00 == 02:00 UTC day 2; 21:30 -05:00 == 02:30 UTC day 2; 10:00 +08:00 == 02:00 UTC day 2 (tie with -04:00!)
        fs::write(
            dir.join("b_dst_minus4.jpg"),
            jpeg_with_exif_and_offset("2026:03:01 22:00:00", Some("-04:00")),
        )
        .unwrap();
        fs::write(
            dir.join("a_plus08_same_utc.jpg"),
            jpeg_with_exif_and_offset("2026:03:02 10:00:00", Some("+08:00")),
        )
        .unwrap();
        fs::write(
            dir.join("c_std_minus5.jpg"),
            jpeg_with_exif_and_offset("2026:03:01 21:30:00", Some("-05:00")),
        )
        .unwrap();

        let out = scan_tree(&dir, false).unwrap();
        let names: Vec<_> = out.photos.iter().map(|p| p.file_name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "a_plus08_same_utc.jpg",
                "b_dst_minus4.jpg",
                "c_std_minus5.jpg"
            ]
        );
        assert!(out.photos[0].equal_timestamp);
        assert!(out.photos[1].equal_timestamp);
        assert!(!out.photos[2].equal_timestamp);
        assert!(out.photos.iter().all(|p| !p.time_uncertain));

        let _ = fs::remove_dir_all(&dir);
    }
}
