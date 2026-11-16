use crate::archive;
use crate::codec::{analyze_image_pixels, decode_preview_image, merge_pixel_quality};
use crate::db;
use crate::error::{err, Result};
use crate::grouping;
use crate::model::*;
use crate::names::{natural_cmp, parse_group_batch, validate_component};
use crate::naming_engine::{compute_seq_numbers, resolve_photo_target, sample_preview};
use crate::scan;
use crate::thumbs::{self, ThumbResult};
use crate::util::{self, new_id, now_iso};
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

pub struct Organizer {
    db: Connection,
    db_path: PathBuf,
    data_dir: PathBuf,
    task: Option<Task>,
    plans: HashMap<String, ExecutionPlan>,
    undo: Vec<EditSnap>,
    redo: Vec<EditSnap>,
    free_override: Option<u64>,
}

pub struct PreparedExecution {
    db_path: PathBuf,
    task: Task,
    plan: ExecutionPlan,
    tags: Vec<TagItem>,
    free_override: Option<u64>,
}

pub enum ResultThumb {
    Jpeg(Vec<u8>),
    Unavailable { reason: String },
}

impl Organizer {
    pub fn open(data_dir: &Path) -> Result<Self> {
        let db_path = data_dir.join("tuxu.db");
        let db = db::open_db(&db_path)?;
        Ok(Self {
            db,
            db_path,
            data_dir: data_dir.to_path_buf(),
            task: None,
            plans: HashMap::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            free_override: None,
        })
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn database_open(&self) -> bool {
        self.db.execute_batch("SELECT 1").is_ok()
    }

    pub fn scan_folder(
        &mut self,
        path: &Path,
        opts: ScanOptions,
        replace: bool,
    ) -> Result<TaskDetail> {
        self.scan_folder_controlled(path, opts, replace, None, |_| ScanControl::Continue)
    }

    pub fn scan_folder_until(
        &mut self,
        path: &Path,
        opts: ScanOptions,
        max_photos: u32,
    ) -> Result<TaskDetail> {
        self.scan_folder_controlled(path, opts, true, Some(max_photos), |_| {
            ScanControl::Continue
        })
    }

    pub fn scan_folder_controlled(
        &mut self,
        path: &Path,
        opts: ScanOptions,
        replace: bool,
        max_photos: Option<u32>,
        on_tick: impl FnMut(&ScanTick) -> ScanControl,
    ) -> Result<TaskDetail> {
        let out = scan::scan_tree_controlled(path, opts.recursive, max_photos, on_tick)?;
        let out_root = self.remembered_output_root()?;
        let strict = self.settings()?.strict_hash;
        let default_task_name = path
            .file_name()
            .map(|n| n.to_string_lossy().trim().to_string())
            .filter(|s| !s.is_empty() && validate_component(s, "任务名称").is_ok())
            .unwrap_or_else(|| "图片归档任务".into());

        let mut task = if !replace {
            if let Some(mut existing) = self.task.take() {
                let old_by_path: HashMap<String, Photo> = existing
                    .photos
                    .into_iter()
                    .map(|p| (p.source_path.to_lowercase(), p))
                    .collect();
                let mut merged = Vec::with_capacity(out.photos.len());
                let mut changed = false;
                for mut photo in out.photos {
                    if let Some(prev) = old_by_path.get(&photo.source_path.to_lowercase()) {
                        photo.group_id = prev.group_id.clone();
                        photo.tag_id = prev.tag_id.clone();
                        photo.custom_name = prev.custom_name.clone();
                        photo.excluded = prev.excluded;
                        let same_file = prev.size == photo.size && prev.mtime_ns == photo.mtime_ns;
                        if same_file {
                            photo.confirmed = prev.confirmed;
                            photo.needs_review = prev.needs_review;
                            if prev.quality.pixel_checked {
                                photo.quality = prev.quality.clone();
                            }
                        } else {
                            changed = true;
                            photo.confirmed = false;
                            photo.needs_review = true;
                        }
                    } else {
                        changed = true;
                    }
                    merged.push(photo);
                }
                existing.source_root = path.display().to_string();
                existing.recursive = opts.recursive;
                existing.scan_complete = !out.cancelled && out.read_errors == 0;
                existing.ignored_count = out.ignored_count;
                existing.ignored_reasons = out.ignored_reasons;
                existing.photos = merged;
                if changed {
                    existing.mappings_valid = false;
                    existing.status = "needs_review".into();
                }
                existing.revision += 1;
                existing.updated_at = now_iso();
                existing
            } else {
                self.new_task(path, opts, out, default_task_name, out_root, strict)
            }
        } else {
            self.new_task(path, opts, out, default_task_name, out_root, strict)
        };
        task.updated_at = now_iso();
        self.task = Some(task);
        self.undo.clear();
        self.redo.clear();
        self.persist_draft()?;
        self.detail()
    }

    fn new_task(
        &self,
        path: &Path,
        opts: ScanOptions,
        out: scan::ScanOutput,
        task_name: String,
        output_root: String,
        strict_hash: bool,
    ) -> Task {
        Task {
            id: new_id("task"),
            source_root: path.display().to_string(),
            recursive: opts.recursive,
            scan_complete: !out.cancelled && out.read_errors == 0,
            task_name,
            output_root,
            dir_layout: "task_and_group".into(),
            dir_pattern: "{task}/{group}".into(),
            naming_rule: NamingRule::default(),
            strict_hash,
            status: "editing".into(),
            mappings_valid: true,
            revision: 1,
            photos: out.photos,
            groups: Vec::new(),
            ignored_count: out.ignored_count,
            ignored_reasons: out.ignored_reasons,
            favorite_tag_ids: Vec::new(),
            created_at: now_iso(),
            updated_at: now_iso(),
        }
    }

    pub fn detail(&self) -> Result<TaskDetail> {
        let task = self.task_ref()?;
        let tags = db::list_tags(&self.db)?;
        let counts = counts_of(task);
        let groups = task.groups.iter().map(|g| group_view(task, g)).collect();
        let mut times: Vec<&str> = task
            .photos
            .iter()
            .filter(|p| !p.excluded)
            .filter_map(|p| p.capture_text.as_deref())
            .collect();
        times.sort();
        let time_span = match (times.first(), times.last()) {
            (Some(a), Some(b)) => Some(((*a).to_string(), (*b).to_string())),
            _ => None,
        };
        Ok(TaskDetail {
            task_id: task.id.clone(),
            source_root: task.source_root.clone(),
            output_root: task.output_root.clone(),
            task_name: task.task_name.clone(),
            dir_layout: task.dir_layout.clone(),
            dir_pattern: task.dir_pattern.clone(),
            naming_rule: task.naming_rule.clone(),
            dir_needs_task: pattern_uses_task(task),
            dir_needs_group: pattern_uses_group(task),
            scan_complete: task.scan_complete,
            recursive: task.recursive,
            strict_hash: task.strict_hash,
            status: task.status.clone(),
            mappings_valid: task.mappings_valid,
            revision: task.revision,
            counts,
            groups,
            ignored_count: task.ignored_count,
            ignored_reasons: task.ignored_reasons.clone(),
            time_span,
            can_undo: !self.undo.is_empty(),
            can_redo: !self.redo.is_empty(),
            last_undo_label: self.undo.last().map(|s| s.label.clone()),
            last_redo_label: self.redo.last().map(|s| s.label.clone()),
            sample_preview_path: sample_preview(task, &tags),
        })
    }

    pub fn photos(&self, group_id: Option<&str>, offset: usize, limit: usize) -> Result<PhotoPage> {
        self.photos_filtered(group_id, None, false, false, false, false, offset, limit)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn photos_filtered(
        &self,
        group_id: Option<&str>,
        query: Option<&str>,
        only_unconfirmed: bool,
        only_quality_issue: bool,
        only_time_anomaly: bool,
        only_excluded: bool,
        offset: usize,
        limit: usize,
    ) -> Result<PhotoPage> {
        let task = self.task_ref()?;
        let tags = db::list_tags(&self.db)?;
        let seq_map = compute_seq_numbers(task);
        let q = query.map(|s| s.trim().to_lowercase()).unwrap_or_default();

        let mut filtered: Vec<PhotoView> = task
            .photos
            .iter()
            .filter(|p| {
                if only_excluded && !p.excluded {
                    return false;
                }
                if let Some(gid) = group_id {
                    if gid == "__ungrouped__" {
                        if p.group_id.is_some() {
                            return false;
                        }
                    } else if p.group_id.as_deref() != Some(gid) {
                        return false;
                    }
                }
                if only_unconfirmed && (p.confirmed && !p.needs_review) {
                    return false;
                }
                if only_quality_issue
                    && !matches!(p.quality.status.as_str(), "warn" | "bad")
                    && !p.corrupt
                {
                    return false;
                }
                if only_time_anomaly
                    && !p.time_uncertain
                    && !p.equal_timestamp
                    && p.time_source == "exif_original"
                {
                    return false;
                }
                true
            })
            .map(|p| {
                let seq = seq_map.get(&p.id).copied().unwrap_or(1);
                photo_view(task, &tags, p, seq)
            })
            .filter(|pv| {
                if q.is_empty() {
                    return true;
                }
                pv.file_name.to_lowercase().contains(&q)
                    || pv.preview_rel_path.to_lowercase().contains(&q)
                    || pv
                        .group_name
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&q)
                    || pv
                        .tag_name
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&q)
            })
            .collect();

        filtered.sort_by_key(|pv| pv.sort_index);
        let total = filtered.len();
        let page_items = if limit == 0 {
            filtered.into_iter().skip(offset).collect()
        } else {
            filtered.into_iter().skip(offset).take(limit).collect()
        };
        Ok(PhotoPage {
            total,
            offset,
            items: page_items,
        })
    }

    pub fn source_file_for_viewing(&self, photo_id: &str) -> Result<String> {
        let task = self.task_ref()?;
        let photo = task
            .photos
            .iter()
            .find(|p| p.id == photo_id)
            .ok_or_else(|| err("", "图片不存在"))?;
        if !util::is_file_any(Path::new(&photo.source_path)) {
            return Err(err("E003", "源图片文件不存在或已移动"));
        }
        Ok(photo.source_path.clone())
    }

    pub fn thumb_input(&self, photo_id: &str) -> Result<(PathBuf, Photo)> {
        let task = self.task_ref()?;
        let photo = task
            .photos
            .iter()
            .find(|p| p.id == photo_id)
            .cloned()
            .ok_or_else(|| err("", "图片不存在"))?;
        Ok((self.data_dir.join("thumbs"), photo))
    }

    pub fn thumbnail(&self, photo_id: &str, max_edge: u32) -> Result<ResultThumb> {
        let (cache_root, photo) = self.thumb_input(photo_id)?;
        match thumbs::render(&cache_root, &photo, max_edge) {
            ThumbResult::Jpeg(bytes) => Ok(ResultThumb::Jpeg(bytes)),
            ThumbResult::Unavailable { reason } => Ok(ResultThumb::Unavailable { reason }),
        }
    }

    pub fn thumb_window(
        &self,
        total: usize,
        scroll_index: usize,
        visible: usize,
        neighbor: usize,
    ) -> ThumbWindow {
        thumbs::retained_payload_window(total, scroll_index, visible, neighbor)
    }

    pub fn quality_snapshot(&self) -> Result<(String, Vec<Photo>)> {
        let task = self.task_ref()?;
        Ok((task.id.clone(), task.photos.clone()))
    }

    pub fn apply_quality_batch(
        &mut self,
        task_id: &str,
        updates: Vec<(String, PhotoQuality)>,
    ) -> Result<()> {
        let task = self.task_mut()?;
        if task.id != task_id {
            return Ok(());
        }
        let map: HashMap<String, PhotoQuality> = updates.into_iter().collect();
        for photo in &mut task.photos {
            if let Some(q) = map.get(&photo.id) {
                photo.quality = q.clone();
            }
        }
        task.updated_at = now_iso();
        self.persist_draft()?;
        Ok(())
    }

    pub fn quality_report(&self, cancelled: bool) -> Result<QualityReport> {
        let task = self.task_ref()?;
        let active: Vec<&Photo> = task.photos.iter().filter(|p| !p.excluded).collect();
        let mut r = QualityReport {
            total: active.len() as u32,
            checked: 0,
            ok_count: 0,
            warn_count: 0,
            bad_count: 0,
            truncated_count: 0,
            low_res_count: 0,
            blur_count: 0,
            underexposed_count: 0,
            overexposed_count: 0,
            low_contrast_count: 0,
            cancelled,
        };
        for p in active {
            if p.quality.pixel_checked || p.corrupt {
                r.checked += 1;
            }
            match p.quality.status.as_str() {
                "ok" => r.ok_count += 1,
                "warn" => r.warn_count += 1,
                "bad" => r.bad_count += 1,
                _ => {}
            }
            for iss in &p.quality.issues {
                match iss.as_str() {
                    "truncated" | "corrupt" => r.truncated_count += 1,
                    "low_res" => r.low_res_count += 1,
                    "blur" => r.blur_count += 1,
                    "underexposed" => r.underexposed_count += 1,
                    "overexposed" => r.overexposed_count += 1,
                    "low_contrast" => r.low_contrast_count += 1,
                    _ => {}
                }
            }
        }
        Ok(r)
    }

    pub fn inspect_quality(&mut self) -> Result<QualityReport> {
        let (task_id, photos) = self.quality_snapshot()?;
        let (updates, cancelled) = run_quality_inspection_batch(&photos, |_| ScanControl::Continue);
        self.apply_quality_batch(&task_id, updates)?;
        self.quality_report(cancelled)
    }

    pub fn set_task_name(&mut self, name: &str) -> Result<TaskDetail> {
        let trimmed = name.trim();
        if !trimmed.is_empty() {
            validate_component(trimmed, "任务名称")?;
        }
        let task = self.task_mut()?;
        task.task_name = trimmed.to_string();
        task.revision += 1;
        task.updated_at = now_iso();
        self.persist_draft()?;
        self.detail()
    }

    pub fn set_dir_layout(&mut self, layout: &str, custom_pattern: &str) -> Result<TaskDetail> {
        let allowed = [
            "task_and_group",
            "group_only",
            "task_only",
            "date_and_group",
            "flat",
            "custom",
        ];
        if !allowed.contains(&layout) {
            return Err(err("E005", "不支持的目录层级模式"));
        }
        if layout == "custom" {
            for seg in custom_pattern.split(['/', '\\']) {
                let s = seg.trim();
                if s.is_empty() {
                    continue;
                }
                let replaced = s
                    .replace("{task}", "T")
                    .replace("{group}", "G")
                    .replace("{date}", "2026-01-01")
                    .replace("{year}", "2026")
                    .replace("{month}", "2026-01")
                    .replace("{day}", "01");
                validate_component(&replaced, "自定义目录段")?;
            }
        }
        self.push_undo_labeled("调整目录层级")?;
        {
            let task = self.task_mut()?;
            task.dir_layout = layout.to_string();
            task.dir_pattern = dir_pattern_for_layout(layout, custom_pattern);
            Self::auto_confirm_ungrouped(task);
            task.revision += 1;
            task.updated_at = now_iso();
        }
        self.persist_draft()?;
        self.detail()
    }

    /// In flat/task-only layouts that do not require grouping, mark healthy scanned photos ready.
    fn auto_confirm_ungrouped(task: &mut Task) {
        if !pattern_uses_group(task) {
            for p in &mut task.photos {
                if !p.excluded && !p.corrupt && !p.needs_review {
                    p.confirmed = true;
                }
            }
        }
    }

    pub fn set_naming_rule(&mut self, mut rule: NamingRule) -> Result<TaskDetail> {
        rule.seq_digits = rule.seq_digits.clamp(1, 6);
        rule.seq_start = rule.seq_start.max(1);
        if !rule.prefix.trim().is_empty() {
            validate_component(rule.prefix.trim(), "前缀")?;
        }
        if !rule.suffix.trim().is_empty() {
            validate_component(rule.suffix.trim(), "后缀")?;
        }
        self.push_undo_labeled("修改命名规则")?;
        {
            let task = self.task_mut()?;
            task.naming_rule = rule;
            task.revision += 1;
            task.updated_at = now_iso();
        }
        self.persist_draft()?;
        self.detail()
    }

    pub fn set_output_root(&mut self, path: &Path) -> Result<TaskDetail> {
        let s = path.display().to_string();
        db::set_setting(&self.db, "output_root", &s)?;
        if let Some(task) = self.task.as_mut() {
            task.output_root = s;
            task.revision += 1;
            task.updated_at = now_iso();
            self.persist_draft()?;
            self.detail()
        } else {
            Err(err("", "尚未导入图片"))
        }
    }

    pub fn set_output_root_remembered(&mut self, path: &Path) -> Result<Option<TaskDetail>> {
        let s = path.display().to_string();
        db::set_setting(&self.db, "output_root", &s)?;
        if let Some(task) = self.task.as_mut() {
            task.output_root = s;
            task.revision += 1;
            task.updated_at = now_iso();
            self.persist_draft()?;
            Ok(Some(self.detail()?))
        } else {
            Ok(None)
        }
    }

    pub fn check_paths(&self, source_root: &str, output_root: &str) -> PathCheckView {
        archive::check_paths(source_root, output_root)
    }

    pub fn set_strict(&mut self, strict: bool) -> Result<TaskDetail> {
        db::set_setting(&self.db, "strict_hash", if strict { "1" } else { "0" })?;
        let task = self.task_mut()?;
        task.strict_hash = strict;
        task.revision += 1;
        task.updated_at = now_iso();
        self.persist_draft()?;
        self.detail()
    }

    pub fn set_groups_text(&mut self, text: &str) -> Result<TaskDetail> {
        let names = parse_group_batch(text)?;
        self.push_undo_labeled("更新分类清单")?;
        {
            let task = self.task_mut()?;
            let existing_by_key: HashMap<String, Group> = task
                .groups
                .iter()
                .map(|g| (g.name.to_lowercase(), g.clone()))
                .collect();
            let mut new_groups = Vec::with_capacity(names.len());
            for (idx, name) in names.into_iter().enumerate() {
                let key = name.to_lowercase();
                if let Some(mut old) = existing_by_key.get(&key).cloned() {
                    old.name = name;
                    old.sort_order = idx as u32;
                    new_groups.push(old);
                } else {
                    new_groups.push(Group {
                        id: new_id("grp"),
                        name,
                        sort_order: idx as u32,
                        preset_id: None,
                        preset_name: String::new(),
                        suggested_tags: Vec::new(),
                    });
                }
            }
            let live_ids: HashSet<String> = new_groups.iter().map(|g| g.id.clone()).collect();
            for p in &mut task.photos {
                if let Some(gid) = &p.group_id {
                    if !live_ids.contains(gid) {
                        p.group_id = None;
                    }
                }
            }
            task.groups = new_groups;
            task.revision += 1;
            task.updated_at = now_iso();
        }
        self.persist_draft()?;
        self.detail()
    }

    pub fn add_group(&mut self, name: &str) -> Result<TaskDetail> {
        let name = name.trim();
        validate_component(name, "分类名称")?;
        {
            let task = self.task_ref()?;
            if task.groups.iter().any(|g| g.name.eq_ignore_ascii_case(name)) {
                return Err(err("E001", format!("分类「{name}」已存在")));
            }
        }
        self.push_undo_labeled(&format!("新建分类 {name}"))?;
        {
            let task = self.task_mut()?;
            let order = task.groups.len() as u32;
            task.groups.push(Group {
                id: new_id("grp"),
                name: name.to_string(),
                sort_order: order,
                preset_id: None,
                preset_name: String::new(),
                suggested_tags: Vec::new(),
            });
            task.revision += 1;
            task.updated_at = now_iso();
        }
        self.persist_draft()?;
        self.detail()
    }

    pub fn rename_group(&mut self, group_id: &str, new_name: &str) -> Result<TaskDetail> {
        let new_name = new_name.trim();
        validate_component(new_name, "分类名称")?;
        {
            let task = self.task_ref()?;
            if !task.groups.iter().any(|g| g.id == group_id) {
                return Err(err("", "分类不存在"));
            }
            if task
                .groups
                .iter()
                .any(|g| g.id != group_id && g.name.eq_ignore_ascii_case(new_name))
            {
                return Err(err("E001", format!("已有同名分类「{new_name}」")));
            }
        }
        self.push_undo_labeled("重命名分类")?;
        {
            let task = self.task_mut()?;
            if let Some(g) = task.groups.iter_mut().find(|g| g.id == group_id) {
                g.name = new_name.to_string();
            }
            task.revision += 1;
            task.updated_at = now_iso();
        }
        self.persist_draft()?;
        self.detail()
    }

    pub fn delete_group(&mut self, group_id: &str) -> Result<TaskDetail> {
        self.push_undo_labeled("删除分类")?;
        {
            let task = self.task_mut()?;
            task.groups.retain(|g| g.id != group_id);
            for (i, g) in task.groups.iter_mut().enumerate() {
                g.sort_order = i as u32;
            }
            for p in &mut task.photos {
                if p.group_id.as_deref() == Some(group_id) {
                    p.group_id = None;
                }
            }
            task.revision += 1;
            task.updated_at = now_iso();
        }
        self.persist_draft()?;
        self.detail()
    }

    pub fn reorder_group_to(&mut self, from_index: usize, to_index: usize) -> Result<TaskDetail> {
        let len = self.task_ref()?.groups.len();
        if from_index >= len || to_index >= len || from_index == to_index {
            return self.detail();
        }
        self.push_undo_labeled("调整分类顺序")?;
        {
            let task = self.task_mut()?;
            let g = task.groups.remove(from_index);
            task.groups.insert(to_index, g);
            for (i, gr) in task.groups.iter_mut().enumerate() {
                gr.sort_order = i as u32;
            }
            task.revision += 1;
            task.updated_at = now_iso();
        }
        self.persist_draft()?;
        self.detail()
    }

    pub fn sort_groups_by_name(&mut self) -> Result<TaskDetail> {
        self.push_undo_labeled("按名称排序分类")?;
        {
            let task = self.task_mut()?;
            task.groups.sort_by(|a, b| natural_cmp(&a.name, &b.name));
            for (i, g) in task.groups.iter_mut().enumerate() {
                g.sort_order = i as u32;
            }
            task.revision += 1;
            task.updated_at = now_iso();
        }
        self.persist_draft()?;
        self.detail()
    }

    pub fn group_by_time(&mut self, granularity: &str) -> Result<TaskDetail> {
        self.push_undo_labeled("按时间自动分组")?;
        {
            let task = self.task_mut()?;
            grouping::group_by_time_bucket(task, granularity)?;
            task.revision += 1;
            task.updated_at = now_iso();
        }
        self.persist_draft()?;
        self.detail()
    }

    pub fn group_by_time_gap(&mut self, gap_minutes: u32, prefix: &str) -> Result<TaskDetail> {
        self.push_undo_labeled("按拍摄间隔分组")?;
        {
            let task = self.task_mut()?;
            grouping::group_by_time_gap(task, gap_minutes, prefix)?;
            task.revision += 1;
            task.updated_at = now_iso();
        }
        self.persist_draft()?;
        self.detail()
    }

    pub fn group_by_fixed_size(&mut self, per_group: u32, prefix: &str) -> Result<TaskDetail> {
        self.push_undo_labeled("按固定数量分组")?;
        {
            let task = self.task_mut()?;
            grouping::group_by_fixed_size(task, per_group, prefix)?;
            task.revision += 1;
            task.updated_at = now_iso();
        }
        self.persist_draft()?;
        self.detail()
    }

    pub fn group_evenly(&mut self) -> Result<TaskDetail> {
        self.push_undo_labeled("平均分配到各分类")?;
        {
            let task = self.task_mut()?;
            grouping::group_evenly_across_existing(task)?;
            task.revision += 1;
            task.updated_at = now_iso();
        }
        self.persist_draft()?;
        self.detail()
    }

    pub fn set_quantities(&mut self, counts: &[u32]) -> Result<TaskDetail> {
        self.push_undo_labeled("按数量分配各分类")?;
        {
            let task = self.task_mut()?;
            grouping::group_by_counts_across_existing(task, counts)?;
            task.revision += 1;
            task.updated_at = now_iso();
        }
        self.persist_draft()?;
        self.detail()
    }

    pub fn apply_preset(
        &mut self,
        preset_id: &str,
        distribute_evenly: bool,
        apply_naming: bool,
    ) -> Result<TaskDetail> {
        let preset = db::load_preset(&self.db, preset_id)?;
        self.push_undo_labeled(&format!("应用方案 {}", preset.name))?;
        {
            let task = self.task_mut()?;
            grouping::apply_preset_groups(task, &preset, distribute_evenly, apply_naming)?;
            task.revision += 1;
            task.updated_at = now_iso();
        }
        self.persist_draft()?;
        self.detail()
    }

    pub fn assign_photos_to_group(
        &mut self,
        photo_ids: &[String],
        group_id: &str,
    ) -> Result<TaskDetail> {
        self.push_undo_labeled("批量归类图片")?;
        {
            let task = self.task_mut()?;
            grouping::assign_photos_to_group(task, photo_ids, group_id)?;
            task.revision += 1;
            task.updated_at = now_iso();
        }
        self.persist_draft()?;
        self.detail()
    }

    pub fn create_group_for_photos(
        &mut self,
        group_name: &str,
        photo_ids: &[String],
    ) -> Result<TaskDetail> {
        let name = group_name.trim();
        validate_component(name, "分类名称")?;
        self.push_undo_labeled(&format!("归入分类 {name}"))?;
        {
            let task = self.task_mut()?;
            let gid = if let Some(existing) = task
                .groups
                .iter()
                .find(|g| g.name.eq_ignore_ascii_case(name))
            {
                existing.id.clone()
            } else {
                let id = new_id("grp");
                let order = task.groups.len() as u32;
                task.groups.push(Group {
                    id: id.clone(),
                    name: name.to_string(),
                    sort_order: order,
                    preset_id: None,
                    preset_name: String::new(),
                    suggested_tags: Vec::new(),
                });
                id
            };
            grouping::assign_photos_to_group(task, photo_ids, &gid)?;
            task.revision += 1;
            task.updated_at = now_iso();
        }
        self.persist_draft()?;
        self.detail()
    }

    pub fn move_photo(&mut self, photo_id: &str, delta: i32) -> Result<TaskDetail> {
        if delta == 0 {
            return self.detail();
        }
        let task = self.task_ref()?;
        let Some(pos) = task.photos.iter().position(|p| p.id == photo_id) else {
            return Err(err("", "图片不存在"));
        };
        let target = (pos as isize + delta as isize).clamp(0, task.photos.len() as isize - 1) as usize;
        if target == pos {
            return self.detail();
        }
        self.push_undo_labeled("调整图片顺序")?;
        {
            let task = self.task_mut()?;
            let photo = task.photos.remove(pos);
            task.photos.insert(target, photo);
            reindex(task);
            task.revision += 1;
            task.updated_at = now_iso();
        }
        self.persist_draft()?;
        self.detail()
    }

    pub fn resort_by_mtime(&mut self) -> Result<TaskDetail> {
        self.push_undo_labeled("按拍摄时间重排")?;
        {
            let task = self.task_mut()?;
            scan::sort_photos(&mut task.photos);
            task.revision += 1;
            task.updated_at = now_iso();
        }
        self.persist_draft()?;
        self.detail()
    }

    pub fn resort_by_filename(&mut self) -> Result<TaskDetail> {
        self.push_undo_labeled("按文件名重排")?;
        {
            let task = self.task_mut()?;
            task.photos
                .sort_by(|a, b| natural_cmp(&a.file_name, &b.file_name));
            reindex(task);
            task.revision += 1;
            task.updated_at = now_iso();
        }
        self.persist_draft()?;
        self.detail()
    }

    pub fn assign_tag(&mut self, photo_id: &str, tag_id: Option<&str>) -> Result<TaskDetail> {
        if let Some(tid) = tag_id {
            let _ = db::tag_by_id(&self.db, tid)?;
        }
        self.push_undo_labeled("设置图片标签")?;
        {
            let task = self.task_mut()?;
            let p = task
                .photos
                .iter_mut()
                .find(|p| p.id == photo_id)
                .ok_or_else(|| err("", "图片不存在"))?;
            p.tag_id = tag_id.map(str::to_string);
            if !p.corrupt && !p.needs_review {
                p.confirmed = true;
            }
            task.revision += 1;
            task.updated_at = now_iso();
        }
        self.persist_draft()?;
        self.detail()
    }

    pub fn batch_assign_tag(
        &mut self,
        photo_ids: &[String],
        tag_id: Option<&str>,
    ) -> Result<TaskDetail> {
        if let Some(tid) = tag_id {
            let _ = db::tag_by_id(&self.db, tid)?;
        }
        self.push_undo_labeled("批量设置标签")?;
        {
            let task = self.task_mut()?;
            let set: HashSet<&str> = photo_ids.iter().map(String::as_str).collect();
            for p in &mut task.photos {
                if set.contains(p.id.as_str()) {
                    p.tag_id = tag_id.map(str::to_string);
                    if !p.corrupt && !p.needs_review {
                        p.confirmed = true;
                    }
                }
            }
            task.revision += 1;
            task.updated_at = now_iso();
        }
        self.persist_draft()?;
        self.detail()
    }

    pub fn set_custom_name(
        &mut self,
        photo_id: &str,
        custom_name: Option<&str>,
    ) -> Result<TaskDetail> {
        let cleaned = custom_name.map(str::trim).filter(|s| !s.is_empty());
        if let Some(name) = cleaned {
            validate_component(name, "自定义文件名")?;
        }
        self.push_undo_labeled("自定义图片名称")?;
        {
            let task = self.task_mut()?;
            let p = task
                .photos
                .iter_mut()
                .find(|p| p.id == photo_id)
                .ok_or_else(|| err("", "图片不存在"))?;
            p.custom_name = cleaned.map(str::to_string);
            if !p.corrupt && !p.needs_review {
                p.confirmed = true;
            }
            task.revision += 1;
            task.updated_at = now_iso();
        }
        self.persist_draft()?;
        self.detail()
    }

    pub fn confirm_photo(&mut self, photo_id: &str) -> Result<TaskDetail> {
        self.push_undo_labeled("确认图片")?;
        {
            let task = self.task_mut()?;
            let p = task
                .photos
                .iter_mut()
                .find(|p| p.id == photo_id)
                .ok_or_else(|| err("", "图片不存在"))?;
            p.confirmed = true;
            p.needs_review = false;
            task.revision += 1;
            task.updated_at = now_iso();
        }
        self.persist_draft()?;
        self.detail()
    }

    pub fn batch_confirm_photos(&mut self, photo_ids: &[String]) -> Result<TaskDetail> {
        self.push_undo_labeled("批量确认图片")?;
        {
            let task = self.task_mut()?;
            let set: HashSet<&str> = photo_ids.iter().map(String::as_str).collect();
            for p in &mut task.photos {
                if set.contains(p.id.as_str()) && !p.excluded && !p.corrupt {
                    p.confirmed = true;
                    p.needs_review = false;
                }
            }
            task.revision += 1;
            task.updated_at = now_iso();
        }
        self.persist_draft()?;
        self.detail()
    }

    pub fn confirm_all_ready(&mut self) -> Result<TaskDetail> {
        self.push_undo_labeled("全部确认就绪")?;
        {
            let task = self.task_mut()?;
            let needs_group = pattern_uses_group(task);
            for p in &mut task.photos {
                if !p.excluded && !p.corrupt && (!needs_group || p.group_id.is_some()) {
                    p.confirmed = true;
                    p.needs_review = false;
                }
            }
            task.mappings_valid = true;
            task.status = "editing".into();
            task.revision += 1;
            task.updated_at = now_iso();
        }
        self.persist_draft()?;
        self.detail()
    }

    pub fn exclude_photo(&mut self, photo_id: &str, excluded: bool) -> Result<TaskDetail> {
        self.push_undo_labeled(if excluded { "排除图片" } else { "恢复图片" })?;
        {
            let task = self.task_mut()?;
            let p = task
                .photos
                .iter_mut()
                .find(|p| p.id == photo_id)
                .ok_or_else(|| err("", "图片不存在"))?;
            p.excluded = excluded;
            task.revision += 1;
            task.updated_at = now_iso();
        }
        self.persist_draft()?;
        self.detail()
    }

    pub fn batch_exclude_photos(
        &mut self,
        photo_ids: &[String],
        excluded: bool,
    ) -> Result<TaskDetail> {
        self.push_undo_labeled(if excluded { "批量排除图片" } else { "批量恢复图片" })?;
        {
            let task = self.task_mut()?;
            let set: HashSet<&str> = photo_ids.iter().map(String::as_str).collect();
            for p in &mut task.photos {
                if set.contains(p.id.as_str()) {
                    p.excluded = excluded;
                }
            }
            task.revision += 1;
            task.updated_at = now_iso();
        }
        self.persist_draft()?;
        self.detail()
    }

    pub fn acknowledge_review(&mut self) -> Result<TaskDetail> {
        self.push_undo_labeled("确认待复核项")?;
        {
            let task = self.task_mut()?;
            for p in &mut task.photos {
                if !p.excluded && !p.corrupt {
                    p.needs_review = false;
                    p.confirmed = true;
                }
            }
            task.mappings_valid = true;
            task.status = "editing".into();
            task.revision += 1;
            task.updated_at = now_iso();
        }
        self.persist_draft()?;
        self.detail()
    }

    pub fn undo_edit(&mut self) -> Result<TaskDetail> {
        let snap = self.undo.pop().ok_or_else(|| err("", "没有可撤销的编辑"))?;
        let current = snapshot_labeled(self.task_ref()?, &snap.label);
        self.redo.push(current);
        self.restore_snap(snap)?;
        self.persist_draft()?;
        self.detail()
    }

    pub fn redo_edit(&mut self) -> Result<TaskDetail> {
        let snap = self.redo.pop().ok_or_else(|| err("", "没有可重做的编辑"))?;
        let current = snapshot_labeled(self.task_ref()?, &snap.label);
        self.undo.push(current);
        self.restore_snap(snap)?;
        self.persist_draft()?;
        self.detail()
    }

    fn restore_snap(&mut self, snap: EditSnap) -> Result<()> {
        let task = self.task_mut()?;
        task.groups = snap.groups;
        task.dir_layout = snap.dir_layout;
        task.dir_pattern = snap.dir_pattern;
        task.naming_rule = snap.naming_rule;
        let by_id: HashMap<String, Photo> = task
            .photos
            .drain(..)
            .map(|p| (p.id.clone(), p))
            .collect();
        let mut rebuilt = Vec::with_capacity(snap.photos.len());
        for ps in snap.photos {
            if let Some(mut p) = by_id.get(&ps.id).cloned() {
                p.sort_index = ps.sort_index;
                p.group_id = ps.group_id;
                p.tag_id = ps.tag_id;
                p.custom_name = ps.custom_name;
                p.confirmed = ps.confirmed;
                p.needs_review = ps.needs_review;
                p.excluded = ps.excluded;
                rebuilt.push(p);
            }
        }
        rebuilt.sort_by_key(|p| p.sort_index);
        task.photos = rebuilt;
        task.revision += 1;
        task.updated_at = now_iso();
        Ok(())
    }

    fn push_undo_labeled(&mut self, label: &str) -> Result<()> {
        let snap = snapshot_labeled(self.task_ref()?, label);
        self.undo.push(snap);
        if self.undo.len() > 60 {
            self.undo.remove(0);
        }
        self.redo.clear();
        Ok(())
    }

    pub fn set_space_probe(&mut self, bytes: Option<u64>) {
        self.free_override = bytes;
    }

    pub fn build_plan(&mut self) -> Result<ExecutionPlan> {
        let task = self.task_ref()?.clone();
        let tags = db::list_tags(&self.db)?;
        let plan = archive::build_plan_for(&task, &tags, self.free_override)?;
        self.plans.insert(plan.id.clone(), plan.clone());
        Ok(plan)
    }

    pub fn prepare_execute(&self, plan_id: &str) -> Result<PreparedExecution> {
        let plan = self
            .plans
            .get(plan_id)
            .cloned()
            .ok_or_else(|| err("", "计划不存在，请重新检查"))?;
        let task = self.task_ref()?.clone();
        let tags = db::list_tags(&self.db)?;
        Ok(PreparedExecution {
            db_path: self.db_path.clone(),
            task,
            plan,
            tags,
            free_override: self.free_override,
        })
    }

    pub fn execute_prepared(
        prepared: PreparedExecution,
        opts: ExecuteOptions,
        mut progress: impl FnMut(&CopyTick),
    ) -> Result<ExecuteReport> {
        let conn = db::open_worker_db(&prepared.db_path)?;
        archive::execute(
            &conn,
            &prepared.task,
            &prepared.plan,
            &opts,
            &prepared.tags,
            prepared.free_override,
            &mut progress,
        )
    }

    pub fn execute_plan(&mut self, plan_id: &str, opts: ExecuteOptions) -> Result<ExecuteReport> {
        let prepared = self.prepare_execute(plan_id)?;
        let report = Self::execute_prepared(prepared, opts, |_| {})?;
        if report.status == "success" {
            if let Some(task) = self.task.as_mut() {
                task.status = "archived".into();
                task.updated_at = now_iso();
            }
            let _ = self.persist_draft();
        }
        Ok(report)
    }

    pub fn recovery(&self) -> Result<Vec<RecoveryView>> {
        let jobs = db::list_jobs(&self.db)?;
        Ok(jobs
            .into_iter()
            .filter(|j| matches!(j.status.as_str(), "running" | "interrupted" | "partial"))
            .map(|j| archive::recovery_view(&j))
            .collect())
    }

    pub fn retry_job(&self, job_id: &str) -> Result<ExecuteReport> {
        let mut job = db::load_job(&self.db, job_id)?;
        archive::retry_with(&self.db, &mut job, None, &mut |_| {})
    }

    pub fn retry_job_with(
        db_path: &Path,
        job_id: &str,
        cancel: Option<&std::sync::atomic::AtomicBool>,
        mut progress: impl FnMut(&CopyTick),
    ) -> Result<ExecuteReport> {
        let conn = db::open_worker_db(db_path)?;
        let mut job = db::load_job(&conn, job_id)?;
        archive::retry_with(&conn, &mut job, cancel, &mut progress)
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn undo_preview(&self, job_id: &str) -> Result<UndoPreview> {
        let job = db::load_job(&self.db, job_id)?;
        Ok(archive::undo_preview(&job))
    }

    pub fn undo_job(&self, job_id: &str) -> Result<UndoReport> {
        let mut job = db::load_job(&self.db, job_id)?;
        archive::undo_job(&self.db, &mut job)
    }

    pub fn list_drafts(&self) -> Result<Vec<DraftSummary>> {
        db::list_drafts(&self.db)
    }

    pub fn open_draft(&mut self, id: &str) -> Result<TaskDetail> {
        let payload = db::load_draft(&self.db, id)?;
        let mut task: Task = serde_json::from_str(&payload)
            .map_err(|e| err("", format!("草稿解析失败：{e}")))?;
        if recheck_sources(&mut task) {
            task.mappings_valid = false;
            task.status = "needs_review".into();
        }
        self.task = Some(task);
        self.undo.clear();
        self.redo.clear();
        self.detail()
    }

    pub fn tags(&self) -> Result<Vec<TagItem>> {
        db::list_tags(&self.db)
    }

    pub fn tag_create(
        &mut self,
        name: String,
        short_label: String,
        aliases: Vec<String>,
    ) -> Result<TagItem> {
        let existing = db::list_tags(&self.db)?;
        let next_order = existing.iter().map(|t| t.sort_order).max().unwrap_or(0) + 10;
        let tag = TagItem {
            id: String::new(),
            name: name.trim().to_string(),
            short_label: short_label.trim().to_string(),
            aliases,
            sort_order: next_order,
            enabled: true,
            source: "user".into(),
            category: "自定义标签".into(),
            updated_at: now_iso(),
        };
        db::insert_tag(&self.db, tag)
    }

    pub fn tag_update(
        &mut self,
        id: &str,
        name: String,
        short_label: String,
        aliases: Vec<String>,
    ) -> Result<()> {
        let mut tag = db::tag_by_id(&self.db, id)?;
        tag.name = name.trim().to_string();
        tag.short_label = if short_label.trim().is_empty() {
            tag.name.clone()
        } else {
            short_label.trim().to_string()
        };
        tag.aliases = aliases;
        db::update_tag(&self.db, &tag)
    }

    pub fn tag_set_enabled(&mut self, id: &str, enabled: bool) -> Result<()> {
        let mut tag = db::tag_by_id(&self.db, id)?;
        tag.enabled = enabled;
        db::update_tag(&self.db, &tag)
    }

    pub fn tag_delete(&mut self, id: &str) -> Result<()> {
        if let Some(task) = self.task.as_mut() {
            for p in &mut task.photos {
                if p.tag_id.as_deref() == Some(id) {
                    p.tag_id = None;
                }
            }
        }
        db::delete_tag(&self.db, id)
    }

    pub fn tag_reorder(&mut self, ids: &[String]) -> Result<()> {
        db::reorder_tags(&self.db, ids)
    }

    pub fn import_tags_text(&mut self, text: &str) -> Result<ImportReport> {
        let names: Vec<String> = text
            .lines()
            .map(str::trim)
            .filter(|s| !s.is_empty() && !s.starts_with('#'))
            .map(str::to_string)
            .collect();
        db::import_tags(&self.db, &names)
    }

    pub fn export_tags_json(&self) -> Result<String> {
        let tags = db::list_tags(&self.db)?;
        serde_json::to_string_pretty(&tags).map_err(|e| err("", e.to_string()))
    }

    pub fn import_tags_json(&mut self, json: &str) -> Result<ImportReport> {
        let parsed: Vec<TagItem> =
            serde_json::from_str(json).map_err(|e| err("E008", format!("标签文件无效：{e}")))?;
        let existing = db::list_tags(&self.db)?;
        let mut added = 0u32;
        let mut updated = 0u32;
        for mut item in parsed {
            if let Some(found) = existing
                .iter()
                .find(|t| t.name.eq_ignore_ascii_case(&item.name))
            {
                let mut upd = found.clone();
                upd.short_label = item.short_label;
                upd.aliases = item.aliases;
                db::update_tag(&self.db, &upd)?;
                updated += 1;
            } else {
                item.id = String::new();
                item.source = "user".into();
                db::insert_tag(&self.db, item)?;
                added += 1;
            }
        }
        Ok(ImportReport { added, updated })
    }

    pub fn list_presets(&self) -> Result<Vec<GroupPreset>> {
        db::list_presets(&self.db)
    }

    pub fn save_preset(
        &mut self,
        id: Option<String>,
        name: String,
        description: String,
        scene_type: String,
        group_names: Vec<String>,
        default_naming: Option<NamingRule>,
    ) -> Result<GroupPreset> {
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err(err("", "方案名称不能为空"));
        }
        for gn in &group_names {
            validate_component(gn, "分类名称")?;
        }
        let now = now_iso();
        let existing = id.as_deref().and_then(|pid| db::load_preset(&self.db, pid).ok());
        let preset = GroupPreset {
            id: existing
                .as_ref()
                .map(|p| p.id.clone())
                .unwrap_or_else(|| new_id("preset")),
            name,
            description: description.trim().to_string(),
            scene_type: if scene_type.trim().is_empty() {
                "自定义场景".into()
            } else {
                scene_type.trim().to_string()
            },
            group_names,
            items: existing.as_ref().map(|p| p.items.clone()).unwrap_or_default(),
            default_naming,
            version: existing.as_ref().map(|p| p.version + 1).unwrap_or(1),
            source: "custom".into(),
            created_at: existing
                .as_ref()
                .map(|p| p.created_at.clone())
                .unwrap_or_else(|| now.clone()),
            updated_at: now,
        };
        db::upsert_preset(&self.db, &preset)
    }

    pub fn duplicate_preset(&mut self, id: &str, new_name: Option<&str>) -> Result<GroupPreset> {
        let mut preset = db::load_preset(&self.db, id)?;
        preset.id = new_id("preset");
        preset.name = new_name
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("{} (副本)", preset.name));
        preset.source = "custom".into();
        preset.version = 1;
        let now = now_iso();
        preset.created_at = now.clone();
        preset.updated_at = now;
        db::upsert_preset(&self.db, &preset)
    }

    pub fn delete_preset(&mut self, id: &str) -> Result<()> {
        db::delete_preset(&self.db, id)
    }

    pub fn export_presets_json(&self) -> Result<String> {
        let presets = db::list_presets(&self.db)?;
        serde_json::to_string_pretty(&presets).map_err(|e| err("", e.to_string()))
    }

    pub fn import_presets_json(&mut self, json: &str) -> Result<ImportReport> {
        let parsed: Vec<GroupPreset> = serde_json::from_str(json)
            .map_err(|e| err("E008", format!("方案文件格式无效：{e}")))?;
        let existing = db::list_presets(&self.db)?;
        let mut added = 0u32;
        let mut updated = 0u32;
        for mut item in parsed {
            for gn in &item.group_names {
                validate_component(gn, "分类名称")?;
            }
            if let Some(found) = existing
                .iter()
                .find(|p| p.name.eq_ignore_ascii_case(&item.name))
            {
                item.id = found.id.clone();
                item.version = found.version + 1;
                updated += 1;
            } else {
                item.id = new_id("preset");
                added += 1;
            }
            item.source = "custom".into();
            item.updated_at = now_iso();
            if item.created_at.is_empty() {
                item.created_at = item.updated_at.clone();
            }
            db::upsert_preset(&self.db, &item)?;
        }
        Ok(ImportReport { added, updated })
    }

    pub fn save_template(&mut self, name: &str) -> Result<TaskTemplate> {
        let name = name.trim();
        if name.is_empty() {
            return Err(err("", "模板名称不能为空"));
        }
        let task = self.task_ref()?;
        let template = TaskTemplate {
            id: new_id("tpl"),
            name: name.to_string(),
            task_name: task.task_name.clone(),
            dir_layout: task.dir_layout.clone(),
            dir_pattern: task.dir_pattern.clone(),
            naming_rule: task.naming_rule.clone(),
            group_names: task.groups.iter().map(|g| g.name.clone()).collect(),
            tag_ids: task
                .photos
                .iter()
                .filter_map(|p| p.tag_id.clone())
                .collect(),
            created_at: now_iso(),
        };
        db::save_template(&self.db, &template)?;
        Ok(template)
    }

    pub fn list_templates(&self) -> Result<Vec<TaskTemplate>> {
        db::list_templates(&self.db)
    }

    pub fn delete_template(&mut self, id: &str) -> Result<()> {
        db::delete_template(&self.db, id)
    }

    pub fn apply_template(&mut self, id: &str) -> Result<TaskDetail> {
        let tpl = db::load_template(&self.db, id)?;
        self.push_undo_labeled(&format!("套用模板 {}", tpl.name))?;
        {
            let task = self.task_mut()?;
            if !tpl.task_name.is_empty() {
                task.task_name = tpl.task_name.clone();
            }
            task.dir_layout = tpl.dir_layout.clone();
            task.dir_pattern = tpl.dir_pattern.clone();
            task.naming_rule = tpl.naming_rule.clone();
            if !tpl.group_names.is_empty() {
                task.groups = tpl
                    .group_names
                    .iter()
                    .enumerate()
                    .map(|(i, gn)| Group {
                        id: new_id("grp"),
                        name: gn.clone(),
                        sort_order: i as u32,
                        preset_id: None,
                        preset_name: tpl.name.clone(),
                        suggested_tags: Vec::new(),
                    })
                    .collect();
                for p in &mut task.photos {
                    p.group_id = None;
                }
            }
            Self::auto_confirm_ungrouped(task);
            task.revision += 1;
            task.updated_at = now_iso();
        }
        self.persist_draft()?;
        self.detail()
    }

    pub fn history(&self) -> Result<Vec<HistoryRow>> {
        let jobs = db::list_jobs(&self.db)?;
        Ok(jobs
            .into_iter()
            .map(|j| {
                let mut group_names = HashSet::new();
                let mut failed = 0u32;
                for item in &j.items {
                    group_names.insert(item.group_name.clone());
                    if item.status == "failed" {
                        failed += 1;
                    }
                }
                HistoryRow {
                    job_id: j.job_id,
                    task_name: j.task_name,
                    status: j.status,
                    output_root: j.output_root,
                    created_at: j.created_at,
                    photo_count: j.items.len() as u32,
                    group_count: group_names.len() as u32,
                    failed,
                }
            })
            .collect())
    }

    pub fn job_manifest(&self, job_id: &str) -> Result<JobManifest> {
        db::load_job(&self.db, job_id)
    }

    pub fn export_job_csv(&self, job_id: &str) -> Result<String> {
        let job = db::load_job(&self.db, job_id)?;
        Ok(archive::job_csv(&job))
    }

    pub fn export_job_json(&self, job_id: &str) -> Result<String> {
        let job = db::load_job(&self.db, job_id)?;
        archive::job_json(&job)
    }

    pub fn export_plan_csv(&mut self, confirmed_skips: &[String]) -> Result<String> {
        let plan = self.build_plan()?;
        Ok(archive::plan_csv(&plan, confirmed_skips))
    }

    pub fn task_output_dir(&self, job_id: &str) -> Result<String> {
        let job = db::load_job(&self.db, job_id)?;
        let root = PathBuf::from(&job.output_root);
        let task_dir = if !job.task_name.trim().is_empty() {
            let candidate = root.join(&job.task_name);
            if util::is_dir_any(&candidate) {
                candidate
            } else {
                root
            }
        } else {
            root
        };
        Ok(task_dir.display().to_string())
    }

    pub fn export_dir(&self) -> PathBuf {
        self.data_dir.join("exports")
    }

    pub fn settings(&self) -> Result<SettingsView> {
        let output_root = db::get_setting(&self.db, "output_root")?.unwrap_or_default();
        let open_folder_after =
            db::get_setting(&self.db, "open_folder_after")?.as_deref() == Some("1");
        let retention_days = db::get_setting(&self.db, "retention_days")?
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(90);
        let strict_hash = db::get_setting(&self.db, "strict_hash")?.as_deref() == Some("1");
        let theme = db::get_setting(&self.db, "theme")?.unwrap_or_else(|| "light".into());
        Ok(SettingsView {
            output_root,
            open_folder_after,
            retention_days,
            strict_hash,
            theme,
        })
    }

    pub fn set_open_folder_after(&mut self, enabled: bool) -> Result<SettingsView> {
        db::set_setting(
            &self.db,
            "open_folder_after",
            if enabled { "1" } else { "0" },
        )?;
        self.settings()
    }

    pub fn set_retention_days(&mut self, days: u32) -> Result<SettingsView> {
        let clamped = days.clamp(1, 3650);
        db::set_setting(&self.db, "retention_days", &clamped.to_string())?;
        self.settings()
    }

    pub fn set_theme(&mut self, theme: &str) -> Result<SettingsView> {
        db::set_setting(&self.db, "theme", theme)?;
        self.settings()
    }

    pub fn clear_remembered_output(&mut self) -> Result<SettingsView> {
        db::set_setting(&self.db, "output_root", "")?;
        self.settings()
    }

    pub fn cleanup_retention(&mut self, confirmed: bool) -> Result<ClearReport> {
        if !confirmed {
            return Err(err("", "请先确认清理过期草稿与历史记录"));
        }
        let days = self.settings()?.retention_days;
        let removed = db::cleanup_records(&self.db, days)?;
        Ok(ClearReport {
            removed_files: removed,
            note: format!("已清理 {days} 天前的 {removed} 条记录；原图与已归档文件不受影响。"),
        })
    }

    pub fn clear_thumbnail_cache(&self) -> Result<ClearReport> {
        let dir = self.data_dir.join("thumbs");
        let mut count = 0u32;
        if let Ok(rd) = fs::read_dir(&dir) {
            for entry in rd.flatten() {
                if entry.path().is_file() && fs::remove_file(entry.path()).is_ok() {
                    count += 1;
                }
            }
        }
        Ok(ClearReport {
            removed_files: count,
            note: "已清理缩略图缓存；原图与已归档文件不受影响。".into(),
        })
    }

    pub fn clear_history(&mut self) -> Result<ClearReport> {
        db::clear_jobs_and_drafts(&self.db)?;
        Ok(ClearReport {
            removed_files: 0,
            note: "已清空任务草稿与归档历史记录；原图与已归档文件不受影响。".into(),
        })
    }

    pub fn diagnostics(&self) -> Diagnostics {
        crate::selfcheck::diagnostics(&self.data_dir, self.database_open())
    }

    pub fn review_risk(&self) -> Result<ReviewRisk> {
        let task = self.task_ref()?;
        let affected_files: Vec<String> = task
            .photos
            .iter()
            .filter(|p| !p.excluded && (p.needs_review || p.time_uncertain || p.equal_timestamp))
            .map(|p| p.file_name.clone())
            .collect();
        let affected = affected_files.len() as u32;
        Ok(ReviewRisk {
            affected,
            file_names: affected_files.into_iter().take(20).collect(),
            message: if affected == 0 {
                "当前无待复核图片".into()
            } else {
                format!("共有 {affected} 张图片存在时间不确定/同秒或待复核标记，建议检查排序")
            },
        })
    }

    fn remembered_output_root(&self) -> Result<String> {
        Ok(db::get_setting(&self.db, "output_root")?.unwrap_or_default())
    }

    fn persist_draft(&self) -> Result<()> {
        let task = self.task_ref()?;
        let payload = serde_json::to_string(task).map_err(|e| err("", e.to_string()))?;
        db::save_draft(
            &self.db,
            &task.id,
            &payload,
            &task.source_root,
            &task.task_name,
            &task.status,
        )
    }

    fn task_ref(&self) -> Result<&Task> {
        self.task.as_ref().ok_or_else(|| err("", "尚未打开任务"))
    }

    fn task_mut(&mut self) -> Result<&mut Task> {
        self.task.as_mut().ok_or_else(|| err("", "尚未打开任务"))
    }
}

pub fn execute_archived_plan(
    org: &mut Organizer,
    plan_id: &str,
    opts: ExecuteOptions,
) -> Result<ExecuteReport> {
    org.execute_plan(plan_id, opts)
}

fn group_view(task: &Task, group: &Group) -> GroupView {
    let photos: Vec<&Photo> = task
        .photos
        .iter()
        .filter(|p| p.group_id.as_deref() == Some(group.id.as_str()) && !p.excluded)
        .collect();
    let count = photos.len() as u32;
    let ready_count = photos
        .iter()
        .filter(|p| p.confirmed && !p.needs_review && !p.corrupt)
        .count() as u32;
    let tagged_count = photos.iter().filter(|p| p.tag_id.is_some()).count() as u32;
    let needs_review = photos
        .iter()
        .filter(|p| p.needs_review || p.corrupt)
        .count() as u32;
    let quality_issue_count = photos
        .iter()
        .filter(|p| matches!(p.quality.status.as_str(), "warn" | "bad"))
        .count() as u32;
    let status = if count == 0 {
        "空分类"
    } else if needs_review > 0 {
        "需核对"
    } else if ready_count == count {
        "已就绪"
    } else {
        "待确认"
    };
    let mut times: Vec<&str> = photos
        .iter()
        .filter_map(|p| p.capture_text.as_deref())
        .collect();
    times.sort();
    GroupView {
        id: group.id.clone(),
        name: group.name.clone(),
        sort_order: group.sort_order,
        count,
        ready_count,
        tagged_count,
        needs_review,
        status: status.into(),
        start_time: times.first().map(|s| (*s).to_string()),
        end_time: times.last().map(|s| (*s).to_string()),
        preset_id: group.preset_id.clone(),
        preset_name: group.preset_name.clone(),
        suggested_tags: group.suggested_tags.clone(),
        duplicate_count: 0,
        quality_issue_count,
    }
}

fn photo_view(task: &Task, tags: &[TagItem], photo: &Photo, seq: u32) -> PhotoView {
    let group_name = photo.group_id.as_ref().and_then(|id| {
        task.groups
            .iter()
            .find(|g| &g.id == id)
            .map(|g| g.name.clone())
    });
    let tag = photo
        .tag_id
        .as_ref()
        .and_then(|id| tags.iter().find(|t| &t.id == id));
    let resolved = resolve_photo_target(task, photo, seq, tags).ok();
    let order_note = if photo.time_uncertain && photo.equal_timestamp {
        format!(
            "时间相同且存在时区不确定性，建议核对顺序：{}",
            photo.time_note
        )
    } else if photo.time_uncertain || photo.time_source != "exif_original" {
        photo.time_note.clone()
    } else if photo.equal_timestamp {
        "时间相同，已按稳定路径顺序排列。".to_string()
    } else {
        String::new()
    };
    PhotoView {
        id: photo.id.clone(),
        sort_index: photo.sort_index,
        original_scan_index: photo.original_scan_index,
        file_name: photo.file_name.clone(),
        source_path: photo.source_path.clone(),
        extension: photo.extension.clone(),
        size: photo.size,
        capture_text: photo.capture_text.clone(),
        exif_offset: photo.exif_offset.clone(),
        sort_time_text: photo.sort_time_text.clone(),
        time_uncertain: photo.time_uncertain,
        time_source: photo.time_source.clone(),
        time_note: photo.time_note.clone(),
        equal_timestamp: photo.equal_timestamp,
        tie_break: photo.tie_break.clone(),
        tie_break_is_capture_order: photo.tie_break_is_capture_order,
        order_note,
        corrupt: photo.corrupt,
        preview_state: photo.preview_state.clone(),
        quality: photo.quality.clone(),
        group_id: photo.group_id.clone(),
        group_name,
        tag_id: photo.tag_id.clone(),
        tag_name: tag.map(|t| t.name.clone()),
        short_label: tag.map(|t| t.short_label.clone()),
        custom_name: photo.custom_name.clone(),
        preview_stem: resolved
            .as_ref()
            .map(|r| r.file_stem.clone())
            .unwrap_or_default(),
        preview_rel_path: resolved
            .map(|r| r.relative_path)
            .unwrap_or_else(|| photo.file_name.clone()),
        confirmed: photo.confirmed,
        needs_review: photo.needs_review,
        excluded: photo.excluded,
    }
}

fn reindex(task: &mut Task) {
    for (i, photo) in task.photos.iter_mut().enumerate() {
        photo.sort_index = i as u32;
    }
}

fn snapshot_labeled(task: &Task, label: &str) -> EditSnap {
    EditSnap {
        label: label.to_string(),
        groups: task.groups.clone(),
        dir_layout: task.dir_layout.clone(),
        dir_pattern: task.dir_pattern.clone(),
        naming_rule: task.naming_rule.clone(),
        photos: task
            .photos
            .iter()
            .map(|p| PhotoSnap {
                id: p.id.clone(),
                source_path: p.source_path.clone(),
                sort_index: p.sort_index,
                group_id: p.group_id.clone(),
                tag_id: p.tag_id.clone(),
                custom_name: p.custom_name.clone(),
                confirmed: p.confirmed,
                needs_review: p.needs_review,
                excluded: p.excluded,
            })
            .collect(),
    }
}

fn recheck_sources(task: &mut Task) -> bool {
    let mut changed = false;
    for photo in &mut task.photos {
        match util::metadata_any(Path::new(&photo.source_path)) {
            Ok(meta) => {
                if meta.len() != photo.size || util::mtime_ns(&meta) != photo.mtime_ns {
                    changed = true;
                    photo.needs_review = true;
                    photo.confirmed = false;
                }
            }
            Err(_) => {
                changed = true;
                photo.needs_review = true;
                photo.confirmed = false;
            }
        }
    }
    changed
}

pub fn run_quality_inspection_batch(
    photos: &[Photo],
    mut on_tick: impl FnMut(&QualityTick) -> ScanControl,
) -> (Vec<(String, PhotoQuality)>, bool) {
    let pending: Vec<&Photo> = photos
        .iter()
        .filter(|p| {
            !p.excluded && !p.corrupt && p.preview_state == "available" && !p.quality.pixel_checked
        })
        .collect();
    let total = pending.len() as u32;
    let mut updates = Vec::with_capacity(pending.len());
    if pending.is_empty() {
        return (updates, false);
    }
    let workers = std::thread::available_parallelism()
        .map(|n| n.get().clamp(2, 6))
        .unwrap_or(4);
    let chunk_size = workers * 3;
    let mut cancelled = false;
    let mut processed = 0u32;

    for chunk in pending.chunks(chunk_size) {
        let first_name = chunk
            .first()
            .map(|p| p.file_name.clone())
            .unwrap_or_default();
        if on_tick(&QualityTick {
            processed,
            total,
            current_file: first_name,
            cancelled: false,
        }) == ScanControl::Cancel
        {
            cancelled = true;
            break;
        }
        let batch_out: Vec<(String, PhotoQuality)> = std::thread::scope(|scope| {
            let handles: Vec<_> = chunk
                .iter()
                .map(|photo| {
                    scope.spawn(move || {
                        let source_dims = photo.quality.width.zip(photo.quality.height);
                        let q = match decode_preview_image(Path::new(&photo.source_path), 360) {
                            Ok(img) => {
                                let metrics = analyze_image_pixels(&img, source_dims);
                                merge_pixel_quality(&photo.quality, metrics)
                            }
                            Err(err_msg) => {
                                let mut bad = photo.quality.clone();
                                if !bad.issues.iter().any(|c| c == "corrupt") {
                                    bad.issues.push("corrupt".to_string());
                                }
                                bad.status = "bad".to_string();
                                bad.pixel_checked = true;
                                bad.summary = format!("无法解码像素画面：{err_msg}");
                                bad
                            }
                        };
                        (photo.id.clone(), q)
                    })
                })
                .collect();
            handles
                .into_iter()
                .filter_map(|h| h.join().ok())
                .collect()
        });
        processed += batch_out.len() as u32;
        updates.extend(batch_out);
    }
    let _ = on_tick(&QualityTick {
        processed,
        total,
        current_file: String::new(),
        cancelled,
    });
    (updates, cancelled)
}
