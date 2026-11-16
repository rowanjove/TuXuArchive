use crate::model::PhotoQuality;
use crate::util::win_extended;
use image::{DynamicImage, GrayImage, RgbImage};
use std::fs::{self, File};
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhotoKind {
    Jpeg,
    Png,
    Bmp,
    Webp,
    Tiff,
    Gif,
    Heic,
    Raw,
}

#[derive(Debug, Clone)]
pub struct ProbeOutcome {
    pub corrupt: bool,
    pub preview_available: bool,
    #[allow(dead_code)]
    pub truncated: bool,
    #[allow(dead_code)]
    pub dimensions: Option<(u32, u32)>,
    pub quality: PhotoQuality,
}

pub fn classify_extension(ext_lower: &str) -> Option<PhotoKind> {
    match ext_lower {
        "jpg" | "jpeg" | "jfif" => Some(PhotoKind::Jpeg),
        "png" => Some(PhotoKind::Png),
        "bmp" => Some(PhotoKind::Bmp),
        "webp" => Some(PhotoKind::Webp),
        "tif" | "tiff" => Some(PhotoKind::Tiff),
        "gif" | "ico" => Some(PhotoKind::Gif),
        "heic" | "heif" | "avif" => Some(PhotoKind::Heic),
        "dng" | "cr2" | "cr3" | "nef" | "arw" | "orf" | "rw2" | "raf" | "srw" | "pef" => {
            Some(PhotoKind::Raw)
        }
        _ => None,
    }
}

pub fn open_any(path: &Path) -> std::io::Result<File> {
    File::open(win_extended(path)).or_else(|_| File::open(path))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContainerMagic {
    Jpeg,
    Png,
    Bmp,
    Webp,
    Tiff,
    Gif,
    Ico,
    Heic,
    Other,
}

fn sniff_magic(head: &[u8]) -> ContainerMagic {
    if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return ContainerMagic::Jpeg;
    }
    if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        return ContainerMagic::Png;
    }
    if head.starts_with(b"BM") && head.len() >= 14 {
        return ContainerMagic::Bmp;
    }
    if head.len() >= 12 && &head[0..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        return ContainerMagic::Webp;
    }
    if head.len() >= 4
        && (&head[0..4] == b"II*\0"
            || &head[0..4] == b"MM\0*"
            || &head[0..4] == b"II+\0"
            || &head[0..4] == b"MM\0+")
    {
        return ContainerMagic::Tiff;
    }
    if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        return ContainerMagic::Gif;
    }
    if head.len() >= 4 && (head[0..4] == [0, 0, 1, 0] || head[0..4] == [0, 0, 2, 0]) {
        return ContainerMagic::Ico;
    }
    if head.len() >= 12 && &head[4..8] == b"ftyp" {
        let brand = String::from_utf8_lossy(&head[8..12]).to_ascii_lowercase();
        if brand.contains("hei")
            || brand == "mif1"
            || brand == "msf1"
            || brand == "heif"
            || brand == "avif"
            || brand == "avis"
        {
            return ContainerMagic::Heic;
        }
    }
    ContainerMagic::Other
}

fn read_head_and_tail(path: &Path) -> std::io::Result<(u64, Vec<u8>, Vec<u8>)> {
    let mut file = open_any(path)?;
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let head_len = (len as usize).min(65536);
    let mut head = vec![0u8; head_len];
    let n = file.read(&mut head)?;
    head.truncate(n);

    let tail = if len <= 4096 {
        head[head.len().saturating_sub(4096)..].to_vec()
    } else {
        let start = len.saturating_sub(4096);
        let mut buf = Vec::with_capacity(4096);
        if file.seek(SeekFrom::Start(start)).is_ok() {
            let _ = file.read_to_end(&mut buf);
        }
        buf
    };
    Ok((len, head, tail))
}

fn contains_subsequence(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

fn check_truncation(magic: ContainerMagic, size: u64, head: &[u8], tail: &[u8]) -> bool {
    if size == 0 {
        return true;
    }
    match magic {
        ContainerMagic::Jpeg => size >= 4 && !contains_subsequence(tail, &[0xFF, 0xD9]),
        ContainerMagic::Png => size >= 24 && !contains_subsequence(tail, b"IEND"),
        ContainerMagic::Webp => {
            if head.len() < 12 {
                return true;
            }
            let declared = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as u64 + 8;
            size + 1 < declared
        }
        ContainerMagic::Bmp => {
            if head.len() < 26 {
                return true;
            }
            let declared_size = u32::from_le_bytes([head[2], head[3], head[4], head[5]]) as u64;
            let data_offset = u32::from_le_bytes([head[10], head[11], head[12], head[13]]) as u64;
            if data_offset > size {
                return true;
            }
            declared_size >= data_offset && size + 4 < declared_size && declared_size > 1024
        }
        ContainerMagic::Gif => size >= 14 && !tail.ends_with(&[0x3B]) && !tail.contains(&0x3B),
        ContainerMagic::Tiff => {
            if head.len() < 8 {
                return true;
            }
            let le = &head[0..2] == b"II";
            let is_big = &head[2..4] == b"+\0" || &head[2..4] == b"\0+";
            if is_big {
                return head.len() < 16;
            }
            let ifd0 = if le {
                u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as u64
            } else {
                u32::from_be_bytes([head[4], head[5], head[6], head[7]]) as u64
            };
            ifd0 < 8 || ifd0 + 2 > size
        }
        ContainerMagic::Ico | ContainerMagic::Heic | ContainerMagic::Other => false,
    }
}

fn probe_header_dimensions(
    path: &Path,
    kind: PhotoKind,
    magic: ContainerMagic,
    head: &[u8],
) -> Option<(u32, u32)> {
    match magic {
        ContainerMagic::Jpeg => image::ImageReader::with_format(
            BufReader::new(std::io::Cursor::new(head)),
            image::ImageFormat::Jpeg,
        )
        .into_dimensions()
        .ok()
        .or_else(|| {
            open_any(path).ok().and_then(|f| {
                image::ImageReader::with_format(BufReader::new(f), image::ImageFormat::Jpeg)
                    .into_dimensions()
                    .ok()
            })
        }),
        ContainerMagic::Png => {
            if head.len() >= 24 && &head[12..16] == b"IHDR" {
                let w = u32::from_be_bytes([head[16], head[17], head[18], head[19]]);
                let h = u32::from_be_bytes([head[20], head[21], head[22], head[23]]);
                if w > 0 && h > 0 {
                    return Some((w, h));
                }
            }
            None
        }
        ContainerMagic::Bmp => image::ImageReader::with_format(
            BufReader::new(std::io::Cursor::new(head)),
            image::ImageFormat::Bmp,
        )
        .into_dimensions()
        .ok(),
        ContainerMagic::Gif => {
            if head.len() >= 10 {
                let w = u16::from_le_bytes([head[6], head[7]]) as u32;
                let h = u16::from_le_bytes([head[8], head[9]]) as u32;
                if w > 0 && h > 0 {
                    return Some((w, h));
                }
            }
            None
        }
        ContainerMagic::Webp => parse_webp_dimensions(head),
        ContainerMagic::Tiff => {
            let _ = kind;
            parse_tiff_dimensions(path)
        }
        ContainerMagic::Ico | ContainerMagic::Heic | ContainerMagic::Other => None,
    }
}

fn parse_webp_dimensions(head: &[u8]) -> Option<(u32, u32)> {
    if head.len() < 30 || &head[0..4] != b"RIFF" || &head[8..12] != b"WEBP" {
        return None;
    }
    let chunk = &head[12..16];
    match chunk {
        b"VP8 " => {
            if head.len() >= 30 && head[23] == 0x9D && head[24] == 0x01 && head[25] == 0x2A {
                let w = (u16::from_le_bytes([head[26], head[27]]) & 0x3FFF) as u32;
                let h = (u16::from_le_bytes([head[28], head[29]]) & 0x3FFF) as u32;
                if w > 0 && h > 0 {
                    return Some((w, h));
                }
            }
        }
        b"VP8L" => {
            if head.len() >= 25 && head[20] == 0x2F {
                let bits = u32::from_le_bytes([head[21], head[22], head[23], head[24]]);
                let w = (bits & 0x3FFF) + 1;
                let h = ((bits >> 14) & 0x3FFF) + 1;
                return Some((w, h));
            }
        }
        b"VP8X" => {
            if head.len() >= 30 {
                let w =
                    1 + (head[24] as u32 | ((head[25] as u32) << 8) | ((head[26] as u32) << 16));
                let h =
                    1 + (head[27] as u32 | ((head[28] as u32) << 8) | ((head[29] as u32) << 16));
                return Some((w, h));
            }
        }
        _ => {}
    }
    None
}

fn has_valid_raw_container(head: &[u8], magic: ContainerMagic) -> bool {
    if magic == ContainerMagic::Tiff {
        return true;
    }
    if head.len() >= 12 && &head[4..8] == b"ftyp" {
        let brand = &head[8..12];
        if brand == b"crx " {
            return true;
        }
    }
    if head.starts_with(b"FUJIFILMCCD-RAW") {
        return true;
    }
    if head.len() >= 4
        && (&head[0..4] == b"IIRO"
            || &head[0..4] == b"IIRS"
            || &head[0..4] == b"IIU\0"
            || &head[0..4] == b"\0MRM"
            || &head[0..4] == b"FOVb")
    {
        return true;
    }
    head.len() >= 14 && &head[0..2] == b"II" && &head[6..14] == b"HEAPCCDR"
}

pub fn probe_photo(path: &Path, kind: PhotoKind) -> ProbeOutcome {
    let Ok((size, head, tail)) = read_head_and_tail(path) else {
        return ProbeOutcome {
            corrupt: true,
            preview_available: false,
            truncated: true,
            dimensions: None,
            quality: build_structural_quality(0, None, true, true, false),
        };
    };
    if size == 0 {
        return ProbeOutcome {
            corrupt: true,
            preview_available: false,
            truncated: true,
            dimensions: None,
            quality: build_structural_quality(0, None, true, true, false),
        };
    }

    let magic = sniff_magic(&head);
    let truncated = check_truncation(magic, size, &head, &tail);
    let mut dims = probe_header_dimensions(path, kind, magic, &head);

    let (corrupt, preview_available) = match kind {
        PhotoKind::Jpeg => {
            let valid = magic == ContainerMagic::Jpeg;
            (!valid, valid)
        }
        PhotoKind::Png => {
            let valid = magic == ContainerMagic::Png && dims.is_some();
            (!valid, valid)
        }
        PhotoKind::Bmp => {
            let valid = magic == ContainerMagic::Bmp && dims.is_some();
            (!valid, valid)
        }
        PhotoKind::Webp => {
            if magic != ContainerMagic::Webp || size < 16 {
                (true, false)
            } else {
                let ok = dims.is_some()
                    || wic_probe_dimensions(path)
                        .map(|d| {
                            dims = Some(d);
                        })
                        .is_some();
                (false, ok)
            }
        }
        PhotoKind::Gif => {
            if !matches!(magic, ContainerMagic::Gif | ContainerMagic::Ico) || size < 14 {
                (true, false)
            } else {
                let ok = dims.is_some()
                    || wic_probe_dimensions(path)
                        .map(|d| {
                            dims = Some(d);
                        })
                        .is_some();
                (false, ok)
            }
        }
        PhotoKind::Tiff => {
            if magic != ContainerMagic::Tiff || size < 12 {
                (true, false)
            } else {
                let ok = can_decode_baseline_tiff(path)
                    || extract_embedded_jpeg(path).is_some()
                    || wic_probe_dimensions(path)
                        .map(|d| {
                            if dims.is_none() {
                                dims = Some(d);
                            }
                        })
                        .is_some();
                (false, ok)
            }
        }
        PhotoKind::Raw => {
            if !has_valid_raw_container(&head, magic) || size < 16 {
                (true, false)
            } else {
                let embedded = extract_embedded_jpeg(path).is_some();
                let wic_dims = if !embedded || dims.is_none() {
                    wic_probe_dimensions(path)
                } else {
                    None
                };
                if dims.is_none() {
                    dims = wic_dims;
                }
                (false, embedded || wic_dims.is_some())
            }
        }
        PhotoKind::Heic => {
            if magic != ContainerMagic::Heic || size < 16 {
                (true, false)
            } else {
                let wic_dims = wic_probe_dimensions(path);
                if dims.is_none() {
                    dims = wic_dims;
                }
                let has_thumb = extract_embedded_jpeg_scan(path).is_some();
                (false, wic_dims.is_some() || has_thumb)
            }
        }
    };

    let quality = build_structural_quality(size, dims, truncated, corrupt, preview_available);
    ProbeOutcome {
        corrupt,
        preview_available,
        truncated,
        dimensions: dims,
        quality,
    }
}

pub fn build_structural_quality(
    size: u64,
    dims: Option<(u32, u32)>,
    truncated: bool,
    corrupt: bool,
    preview_available: bool,
) -> PhotoQuality {
    let mut issues = Vec::new();
    if corrupt {
        issues.push("corrupt".to_string());
    }
    if truncated && !corrupt {
        issues.push("truncated".to_string());
    }
    if let Some((w, h)) = dims {
        if w < 800 || h < 600 {
            issues.push("low_res".to_string());
        }
    }
    if !corrupt && !preview_available {
        issues.push("unsupported_preview".to_string());
    }
    if !corrupt && size < 4096 && dims.is_some_and(|(w, h)| w >= 64 && h >= 64) {
        issues.push("truncated".to_string());
    }
    issues.dedup();
    let status = classify_quality_status(&issues, false);
    let summary = format_quality_summary(dims, &issues, false);
    PhotoQuality {
        status,
        width: dims.map(|d| d.0),
        height: dims.map(|d| d.1),
        issues,
        summary,
        luma_mean: None,
        sharpness: None,
        pixel_checked: false,
    }
}

pub fn merge_pixel_quality(base: &PhotoQuality, metrics: PixelMetrics) -> PhotoQuality {
    let mut issues: Vec<String> = base
        .issues
        .iter()
        .filter(|tag| {
            !matches!(
                tag.as_str(),
                "blur" | "underexposed" | "overexposed" | "low_contrast"
            )
        })
        .cloned()
        .collect();
    if let Some((w, h)) = metrics.evaluated_dims {
        if w >= 32 && h >= 32 {
            if metrics.underexposed {
                issues.push("underexposed".to_string());
            }
            if metrics.overexposed {
                issues.push("overexposed".to_string());
            }
            if metrics.low_contrast {
                issues.push("low_contrast".to_string());
            }
            if metrics.blur {
                issues.push("blur".to_string());
            }
        }
    }
    let dims = base.width.zip(base.height).or(metrics.source_dims);
    let status = classify_quality_status(&issues, true);
    let summary = format_quality_summary(dims, &issues, true);
    PhotoQuality {
        status,
        width: dims.map(|d| d.0),
        height: dims.map(|d| d.1),
        issues,
        summary,
        luma_mean: Some((metrics.luma_mean * 10.0).round() / 10.0),
        sharpness: Some((metrics.sharpness * 10.0).round() / 10.0),
        pixel_checked: true,
    }
}

fn classify_quality_status(issues: &[String], pixel_checked: bool) -> String {
    if issues.iter().any(|x| x == "corrupt" || x == "truncated") {
        return "bad".to_string();
    }
    if issues.iter().any(|x| {
        matches!(
            x.as_str(),
            "blur"
                | "underexposed"
                | "overexposed"
                | "low_contrast"
                | "low_res"
                | "unsupported_preview"
        )
    }) {
        return "warn".to_string();
    }
    if pixel_checked {
        "ok".to_string()
    } else {
        "unchecked".to_string()
    }
}

pub fn quality_issue_label(code: &str) -> &'static str {
    match code {
        "corrupt" => "文件损坏",
        "truncated" => "文件数据可能截断",
        "low_res" => "分辨率偏低",
        "blur" => "画面模糊/虚焦",
        "underexposed" => "画面偏暗/欠曝",
        "overexposed" => "画面过亮/过曝",
        "low_contrast" => "灰蒙/反差偏低",
        "unsupported_preview" => "可按原文件归档（系统缺少预览解码组件）",
        _ => "画质待确认",
    }
}

fn format_quality_summary(
    dims: Option<(u32, u32)>,
    issues: &[String],
    pixel_checked: bool,
) -> String {
    let dim_prefix = dims.map(|(w, h)| format!("{w}×{h}"));
    let issue_text = if issues.is_empty() {
        if pixel_checked {
            "清晰度与曝光正常".to_string()
        } else {
            "文件头完整（未做像素级检查）".to_string()
        }
    } else {
        issues
            .iter()
            .map(|c| quality_issue_label(c))
            .collect::<Vec<_>>()
            .join("；")
    };
    match dim_prefix {
        Some(dim) => format!("{dim} · {issue_text}"),
        None => issue_text,
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PixelMetrics {
    pub source_dims: Option<(u32, u32)>,
    pub evaluated_dims: Option<(u32, u32)>,
    pub luma_mean: f32,
    pub sharpness: f32,
    pub underexposed: bool,
    pub overexposed: bool,
    pub low_contrast: bool,
    pub blur: bool,
}

pub fn analyze_image_pixels(img: &DynamicImage, source_dims: Option<(u32, u32)>) -> PixelMetrics {
    let small = if img.width() > 320 || img.height() > 320 {
        img.thumbnail(320, 320)
    } else {
        img.clone()
    };
    let gray: GrayImage = small.to_luma8();
    let w = gray.width();
    let h = gray.height();
    let raw = gray.as_raw();
    let total = raw.len().max(1) as f64;

    let mut sum = 0.0f64;
    let mut dark_count = 0usize;
    let mut bright_count = 0usize;
    for &p in raw {
        let v = p as f64;
        sum += v;
        if p <= 24 {
            dark_count += 1;
        }
        if p >= 246 {
            bright_count += 1;
        }
    }
    let mean = sum / total;
    let mut var_sum = 0.0f64;
    for &p in raw {
        let diff = p as f64 - mean;
        var_sum += diff * diff;
    }
    let stddev = (var_sum / total).sqrt();
    let dark_ratio = dark_count as f64 / total;
    let bright_ratio = bright_count as f64 / total;

    // 4x4 tile top-quartile Laplacian variance: ignores uniform sky in sharp drone photos.
    let sharpness = if w >= 12 && h >= 12 {
        let mut tile_vars = Vec::with_capacity(16);
        for ty in 0..4u32 {
            for tx in 0..4u32 {
                let x0 = ((tx * w) / 4).max(1);
                let x1 = (((tx + 1) * w) / 4).min(w - 1);
                let y0 = ((ty * h) / 4).max(1);
                let y1 = (((ty + 1) * h) / 4).min(h - 1);
                if x1 <= x0 || y1 <= y0 {
                    continue;
                }
                let mut lsum = 0.0f64;
                let mut lsq = 0.0f64;
                let mut lcnt = 0usize;
                for y in y0..y1 {
                    let row = (y * w) as usize;
                    let prev = ((y - 1) * w) as usize;
                    let next = ((y + 1) * w) as usize;
                    for x in x0..x1 {
                        let xu = x as usize;
                        let center = raw[row + xu] as f64;
                        let lap = (raw[prev + xu] as f64)
                            + (raw[next + xu] as f64)
                            + (raw[row + xu - 1] as f64)
                            + (raw[row + xu + 1] as f64)
                            - 4.0 * center;
                        lsum += lap;
                        lsq += lap * lap;
                        lcnt += 1;
                    }
                }
                if lcnt > 0 {
                    let lmean = lsum / lcnt as f64;
                    let lvar = (lsq / lcnt as f64) - lmean * lmean;
                    tile_vars.push(lvar.max(0.0));
                }
            }
        }
        if tile_vars.is_empty() {
            0.0
        } else {
            tile_vars.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
            let take_n = tile_vars.len().min(4);
            tile_vars[..take_n].iter().sum::<f64>() / take_n as f64
        }
    } else {
        0.0
    };

    let underexposed = mean < 42.0 || (dark_ratio > 0.72 && mean < 68.0);
    let overexposed = bright_ratio > 0.55 || (mean > 228.0 && bright_ratio > 0.35);
    let low_contrast = stddev < 14.0;
    let blur = !low_contrast && sharpness < 55.0;

    PixelMetrics {
        source_dims,
        evaluated_dims: Some((img.width(), img.height())),
        luma_mean: mean as f32,
        sharpness: sharpness as f32,
        underexposed,
        overexposed,
        low_contrast,
        blur,
    }
}

pub fn decode_preview_image(path: &Path, max_edge: u32) -> Result<DynamicImage, String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let kind = classify_extension(&ext);

    let img = match kind {
        Some(PhotoKind::Jpeg) | Some(PhotoKind::Png) | Some(PhotoKind::Bmp) => {
            decode_with_image_crate(path).or_else(|e| {
                #[cfg(windows)]
                {
                    wic_decode_scaled(path, max_edge).ok_or(e)
                }
                #[cfg(not(windows))]
                {
                    Err(e)
                }
            })?
        }
        Some(PhotoKind::Tiff) => {
            #[cfg(windows)]
            let res = wic_decode_scaled(path, max_edge)
                .ok_or_else(|| "Windows 解码跳过".to_string())
                .or_else(|_| decode_baseline_tiff(path))
                .or_else(|_| decode_embedded_jpeg(path));
            #[cfg(not(windows))]
            let res = decode_baseline_tiff(path).or_else(|_| decode_embedded_jpeg(path));
            res?
        }
        Some(PhotoKind::Raw) => decode_embedded_jpeg(path).or_else(|e| {
            #[cfg(windows)]
            {
                wic_decode_scaled(path, max_edge).ok_or(e)
            }
            #[cfg(not(windows))]
            {
                Err(e)
            }
        })?,
        Some(PhotoKind::Webp) | Some(PhotoKind::Gif) => {
            #[cfg(windows)]
            let res = wic_decode_scaled(path, max_edge)
                .ok_or_else(|| "当前格式预览解码失败".to_string())
                .or_else(|_| decode_with_image_crate(path));
            #[cfg(not(windows))]
            let res = decode_with_image_crate(path);
            res?
        }
        Some(PhotoKind::Heic) => {
            #[cfg(windows)]
            let res = wic_decode_scaled(path, max_edge)
                .ok_or_else(|| "系统缺少 HEIF 扩展组件，可按原文件安全归档".to_string())
                .or_else(|e| decode_embedded_jpeg_scan(path).map_err(|_| e));
            #[cfg(not(windows))]
            let res = decode_embedded_jpeg_scan(path);
            res?
        }
        None => decode_with_image_crate(path)?,
    };

    if matches!(kind, Some(PhotoKind::Heic)) {
        Ok(img)
    } else {
        Ok(apply_exif_orientation(path, img))
    }
}

fn decode_with_image_crate(path: &Path) -> Result<DynamicImage, String> {
    let reader = image::ImageReader::open(win_extended(path))
        .or_else(|_| image::ImageReader::open(path))
        .map_err(|e| format!("无法打开照片：{e}"))?
        .with_guessed_format()
        .map_err(|e| format!("无法识别照片格式：{e}"))?;
    reader.decode().map_err(|e| format!("照片解码失败：{e}"))
}

fn decode_embedded_jpeg(path: &Path) -> Result<DynamicImage, String> {
    let bytes = extract_embedded_jpeg(path)
        .or_else(|| extract_embedded_jpeg_scan(path))
        .ok_or_else(|| "文件中没有可提取的内嵌 JPEG 预览".to_string())?;
    image::load_from_memory_with_format(&bytes, image::ImageFormat::Jpeg)
        .map_err(|e| format!("内嵌预览图解码失败：{e}"))
}

fn decode_embedded_jpeg_scan(path: &Path) -> Result<DynamicImage, String> {
    let bytes = extract_embedded_jpeg_scan(path)
        .ok_or_else(|| "系统未安装 HEIF 扩展，且文件内没有可用 JPEG 缩略图".to_string())?;
    image::load_from_memory_with_format(&bytes, image::ImageFormat::Jpeg)
        .map_err(|e| format!("缩略图解码失败：{e}"))
}

fn apply_exif_orientation(path: &Path, img: DynamicImage) -> DynamicImage {
    let Ok(file) = open_any(path) else {
        return img;
    };
    let mut reader = BufReader::new(file);
    let Ok(exif_data) = exif::Reader::new().read_from_container(&mut reader) else {
        return img;
    };
    let Some(field) = exif_data.get_field(exif::Tag::Orientation, exif::In::PRIMARY) else {
        return img;
    };
    let Some(val) = field.value.get_uint(0) else {
        return img;
    };
    match val {
        2 => img.fliph(),
        3 => img.rotate180(),
        4 => img.flipv(),
        5 => img.rotate90().fliph(),
        6 => img.rotate90(),
        7 => img.rotate270().fliph(),
        8 => img.rotate270(),
        _ => img,
    }
}

/* ==================== Pure-Rust TIFF/DNG IFD + Embedded Preview + Baseline TIFF ==================== */

struct TiffEndian {
    le: bool,
}

impl TiffEndian {
    fn u16(&self, b: &[u8]) -> u16 {
        if self.le {
            u16::from_le_bytes([b[0], b[1]])
        } else {
            u16::from_be_bytes([b[0], b[1]])
        }
    }
    fn u32(&self, b: &[u8]) -> u32 {
        if self.le {
            u32::from_le_bytes([b[0], b[1], b[2], b[3]])
        } else {
            u32::from_be_bytes([b[0], b[1], b[2], b[3]])
        }
    }
}

#[derive(Default)]
struct IfdSummary {
    width: Option<u32>,
    height: Option<u32>,
    compression: Option<u16>,
    photometric: Option<u16>,
    samples_per_pixel: Option<u16>,
    bits_per_sample: Vec<u16>,
    strip_offsets: Vec<u32>,
    strip_byte_counts: Vec<u32>,
    jpeg_offset: Option<u32>,
    jpeg_length: Option<u32>,
    sub_ifds: Vec<u32>,
    next_ifd: u32,
}

fn read_ifd_values_u32(
    data: &[u8],
    e: &TiffEndian,
    typ: u16,
    count: u32,
    val_off_bytes: &[u8],
) -> Vec<u32> {
    let elem_size = match typ {
        1 | 2 | 6 | 7 => 1usize,
        3 | 8 => 2usize,
        4 | 9 => 4usize,
        _ => return Vec::new(),
    };
    let total = elem_size.saturating_mul(count as usize);
    let src: &[u8] = if total <= 4 {
        val_off_bytes
    } else {
        let off = e.u32(val_off_bytes) as usize;
        if off.saturating_add(total) > data.len() {
            return Vec::new();
        }
        &data[off..off + total]
    };
    let mut out = Vec::with_capacity((count as usize).min(4096));
    for i in 0..(count as usize).min(4096) {
        let start = i * elem_size;
        if start + elem_size > src.len() {
            break;
        }
        let v = match elem_size {
            1 => src[start] as u32,
            2 => e.u16(&src[start..start + 2]) as u32,
            4 => e.u32(&src[start..start + 4]),
            _ => 0,
        };
        out.push(v);
    }
    out
}

fn parse_ifd_at(data: &[u8], e: &TiffEndian, offset: usize) -> Option<IfdSummary> {
    if offset + 2 > data.len() {
        return None;
    }
    let count = e.u16(&data[offset..offset + 2]) as usize;
    let entries_end = offset.checked_add(2 + count * 12)?;
    if entries_end + 4 > data.len() {
        return None;
    }
    let mut out = IfdSummary::default();
    for i in 0..count {
        let pos = offset + 2 + i * 12;
        let tag = e.u16(&data[pos..pos + 2]);
        let typ = e.u16(&data[pos + 2..pos + 4]);
        let cnt = e.u32(&data[pos + 4..pos + 8]);
        let val_bytes = &data[pos + 8..pos + 12];
        let vals = || read_ifd_values_u32(data, e, typ, cnt, val_bytes);
        match tag {
            0x0100 => out.width = vals().first().copied(),
            0x0101 => out.height = vals().first().copied(),
            0x0102 => out.bits_per_sample = vals().into_iter().map(|v| v as u16).collect(),
            0x0103 => out.compression = vals().first().map(|v| *v as u16),
            0x0106 => out.photometric = vals().first().map(|v| *v as u16),
            0x0111 => out.strip_offsets = vals(),
            0x0115 => out.samples_per_pixel = vals().first().map(|v| *v as u16),
            0x0117 => out.strip_byte_counts = vals(),
            0x014A => out.sub_ifds = vals(),
            0x0201 => out.jpeg_offset = vals().first().copied(),
            0x0202 => out.jpeg_length = vals().first().copied(),
            _ => {}
        }
    }
    out.next_ifd = e.u32(&data[entries_end..entries_end + 4]);
    Some(out)
}

fn read_tiff_prefix_or_all(path: &Path) -> Option<Vec<u8>> {
    let mut file = open_any(path).ok()?;
    let len = file.metadata().ok()?.len();
    if !(8..=160 * 1024 * 1024).contains(&len) {
        return None;
    }
    let mut buf = Vec::with_capacity(len as usize);
    file.read_to_end(&mut buf).ok()?;
    Some(buf)
}

fn parse_tiff_dimensions(path: &Path) -> Option<(u32, u32)> {
    let mut file = open_any(path).ok()?;
    let mut buf = vec![0u8; 256 * 1024];
    let n = file.read(&mut buf).ok()?;
    buf.truncate(n);
    if buf.len() < 8 {
        return None;
    }
    let le = if &buf[0..4] == b"II*\0" {
        true
    } else if &buf[0..4] == b"MM\0*" {
        false
    } else {
        return None;
    };
    let e = TiffEndian { le };
    let first = e.u32(&buf[4..8]) as usize;
    let ifd = parse_ifd_at(&buf, &e, first)?;
    let w = ifd.width.filter(|v| *v > 0)?;
    let h = ifd.height.filter(|v| *v > 0)?;
    Some((w, h))
}

fn can_decode_baseline_tiff(path: &Path) -> bool {
    let Some(data) = read_tiff_prefix_or_all(path) else {
        return false;
    };
    let Some((e, ifd0)) = parse_tiff_ifd0(&data) else {
        return false;
    };
    let _ = e;
    is_supported_baseline_tiff(&ifd0)
}

fn parse_tiff_ifd0(data: &[u8]) -> Option<(TiffEndian, IfdSummary)> {
    if data.len() < 8 {
        return None;
    }
    let le = if &data[0..4] == b"II*\0" {
        true
    } else if &data[0..4] == b"MM\0*" {
        false
    } else {
        return None;
    };
    let e = TiffEndian { le };
    let off = e.u32(&data[4..8]) as usize;
    let ifd = parse_ifd_at(data, &e, off)?;
    Some((e, ifd))
}

fn is_supported_baseline_tiff(ifd: &IfdSummary) -> bool {
    let comp = ifd.compression.unwrap_or(1);
    let spp = ifd.samples_per_pixel.unwrap_or(1);
    let bps = ifd.bits_per_sample.first().copied().unwrap_or(8);
    let photo = ifd.photometric.unwrap_or(2);
    comp == 1
        && matches!(spp, 1 | 3 | 4)
        && matches!(bps, 8 | 16)
        && matches!(photo, 0..=2)
        && ifd.width.is_some_and(|w| w > 0)
        && ifd.height.is_some_and(|h| h > 0)
        && !ifd.strip_offsets.is_empty()
}

fn decode_baseline_tiff(path: &Path) -> Result<DynamicImage, String> {
    let data =
        read_tiff_prefix_or_all(path).ok_or_else(|| "TIFF 文件过大或无法读取".to_string())?;
    let (_, ifd) = parse_tiff_ifd0(&data).ok_or_else(|| "无法解析 TIFF 文件头".to_string())?;
    if !is_supported_baseline_tiff(&ifd) {
        return Err("非未压缩基线 TIFF".to_string());
    }
    let w = ifd.width.unwrap_or(0);
    let h = ifd.height.unwrap_or(0);
    let spp = ifd.samples_per_pixel.unwrap_or(1) as usize;
    let bps = ifd.bits_per_sample.first().copied().unwrap_or(8) as usize;
    let photo = ifd.photometric.unwrap_or(2);
    let bytes_per_sample = bps / 8;

    let mut pixels = Vec::with_capacity(
        (w as usize)
            .saturating_mul(h as usize)
            .saturating_mul(spp * bytes_per_sample),
    );
    for (idx, &offset) in ifd.strip_offsets.iter().enumerate() {
        let start = offset as usize;
        if start >= data.len() {
            break;
        }
        let chunk_len = ifd
            .strip_byte_counts
            .get(idx)
            .copied()
            .map(|c| c as usize)
            .unwrap_or_else(|| data.len().saturating_sub(start));
        let end = start.saturating_add(chunk_len).min(data.len());
        pixels.extend_from_slice(&data[start..end]);
    }

    let expected_pixels = (w as usize).saturating_mul(h as usize);
    let step = spp * bytes_per_sample;
    if pixels.len() < expected_pixels * step {
        return Err("TIFF 像素数据不完整".to_string());
    }
    let mut rgb = Vec::with_capacity(expected_pixels * 3);
    let is_le = &data[0..2] == b"II";
    let read_ch = |slice: &[u8]| -> u8 {
        if bytes_per_sample == 1 {
            slice[0]
        } else {
            let v = if is_le {
                u16::from_le_bytes([slice[0], slice[1]])
            } else {
                u16::from_be_bytes([slice[0], slice[1]])
            };
            (v >> 8) as u8
        }
    };

    for i in 0..expected_pixels {
        let base = i * step;
        if spp == 1 {
            let mut g = read_ch(&pixels[base..base + bytes_per_sample]);
            if photo == 0 {
                g = 255 - g;
            }
            rgb.extend_from_slice(&[g, g, g]);
        } else {
            let r = read_ch(&pixels[base..base + bytes_per_sample]);
            let g = read_ch(&pixels[base + bytes_per_sample..base + 2 * bytes_per_sample]);
            let b = read_ch(&pixels[base + 2 * bytes_per_sample..base + 3 * bytes_per_sample]);
            rgb.extend_from_slice(&[r, g, b]);
        }
    }
    let out = RgbImage::from_raw(w, h, rgb).ok_or_else(|| "构建 TIFF 图像失败".to_string())?;
    Ok(DynamicImage::ImageRgb8(out))
}

fn collect_ifd_jpeg(
    data: &[u8],
    ifd: &IfdSummary,
    best: &mut Option<(usize, usize)>,
    queue: &mut Vec<u32>,
) {
    let mut consider = |start: usize, end: usize| {
        if end <= data.len()
            && end > start + 128
            && data[start..end].starts_with(&[0xFF, 0xD8, 0xFF])
            && best.is_none_or(|(bs, be)| (end - start) > (be - bs))
        {
            *best = Some((start, end));
        }
    };
    if let (Some(off), Some(len)) = (ifd.jpeg_offset, ifd.jpeg_length) {
        let s = off as usize;
        consider(s, s.saturating_add(len as usize));
    }
    if matches!(ifd.compression, Some(6 | 7)) && ifd.strip_offsets.len() == 1 {
        let s = ifd.strip_offsets[0] as usize;
        let len = ifd.strip_byte_counts.first().copied().unwrap_or(0) as usize;
        if len > 0 {
            consider(s, s.saturating_add(len));
        }
    }
    queue.extend_from_slice(&ifd.sub_ifds);
    if ifd.next_ifd != 0 {
        queue.push(ifd.next_ifd);
    }
}

fn extract_embedded_jpeg(path: &Path) -> Option<Vec<u8>> {
    let data = read_tiff_prefix_or_all(path)?;
    let (e, ifd0) = parse_tiff_ifd0(&data)?;
    let mut best: Option<(usize, usize)> = None;
    let mut queue = Vec::new();
    let mut visited = std::collections::HashSet::new();
    collect_ifd_jpeg(&data, &ifd0, &mut best, &mut queue);
    while let Some(off) = queue.pop() {
        if !visited.insert(off) || visited.len() > 32 {
            continue;
        }
        if let Some(sub) = parse_ifd_at(&data, &e, off as usize) {
            collect_ifd_jpeg(&data, &sub, &mut best, &mut queue);
        }
    }
    best.map(|(s, e_idx)| data[s..e_idx].to_vec())
}

fn extract_embedded_jpeg_scan(path: &Path) -> Option<Vec<u8>> {
    let mut file = open_any(path).ok()?;
    let len = file.metadata().ok()?.len();
    // Bounded prefix read: embedded EXIF/preview JPEGs sit within the first ~12MB of RAW/HEIC files.
    let read_len = (len as usize).min(12 * 1024 * 1024);
    let mut buf = vec![0u8; read_len];
    let n = file.read(&mut buf).ok()?;
    buf.truncate(n);

    let mut best: Option<&[u8]> = None;
    let mut i = 16usize;
    while i + 4 < buf.len() {
        if buf[i] == 0xFF && buf[i + 1] == 0xD8 && buf[i + 2] == 0xFF {
            let mut j = i + 3;
            while j + 1 < buf.len() {
                if buf[j] == 0xFF && buf[j + 1] == 0xD9 {
                    let candidate = &buf[i..=j + 1];
                    if candidate.len() >= 1024 && best.is_none_or(|cur| candidate.len() > cur.len())
                    {
                        best = Some(candidate);
                    }
                    i = j + 2;
                    break;
                }
                j += 1;
            }
            if j + 1 >= buf.len() {
                break;
            }
        } else {
            i += 1;
        }
    }
    best.map(|s| s.to_vec())
}

/* ==================== Windows Imaging Component (WIC) Decoder ==================== */

#[cfg(windows)]
fn wic_path_wide(path: &Path) -> Vec<u16> {
    let clean = crate::util::plain_path(path);
    let use_path = if clean.to_string_lossy().encode_utf16().count() < 250 {
        clean
    } else {
        win_extended(&clean)
    };
    use std::os::windows::ffi::OsStrExt;
    use_path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(windows)]
struct ComGuard {
    should_uninit: bool,
}

#[cfg(windows)]
impl ComGuard {
    fn init() -> Result<Self, ()> {
        use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if hr.0 == 0 || hr.0 == 1 {
            Ok(Self {
                should_uninit: true,
            })
        } else if hr.0 == 0x80010106u32 as i32 {
            // RPC_E_CHANGED_MODE: thread already initialized (e.g. STA), safe to use without uninit.
            Ok(Self {
                should_uninit: false,
            })
        } else {
            Err(())
        }
    }
}

#[cfg(windows)]
impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.should_uninit {
            unsafe {
                windows::Win32::System::Com::CoUninitialize();
            }
        }
    }
}

#[cfg(windows)]
fn wic_probe_dimensions(path: &Path) -> Option<(u32, u32)> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::GENERIC_READ;
    use windows::Win32::Graphics::Imaging::{
        CLSID_WICImagingFactory, IWICImagingFactory, WICDecodeMetadataCacheOnDemand,
    };
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};

    if fs::metadata(win_extended(path)).is_err() && fs::metadata(path).is_err() {
        return None;
    }
    let _com = ComGuard::init().ok()?;
    unsafe {
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let wide = wic_path_wide(path);
        let decoder = factory
            .CreateDecoderFromFilename(
                PCWSTR(wide.as_ptr()),
                None,
                GENERIC_READ,
                WICDecodeMetadataCacheOnDemand,
            )
            .ok()?;
        let frame = decoder.GetFrame(0).ok()?;
        let mut w = 0u32;
        let mut h = 0u32;
        frame.GetSize(&mut w, &mut h).ok()?;
        if w > 0 && h > 0 {
            Some((w, h))
        } else {
            None
        }
    }
}

#[cfg(not(windows))]
fn wic_probe_dimensions(_path: &Path) -> Option<(u32, u32)> {
    None
}

#[cfg(windows)]
fn wic_decode_scaled(path: &Path, max_edge: u32) -> Option<DynamicImage> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::GENERIC_READ;
    use windows::Win32::Graphics::Imaging::{
        CLSID_WICImagingFactory, GUID_WICPixelFormat24bppRGB, IWICBitmapSource, IWICImagingFactory,
        WICBitmapDitherTypeNone, WICBitmapInterpolationModeFant, WICBitmapPaletteTypeMedianCut,
        WICDecodeMetadataCacheOnDemand,
    };
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};

    let _com = ComGuard::init().ok()?;
    unsafe {
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let wide = wic_path_wide(path);
        let decoder = factory
            .CreateDecoderFromFilename(
                PCWSTR(wide.as_ptr()),
                None,
                GENERIC_READ,
                WICDecodeMetadataCacheOnDemand,
            )
            .ok()?;
        let frame = decoder.GetFrame(0).ok()?;
        let mut src_w = 0u32;
        let mut src_h = 0u32;
        frame.GetSize(&mut src_w, &mut src_h).ok()?;
        if src_w == 0 || src_h == 0 {
            return None;
        }

        let (dst_w, dst_h) = if src_w > max_edge || src_h > max_edge {
            if src_w >= src_h {
                let h = ((src_h as f64) * (max_edge as f64) / (src_w as f64))
                    .round()
                    .max(1.0) as u32;
                (max_edge.max(1), h)
            } else {
                let w = ((src_w as f64) * (max_edge as f64) / (src_h as f64))
                    .round()
                    .max(1.0) as u32;
                (w, max_edge.max(1))
            }
        } else {
            (src_w, src_h)
        };

        let frame_src: IWICBitmapSource = windows::core::Interface::cast(&frame).ok()?;
        let scaled_src: IWICBitmapSource = if dst_w != src_w || dst_h != src_h {
            let scaler = factory.CreateBitmapScaler().ok()?;
            scaler
                .Initialize(&frame_src, dst_w, dst_h, WICBitmapInterpolationModeFant)
                .ok()?;
            windows::core::Interface::cast(&scaler).ok()?
        } else {
            frame_src
        };

        let converter = factory.CreateFormatConverter().ok()?;
        converter
            .Initialize(
                &scaled_src,
                &GUID_WICPixelFormat24bppRGB,
                WICBitmapDitherTypeNone,
                None,
                0.0,
                WICBitmapPaletteTypeMedianCut,
            )
            .ok()?;
        let conv_src: IWICBitmapSource = windows::core::Interface::cast(&converter).ok()?;
        let stride = dst_w.checked_mul(3)?;
        let total = stride.checked_mul(dst_h)? as usize;
        let mut buf = vec![0u8; total];
        conv_src
            .CopyPixels(std::ptr::null(), stride, &mut buf)
            .ok()?;
        let rgb = RgbImage::from_raw(dst_w, dst_h, buf)?;
        Some(DynamicImage::ImageRgb8(rgb))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "tuxu-codec-{}-{}-{}",
            tag,
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn write_uncompressed_rgb_tiff(path: &Path, w: u32, h: u32, rgb: &[u8]) {
        assert_eq!(rgb.len(), (w * h * 3) as usize);
        let mut out = Vec::new();
        out.extend_from_slice(b"II*\0");
        out.extend_from_slice(&8u32.to_le_bytes()); // IFD0 offset
        let num_entries: u16 = 8;
        out.extend_from_slice(&num_entries.to_le_bytes());
        let data_start = 8u32 + 2 + (num_entries as u32) * 12 + 4;
        let bps_offset = data_start;
        let pixel_offset = data_start + 6;
        let entries: [(u16, u16, u32, u32); 8] = [
            (0x0100, 4, 1, w),
            (0x0101, 4, 1, h),
            (0x0102, 3, 3, bps_offset),
            (0x0103, 3, 1, 1), // uncompressed
            (0x0106, 3, 1, 2), // RGB
            (0x0111, 4, 1, pixel_offset),
            (0x0115, 3, 1, 3), // 3 channels
            (0x0117, 4, 1, rgb.len() as u32),
        ];
        for (tag, typ, cnt, val) in entries {
            out.extend_from_slice(&tag.to_le_bytes());
            out.extend_from_slice(&typ.to_le_bytes());
            out.extend_from_slice(&cnt.to_le_bytes());
            out.extend_from_slice(&val.to_le_bytes());
        }
        out.extend_from_slice(&0u32.to_le_bytes()); // next IFD
        for _ in 0..3 {
            out.extend_from_slice(&8u16.to_le_bytes());
        }
        out.extend_from_slice(rgb);
        fs::write(path, out).unwrap();
    }

    #[test]
    fn decodes_bmp_and_baseline_tiff_and_embedded_dng_preview() {
        let dir = tmp_dir("formats");
        let img = RgbImage::from_fn(48, 36, |x, y| {
            image::Rgb([(x * 5) as u8, (y * 6) as u8, ((x + y) * 2) as u8])
        });
        let dyn_img = DynamicImage::ImageRgb8(img.clone());

        let bmp_path = dir.join("sample.bmp");
        dyn_img
            .save_with_format(&bmp_path, image::ImageFormat::Bmp)
            .unwrap();
        let probe_bmp = probe_photo(&bmp_path, PhotoKind::Bmp);
        assert!(probe_bmp.preview_available);
        assert_eq!(probe_bmp.dimensions, Some((48, 36)));
        assert_eq!(decode_preview_image(&bmp_path, 128).unwrap().width(), 48);

        let tiff_path = dir.join("sample.tiff");
        write_uncompressed_rgb_tiff(&tiff_path, 48, 36, img.as_raw());
        let probe_tiff = probe_photo(&tiff_path, PhotoKind::Tiff);
        assert!(probe_tiff.preview_available);
        assert_eq!(probe_tiff.dimensions, Some((48, 36)));
        let dec_tiff = decode_preview_image(&tiff_path, 128).unwrap();
        assert_eq!((dec_tiff.width(), dec_tiff.height()), (48, 36));

        // Simulated DNG containing JPEGInterchangeFormat thumbnail in IFD0
        let mut jpeg_buf = Vec::new();
        dyn_img
            .write_to(
                &mut std::io::Cursor::new(&mut jpeg_buf),
                image::ImageFormat::Jpeg,
            )
            .unwrap();
        let mut dng = Vec::new();
        dng.extend_from_slice(b"II*\0");
        dng.extend_from_slice(&8u32.to_le_bytes());
        dng.extend_from_slice(&4u16.to_le_bytes());
        let jpg_off = 8u32 + 2 + 4 * 12 + 4;
        for (tag, typ, cnt, val) in [
            (0x0100u16, 4u16, 1u32, 4000u32),
            (0x0101u16, 4u16, 1u32, 3000u32),
            (0x0201u16, 4u16, 1u32, jpg_off),
            (0x0202u16, 4u16, 1u32, jpeg_buf.len() as u32),
        ] {
            dng.extend_from_slice(&tag.to_le_bytes());
            dng.extend_from_slice(&typ.to_le_bytes());
            out_extend(&mut dng, cnt, val);
        }
        dng.extend_from_slice(&0u32.to_le_bytes());
        dng.extend_from_slice(&jpeg_buf);
        let dng_path = dir.join("drone.DNG");
        fs::write(&dng_path, dng).unwrap();
        let probe_dng = probe_photo(&dng_path, PhotoKind::Raw);
        assert!(probe_dng.preview_available);
        assert_eq!(probe_dng.dimensions, Some((4000, 3000)));
        assert!(decode_preview_image(&dng_path, 220).is_ok());

        let _ = fs::remove_dir_all(dir);
    }

    fn out_extend(out: &mut Vec<u8>, cnt: u32, val: u32) {
        out.extend_from_slice(&cnt.to_le_bytes());
        out.extend_from_slice(&val.to_le_bytes());
    }

    #[test]
    fn analyzes_blur_exposure_contrast_and_truncation() {
        let dir = tmp_dir("quality");

        // 1. Sharp checkerboard vs uniform low contrast & dark vs bright
        let sharp = DynamicImage::ImageRgb8(RgbImage::from_fn(240, 240, |x, y| {
            let v = if ((x / 4) + (y / 4)) % 2 == 0 {
                220
            } else {
                30
            };
            image::Rgb([v, v, v])
        }));
        let m_sharp = analyze_image_pixels(&sharp, Some((1920, 1080)));
        assert!(!m_sharp.blur);
        assert!(!m_sharp.low_contrast);
        assert!(!m_sharp.underexposed);
        assert!(!m_sharp.overexposed);

        let dark = DynamicImage::ImageRgb8(RgbImage::from_fn(128, 128, |x, y| {
            let d = if (x + y) % 2 == 0 { 8 } else { 35 };
            image::Rgb([d, d, d])
        }));
        let m_dark = analyze_image_pixels(&dark, Some((1920, 1080)));
        assert!(m_dark.underexposed);

        let flat =
            DynamicImage::ImageRgb8(RgbImage::from_pixel(128, 128, image::Rgb([128, 128, 128])));
        let m_flat = analyze_image_pixels(&flat, Some((1920, 1080)));
        assert!(m_flat.low_contrast);

        // Soft gradient image with enough stddev to trigger blur detection
        let blurry = DynamicImage::ImageRgb8(RgbImage::from_fn(160, 160, |x, _y| {
            let v = (40 + x.min(180)) as u8;
            image::Rgb([v, v, v])
        }));
        let m_blur = analyze_image_pixels(&blurry, Some((1920, 1080)));
        assert!(m_blur.blur, "expected blur, sharpness={}", m_blur.sharpness);

        // Truncated JPEG check (no FF D9 trailer)
        let mut trunc = Vec::new();
        sharp
            .write_to(
                &mut std::io::Cursor::new(&mut trunc),
                image::ImageFormat::Jpeg,
            )
            .unwrap();
        trunc.truncate(trunc.len().saturating_sub(8));
        let trunc_path = dir.join("cut.jpg");
        fs::write(&trunc_path, trunc).unwrap();
        let probe_trunc = probe_photo(&trunc_path, PhotoKind::Jpeg);
        assert!(probe_trunc.truncated);
        assert_eq!(probe_trunc.quality.status, "bad");
        assert!(probe_trunc
            .quality
            .issues
            .contains(&"truncated".to_string()));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn distinguishes_corrupt_files_from_valid_container_without_decoder() {
        let dir = tmp_dir("c3-corrupt-vs-unsupported");

        // Garbage bytes disguised as HEIC / DNG / BMP / WEBP / PNG -> true corrupt
        for (name, kind) in [
            ("bad.heic", PhotoKind::Heic),
            ("bad.dng", PhotoKind::Raw),
            ("bad.bmp", PhotoKind::Bmp),
            ("bad.webp", PhotoKind::Webp),
            ("bad.png", PhotoKind::Png),
        ] {
            let p = dir.join(name);
            fs::write(&p, b"not a valid image container payload").unwrap();
            let out = probe_photo(&p, kind);
            assert!(out.corrupt, "{name} should be corrupt");
            assert!(
                !out.preview_available,
                "{name} must not report preview_available"
            );
            assert!(out.quality.issues.contains(&"corrupt".to_string()));
        }

        // Valid ISO BMFF CR3 RAW container without embedded JPEG -> not corrupt, unsupported_preview
        let mut valid_cr3 = Vec::new();
        valid_cr3.extend_from_slice(&24u32.to_be_bytes());
        valid_cr3.extend_from_slice(b"ftypcrx ");
        valid_cr3.extend_from_slice(&0u32.to_be_bytes());
        valid_cr3.extend_from_slice(b"crx isom");
        valid_cr3.extend_from_slice(&[0u8; 64]);
        let cr3_path = dir.join("valid.cr3");
        fs::write(&cr3_path, &valid_cr3).unwrap();
        let out_cr3 = probe_photo(&cr3_path, PhotoKind::Raw);
        assert!(!out_cr3.corrupt);
        assert!(!out_cr3.preview_available);
        assert!(out_cr3
            .quality
            .issues
            .contains(&"unsupported_preview".to_string()));

        let _ = fs::remove_dir_all(dir);
    }
}
