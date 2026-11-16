use crate::error::{err, Result};
use crate::model::{dir_pattern_for_layout, NamingRule, Photo, TagItem, Task};
use crate::names::{sanitize_segment, validate_component};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct ResolvedTarget {
    pub dir_segments: Vec<String>,
    pub file_stem: String,
    pub file_name: String,
    pub relative_path: String,
    pub group_name: String,
}

fn extract_date_parts(photo: &Photo) -> (String, String, String, String, String, String) {
    let raw = photo
        .capture_text
        .as_deref()
        .or(photo.sort_time_text.as_deref())
        .unwrap_or("");
    let digits: Vec<char> = raw.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() >= 14 {
        let y: String = digits[0..4].iter().collect();
        let m: String = digits[4..6].iter().collect();
        let d: String = digits[6..8].iter().collect();
        let hh: String = digits[8..10].iter().collect();
        let mm: String = digits[10..12].iter().collect();
        let ss: String = digits[12..14].iter().collect();
        return (y, m, d, hh, mm, ss);
    }
    if digits.len() >= 8 {
        let y: String = digits[0..4].iter().collect();
        let m: String = digits[4..6].iter().collect();
        let d: String = digits[6..8].iter().collect();
        return (y, m, d, "00".into(), "00".into(), "00".into());
    }
    (
        "0000".into(),
        "00".into(),
        "00".into(),
        "00".into(),
        "00".into(),
        "00".into(),
    )
}

pub fn format_photo_date(photo: &Photo, format: &str) -> String {
    let (y, m, d, hh, mm, ss) = extract_date_parts(photo);
    if y == "0000" {
        return "未知日期".into();
    }
    match format {
        "none" => String::new(),
        "YYYY-MM-DD" => format!("{y}-{m}-{d}"),
        "YYYYMMDD_HHMMSS" => format!("{y}{m}{d}_{hh}{mm}{ss}"),
        "YYYY-MM" => format!("{y}-{m}"),
        _ => format!("{y}{m}{d}"),
    }
}

pub fn effective_pattern(rule: &NamingRule) -> String {
    match rule.preset.as_str() {
        "custom" => rule.pattern.clone(),
        "date_group_seq" => {
            let sep = if rule.separator.is_empty() {
                "_"
            } else {
                rule.separator.as_str()
            };
            let mut parts: Vec<String> = Vec::new();
            if !rule.prefix.trim().is_empty() {
                parts.push("{prefix}".into());
            }
            if rule.date_format != "none" {
                parts.push("{date}".into());
            }
            if rule.include_group {
                parts.push("{group}".into());
            }
            if rule.include_tag {
                parts.push("{tag}".into());
            }
            if rule.include_original {
                parts.push("{orig}".into());
            }
            if rule.seq_scope != "none" {
                parts.push("{seq}".into());
            }
            if !rule.suffix.trim().is_empty() {
                parts.push("{suffix}".into());
            }
            if parts.is_empty() {
                "{orig}".into()
            } else {
                parts.join(sep)
            }
        }
        "group_seq" => {
            let sep = if rule.separator.is_empty() {
                "_"
            } else {
                rule.separator.as_str()
            };
            format!("{{prefix}}{{group}}{sep}{{seq}}{{suffix}}")
        }
        "prefix_seq" => "{prefix}{seq}{suffix}".into(),
        "orig_group" => {
            let sep = if rule.separator.is_empty() {
                "_"
            } else {
                rule.separator.as_str()
            };
            format!("{{prefix}}{{orig}}{sep}{{group}}{{suffix}}")
        }
        "tag_seq" => {
            let sep = if rule.separator.is_empty() {
                "_"
            } else {
                rule.separator.as_str()
            };
            format!("{{prefix}}{{tag}}{sep}{{seq}}{{suffix}}")
        }
        _ => rule.pattern.clone(),
    }
}

fn orig_stem(file_name: &str) -> String {
    match file_name.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem.to_string(),
        _ => file_name.to_string(),
    }
}

fn collapse_separators(raw: &str, sep: &str) -> String {
    let mut s = raw.trim().to_string();
    if !sep.is_empty() {
        let double = format!("{sep}{sep}");
        while s.contains(&double) {
            s = s.replace(&double, sep);
        }
        s = s.trim_matches(|c: char| sep.contains(c) || c == ' ').to_string();
    }
    for ch in ['_', '-'] {
        let d = format!("{ch}{ch}");
        while s.contains(&d) {
            s = s.replace(&d, &ch.to_string());
        }
    }
    s.trim_matches(|c: char| matches!(c, '_' | '-' | ' ')).to_string()
}

pub fn compute_seq_numbers(task: &Task) -> HashMap<String, u32> {
    let mut active: Vec<&Photo> = task.photos.iter().filter(|p| !p.excluded).collect();
    active.sort_by_key(|p| p.sort_index);
    let mut seq_map = HashMap::with_capacity(active.len());
    let start = task.naming_rule.seq_start.max(1);
    if task.naming_rule.seq_scope == "global" {
        for (idx, photo) in active.iter().enumerate() {
            seq_map.insert(photo.id.clone(), start + idx as u32);
        }
    } else {
        let mut per_group_counter: HashMap<String, u32> = HashMap::new();
        for photo in active {
            let key = photo.group_id.clone().unwrap_or_default();
            let counter = per_group_counter.entry(key).or_insert(0);
            seq_map.insert(photo.id.clone(), start + *counter);
            *counter += 1;
        }
    }
    seq_map
}

pub fn resolve_photo_target(
    task: &Task,
    photo: &Photo,
    seq_num: u32,
    tags: &[TagItem],
) -> Result<ResolvedTarget> {
    let group_name = photo
        .group_id
        .as_ref()
        .and_then(|gid| task.groups.iter().find(|g| &g.id == gid))
        .map(|g| g.name.clone())
        .unwrap_or_else(|| "未分类".into());

    let tag_name = photo
        .tag_id
        .as_ref()
        .and_then(|tid| tags.iter().find(|t| &t.id == tid))
        .map(|t| t.name.clone())
        .unwrap_or_default();

    let (y, m, d, hh, mm, ss) = extract_date_parts(photo);
    let date_ymd = if y == "0000" {
        "未知日期".to_string()
    } else {
        format!("{y}-{m}-{d}")
    };
    let month_ym = if y == "0000" {
        "未知日期".to_string()
    } else {
        format!("{y}-{m}")
    };

    let dir_pat = dir_pattern_for_layout(&task.dir_layout, &task.dir_pattern);
    let mut dir_segments = Vec::new();
    for raw_seg in dir_pat.split(['/', '\\']) {
        let seg_tmpl = raw_seg.trim();
        if seg_tmpl.is_empty() {
            continue;
        }
        let expanded = seg_tmpl
            .replace("{task}", &task.task_name)
            .replace("{group}", &group_name)
            .replace("{date}", &date_ymd)
            .replace("{year}", &y)
            .replace("{month}", &month_ym)
            .replace("{day}", &d);
        let cleaned = expanded.trim();
        if cleaned.is_empty() {
            continue;
        }
        validate_component(cleaned, "目录名称")?;
        dir_segments.push(cleaned.to_string());
    }

    let file_stem = if let Some(custom) = photo
        .custom_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        validate_component(custom, "自定义文件名")?;
        custom.to_string()
    } else {
        let rule = &task.naming_rule;
        let pattern = effective_pattern(rule);
        let digits = rule.seq_digits.clamp(1, 6) as usize;
        let seq_str = if rule.seq_scope == "none" {
            String::new()
        } else {
            format!("{seq_num:0width$}", width = digits)
        };
        let date_str = format_photo_date(photo, &rule.date_format);
        let datetime_str = if y == "0000" {
            "未知时间".into()
        } else {
            format!("{y}{m}{d}_{hh}{mm}{ss}")
        };
        let orig = orig_stem(&photo.file_name);

        let mut built = pattern
            .replace("{prefix}", rule.prefix.trim())
            .replace("{suffix}", rule.suffix.trim())
            .replace("{task}", task.task_name.trim())
            .replace("{group}", &group_name)
            .replace("{tag}", &tag_name)
            .replace("{date}", &date_str)
            .replace("{datetime}", &datetime_str)
            .replace("{year}", &y)
            .replace("{month}", &month_ym)
            .replace("{day}", &d)
            .replace("{seq}", &seq_str)
            .replace("{orig}", &orig);

        built = collapse_separators(&built, &rule.separator);
        if built.is_empty() {
            built = orig;
        }
        if validate_component(&built, "目标文件名").is_err() {
            built = sanitize_segment(&built);
        }
        validate_component(&built, "目标文件名")?;
        built
    };

    if file_stem.is_empty() {
        return Err(err("E005", "生成的文件名不能为空"));
    }

    let file_name = format!("{file_stem}{}", photo.extension);
    let mut rel_parts = dir_segments.clone();
    rel_parts.push(file_name.clone());
    let relative_path = rel_parts.join("\\");

    Ok(ResolvedTarget {
        dir_segments,
        file_stem,
        file_name,
        relative_path,
        group_name,
    })
}

pub fn sample_preview(task: &Task, tags: &[TagItem]) -> String {
    if let Some(photo) = task.photos.iter().find(|p| !p.excluded) {
        let seq_map = compute_seq_numbers(task);
        let seq = seq_map.get(&photo.id).copied().unwrap_or(1);
        if let Ok(resolved) = resolve_photo_target(task, photo, seq, tags) {
            return resolved.relative_path;
        }
    }
    let dummy = Photo {
        id: "sample".into(),
        source_path: "IMG_0001.jpg".into(),
        file_name: "IMG_0001.jpg".into(),
        extension: ".jpg".into(),
        size: 1024,
        mtime_ns: 0,
        capture_text: Some("2026:05:01 10:20:30".into()),
        capture_sort_key: None,
        exif_offset: None,
        sort_time_text: None,
        time_uncertain: false,
        time_source: "exif_original".into(),
        time_note: String::new(),
        equal_timestamp: false,
        tie_break: "path".into(),
        tie_break_is_capture_order: false,
        corrupt: false,
        preview_state: "available".into(),
        quality: Default::default(),
        sort_index: 0,
        original_scan_index: 0,
        group_id: task.groups.first().map(|g| g.id.clone()),
        tag_id: None,
        custom_name: None,
        confirmed: true,
        needs_review: false,
        excluded: false,
    };
    let mut sample_task = task.clone();
    if sample_task.task_name.trim().is_empty() {
        sample_task.task_name = "示例归档任务".into();
    }
    if sample_task.groups.is_empty() {
        sample_task.groups.push(crate::model::Group {
            id: "g-sample".into(),
            name: "风景".into(),
            sort_order: 0,
            preset_id: None,
            preset_name: String::new(),
            suggested_tags: Vec::new(),
        });
    }
    let mut dummy_with_group = dummy;
    dummy_with_group.group_id = sample_task.groups.first().map(|g| g.id.clone());
    resolve_photo_target(&sample_task, &dummy_with_group, 1, tags)
        .map(|r| r.relative_path)
        .unwrap_or_else(|_| "示例归档任务\\风景\\20260501_风景_001.jpg".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Group, NamingRule, Task};
    use std::collections::BTreeMap;

    fn base_task() -> Task {
        Task {
            id: "t1".into(),
            source_root: "D:\\src".into(),
            recursive: false,
            scan_complete: true,
            task_name: "春日旅行".into(),
            output_root: "D:\\out".into(),
            dir_layout: "task_and_group".into(),
            dir_pattern: "{task}/{group}".into(),
            naming_rule: NamingRule::default(),
            strict_hash: false,
            status: "editing".into(),
            mappings_valid: true,
            revision: 1,
            photos: vec![],
            groups: vec![Group {
                id: "g1".into(),
                name: "沿途风景".into(),
                sort_order: 0,
                preset_id: None,
                preset_name: String::new(),
                suggested_tags: Vec::new(),
            }],
            ignored_count: 0,
            ignored_reasons: BTreeMap::new(),
            favorite_tag_ids: vec![],
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    fn make_photo(id: &str, idx: u32, name: &str, group: Option<&str>) -> Photo {
        Photo {
            id: id.into(),
            source_path: format!("D:\\src\\{name}"),
            file_name: name.into(),
            extension: ".jpg".into(),
            size: 100,
            mtime_ns: 0,
            capture_text: Some("2026:05:01 09:30:00".into()),
            capture_sort_key: None,
            exif_offset: None,
            sort_time_text: None,
            time_uncertain: false,
            time_source: "exif_original".into(),
            time_note: String::new(),
            equal_timestamp: false,
            tie_break: "path".into(),
            tie_break_is_capture_order: false,
            corrupt: false,
            preview_state: "available".into(),
            quality: Default::default(),
            sort_index: idx,
            original_scan_index: idx,
            group_id: group.map(str::to_string),
            tag_id: None,
            custom_name: None,
            confirmed: true,
            needs_review: false,
            excluded: false,
        }
    }

    #[test]
    fn allows_multiple_photos_in_same_group_with_sequential_numbering() {
        let mut task = base_task();
        task.photos = vec![
            make_photo("p1", 0, "DSC_001.jpg", Some("g1")),
            make_photo("p2", 1, "DSC_002.jpg", Some("g1")),
        ];
        let seqs = compute_seq_numbers(&task);
        let r1 = resolve_photo_target(&task, &task.photos[0], seqs["p1"], &[]).unwrap();
        let r2 = resolve_photo_target(&task, &task.photos[1], seqs["p2"], &[]).unwrap();
        assert_eq!(r1.relative_path, "春日旅行\\沿途风景\\20260501_沿途风景_001.jpg");
        assert_eq!(r2.relative_path, "春日旅行\\沿途风景\\20260501_沿途风景_002.jpg");
    }

    #[test]
    fn supports_flexible_directory_hierarchies_and_custom_patterns() {
        let mut task = base_task();
        task.dir_layout = "date_and_group".into();
        task.naming_rule.preset = "custom".into();
        task.naming_rule.prefix = "TRIP_".into();
        task.naming_rule.suffix = "_v1".into();
        task.naming_rule.pattern = "{prefix}{orig}_{seq}{suffix}".into();
        task.naming_rule.seq_digits = 2;
        task.photos = vec![make_photo("p1", 0, "IMG_8899.jpg", Some("g1"))];

        let r = resolve_photo_target(&task, &task.photos[0], 1, &[]).unwrap();
        assert_eq!(r.relative_path, "2026-05\\沿途风景\\TRIP_IMG_8899_01_v1.jpg");

        task.dir_layout = "flat".into();
        let r_flat = resolve_photo_target(&task, &task.photos[0], 1, &[]).unwrap();
        assert_eq!(r_flat.relative_path, "TRIP_IMG_8899_01_v1.jpg");
    }
}
