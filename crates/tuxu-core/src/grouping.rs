use crate::error::{err, Result};
use crate::model::{Group, GroupPreset, Photo, Task};
use crate::names::validate_component;
use crate::util::new_id;
use std::collections::{BTreeMap, HashMap};

fn photo_day_key(photo: &Photo) -> String {
    let raw = photo
        .capture_text
        .as_deref()
        .or(photo.sort_time_text.as_deref())
        .unwrap_or("");
    let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() >= 8 {
        format!("{}-{}-{}", &digits[0..4], &digits[4..6], &digits[6..8])
    } else {
        "未知日期".into()
    }
}

fn photo_month_key(photo: &Photo) -> String {
    let raw = photo
        .capture_text
        .as_deref()
        .or(photo.sort_time_text.as_deref())
        .unwrap_or("");
    let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() >= 6 {
        format!("{}-{}", &digits[0..4], &digits[4..6])
    } else {
        "未知月份".into()
    }
}

fn sort_key_epoch_secs(photo: &Photo) -> Option<i64> {
    if let Some(ms) = photo.capture_sort_key {
        return Some(ms.div_euclid(1000));
    }
    let text = photo.capture_text.as_deref()?;
    chrono::NaiveDateTime::parse_from_str(text, "%Y:%m:%d %H:%M:%S")
        .or_else(|_| chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S"))
        .ok()
        .map(|naive| naive.and_utc().timestamp())
}

fn active_indices_sorted(task: &Task) -> Vec<usize> {
    let mut indices: Vec<usize> = task
        .photos
        .iter()
        .enumerate()
        .filter(|(_, p)| !p.excluded)
        .map(|(i, _)| i)
        .collect();
    indices.sort_by_key(|&i| task.photos[i].sort_index);
    indices
}

fn mark_photo_group(photo: &mut Photo, group_id: String) {
    photo.group_id = Some(group_id);
    if !photo.corrupt && !photo.needs_review {
        photo.confirmed = true;
    }
}

pub fn group_by_time_bucket(task: &mut Task, granularity: &str) -> Result<usize> {
    let indices = active_indices_sorted(task);
    if indices.is_empty() {
        return Err(err("", "当前没有可分组的有效图片"));
    }
    let mut bucket_order: Vec<String> = Vec::new();
    let mut bucket_map: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for idx in indices {
        let key = if granularity == "month" {
            photo_month_key(&task.photos[idx])
        } else {
            photo_day_key(&task.photos[idx])
        };
        if !bucket_map.contains_key(&key) {
            bucket_order.push(key.clone());
        }
        bucket_map.entry(key).or_default().push(idx);
    }

    let mut groups = Vec::with_capacity(bucket_order.len());
    for (order, name) in bucket_order.iter().enumerate() {
        validate_component(name, "分组名称")?;
        let gid = new_id("grp");
        if let Some(photo_idxs) = bucket_map.get(name) {
            for &pi in photo_idxs {
                mark_photo_group(&mut task.photos[pi], gid.clone());
            }
        }
        groups.push(Group {
            id: gid,
            name: name.clone(),
            sort_order: order as u32,
            preset_id: None,
            preset_name: String::new(),
            suggested_tags: Vec::new(),
        });
    }
    let count = groups.len();
    task.groups = groups;
    Ok(count)
}

pub fn group_by_time_gap(task: &mut Task, gap_minutes: u32, prefix: &str) -> Result<usize> {
    let gap_secs = i64::from(gap_minutes.max(1)) * 60;
    let prefix = if prefix.trim().is_empty() {
        "批次"
    } else {
        prefix.trim()
    };
    let indices = active_indices_sorted(task);
    if indices.is_empty() {
        return Err(err("", "当前没有可分组的有效图片"));
    }

    let mut clusters: Vec<Vec<usize>> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    let mut prev_ts: Option<i64> = None;

    for idx in indices {
        let ts = sort_key_epoch_secs(&task.photos[idx]);
        let start_new = match (prev_ts, ts) {
            (Some(p), Some(c)) => (c - p).abs() > gap_secs,
            _ => false,
        };
        if start_new && !current.is_empty() {
            clusters.push(current);
            current = Vec::new();
        }
        current.push(idx);
        if ts.is_some() {
            prev_ts = ts;
        }
    }
    if !current.is_empty() {
        clusters.push(current);
    }

    let width = if clusters.len() >= 100 { 3 } else { 2 };
    let mut groups = Vec::with_capacity(clusters.len());
    for (order, cluster) in clusters.into_iter().enumerate() {
        let name = format!("{prefix}{:0width$}", order + 1, width = width);
        validate_component(&name, "分组名称")?;
        let gid = new_id("grp");
        for pi in cluster {
            mark_photo_group(&mut task.photos[pi], gid.clone());
        }
        groups.push(Group {
            id: gid,
            name,
            sort_order: order as u32,
            preset_id: None,
            preset_name: String::new(),
            suggested_tags: Vec::new(),
        });
    }
    let count = groups.len();
    task.groups = groups;
    Ok(count)
}

pub fn group_by_fixed_size(task: &mut Task, per_group: u32, prefix: &str) -> Result<usize> {
    if per_group == 0 {
        return Err(err("", "每组图片数量必须大于 0"));
    }
    let prefix = if prefix.trim().is_empty() {
        "分组"
    } else {
        prefix.trim()
    };
    let indices = active_indices_sorted(task);
    if indices.is_empty() {
        return Err(err("", "当前没有可分组的有效图片"));
    }
    let chunks: Vec<&[usize]> = indices.chunks(per_group as usize).collect();
    let width = if chunks.len() >= 100 { 3 } else { 2 };
    let mut groups = Vec::with_capacity(chunks.len());
    for (order, chunk) in chunks.into_iter().enumerate() {
        let name = format!("{prefix}{:0width$}", order + 1, width = width);
        validate_component(&name, "分组名称")?;
        let gid = new_id("grp");
        for &pi in chunk {
            mark_photo_group(&mut task.photos[pi], gid.clone());
        }
        groups.push(Group {
            id: gid,
            name,
            sort_order: order as u32,
            preset_id: None,
            preset_name: String::new(),
            suggested_tags: Vec::new(),
        });
    }
    let count = groups.len();
    task.groups = groups;
    Ok(count)
}

pub fn group_evenly_across_existing(task: &mut Task) -> Result<()> {
    if task.groups.is_empty() {
        return Err(err("", "请先添加至少一个分类"));
    }
    let indices = active_indices_sorted(task);
    let total = indices.len();
    let g_count = task.groups.len();
    let base = total / g_count;
    let rem = total % g_count;
    let mut cursor = 0usize;
    for (gi, group) in task.groups.iter().enumerate() {
        let take = base + usize::from(gi < rem);
        for &pi in &indices[cursor..cursor + take] {
            mark_photo_group(&mut task.photos[pi], group.id.clone());
        }
        cursor += take;
    }
    Ok(())
}

pub fn group_by_counts_across_existing(task: &mut Task, counts: &[u32]) -> Result<()> {
    if counts.len() != task.groups.len() {
        return Err(err("", "各分类数量个数与当前分类总数不一致"));
    }
    let indices = active_indices_sorted(task);
    let total_requested: usize = counts.iter().map(|&c| c as usize).sum();
    if total_requested > indices.len() {
        return Err(err(
            "",
            format!(
                "分配总数（{total_requested}）超过了有效图片数（{}）",
                indices.len()
            ),
        ));
    }
    let mut cursor = 0usize;
    for (gi, &cnt) in counts.iter().enumerate() {
        let gid = task.groups[gi].id.clone();
        for &pi in &indices[cursor..cursor + cnt as usize] {
            mark_photo_group(&mut task.photos[pi], gid.clone());
        }
        cursor += cnt as usize;
    }
    for &pi in &indices[cursor..] {
        task.photos[pi].group_id = None;
    }
    Ok(())
}

pub fn assign_photos_to_group(task: &mut Task, photo_ids: &[String], group_id: &str) -> Result<u32> {
    if !task.groups.iter().any(|g| g.id == group_id) {
        return Err(err("", "目标分类不存在"));
    }
    let mut map: HashMap<String, usize> = HashMap::new();
    for (i, p) in task.photos.iter().enumerate() {
        map.insert(p.id.clone(), i);
    }
    let mut changed = 0u32;
    for pid in photo_ids {
        if let Some(&idx) = map.get(pid) {
            if !task.photos[idx].excluded {
                mark_photo_group(&mut task.photos[idx], group_id.to_string());
                changed += 1;
            }
        }
    }
    Ok(changed)
}

pub fn apply_preset_groups(
    task: &mut Task,
    preset: &GroupPreset,
    distribute_evenly: bool,
    apply_naming: bool,
) -> Result<()> {
    let mut new_groups = Vec::new();
    for (order, gname) in preset.group_names.iter().enumerate() {
        validate_component(gname, "分类名称")?;
        new_groups.push(Group {
            id: new_id("grp"),
            name: gname.clone(),
            sort_order: order as u32,
            preset_id: Some(preset.id.clone()),
            preset_name: preset.name.clone(),
            suggested_tags: preset.items.clone(),
        });
    }
    if new_groups.is_empty() {
        return Err(err("", "所选方案不包含预设分类"));
    }
    let existing_by_name: HashMap<String, String> = task
        .groups
        .iter()
        .map(|g| (g.name.to_lowercase(), g.id.clone()))
        .collect();
    let new_by_name: HashMap<String, String> = new_groups
        .iter()
        .map(|g| (g.name.to_lowercase(), g.id.clone()))
        .collect();
    let old_to_new: HashMap<String, String> = existing_by_name
        .iter()
        .filter_map(|(name_key, old_id)| {
            new_by_name
                .get(name_key)
                .map(|new_id| (old_id.clone(), new_id.clone()))
        })
        .collect();

    for photo in &mut task.photos {
        if let Some(old_gid) = &photo.group_id {
            photo.group_id = old_to_new.get(old_gid).cloned();
        }
    }
    task.groups = new_groups;
    if apply_naming {
        if let Some(rule) = &preset.default_naming {
            task.naming_rule = rule.clone();
        }
    }
    if distribute_evenly {
        group_evenly_across_existing(task)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{NamingRule, Task};

    fn make_test_task(times: &[&str]) -> Task {
        let photos = times
            .iter()
            .enumerate()
            .map(|(i, ts)| Photo {
                id: format!("p{i}"),
                source_path: format!("D:\\src\\IMG_{i:03}.jpg"),
                file_name: format!("IMG_{i:03}.jpg"),
                extension: ".jpg".into(),
                size: 100,
                mtime_ns: 0,
                capture_text: Some((*ts).into()),
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
                sort_index: i as u32,
                original_scan_index: i as u32,
                group_id: None,
                tag_id: None,
                custom_name: None,
                confirmed: false,
                needs_review: false,
                excluded: false,
            })
            .collect();
        Task {
            id: "t1".into(),
            source_root: "D:\\src".into(),
            recursive: false,
            scan_complete: true,
            task_name: "测试整理".into(),
            output_root: "D:\\out".into(),
            dir_layout: "task_and_group".into(),
            dir_pattern: "{task}/{group}".into(),
            naming_rule: NamingRule::default(),
            strict_hash: false,
            status: "editing".into(),
            mappings_valid: true,
            revision: 1,
            photos,
            groups: vec![],
            ignored_count: 0,
            ignored_reasons: BTreeMap::new(),
            favorite_tag_ids: vec![],
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn groups_by_day_and_gap_and_fixed_size() {
        let mut task = make_test_task(&[
            "2026:05:01 09:00:00",
            "2026:05:01 09:10:00",
            "2026:05:01 14:00:00",
            "2026:05:02 10:00:00",
        ]);
        assert_eq!(group_by_time_bucket(&mut task, "day").unwrap(), 2);
        assert_eq!(task.groups[0].name, "2026-05-01");
        assert_eq!(task.groups[1].name, "2026-05-02");
        assert!(task.photos.iter().all(|p| p.group_id.is_some() && p.confirmed));

        assert_eq!(group_by_time_gap(&mut task, 60, "时段").unwrap(), 3);
        assert_eq!(task.groups.len(), 3);
        assert_eq!(task.photos[0].group_id, task.photos[1].group_id);
        assert_ne!(task.photos[1].group_id, task.photos[2].group_id);

        assert_eq!(group_by_fixed_size(&mut task, 2, "卷").unwrap(), 2);
        assert_eq!(task.groups[0].name, "卷01");
        assert_eq!(task.groups[1].name, "卷02");

        // Normalized UTC capture_sort_key overrides timezone-shifted wall capture_text
        task.photos[0].capture_sort_key = Some(1_777_626_000_000);
        task.photos[1].capture_text = Some("2026:05:01 17:00:30".into()); // +8h wall text
        task.photos[1].capture_sort_key = Some(1_777_626_030_000); // 30s later in UTC
        task.photos[2].capture_sort_key = Some(1_777_644_000_000); // +5h later
        task.photos[3].capture_sort_key = Some(1_777_716_000_000);
        assert_eq!(group_by_time_gap(&mut task, 60, "批次").unwrap(), 3);
        assert_eq!(task.photos[0].group_id, task.photos[1].group_id);
        assert_ne!(task.photos[1].group_id, task.photos[2].group_id);
    }
}
