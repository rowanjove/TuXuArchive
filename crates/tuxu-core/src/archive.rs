use crate::db;
use crate::error::{err, Result};
use crate::model::*;
use crate::names::validate_component;
use crate::naming_engine::{compute_seq_numbers, resolve_photo_target};
use crate::util::{self, now_iso};
use rusqlite::Connection;
use std::collections::{BTreeSet, HashSet};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const DB_CHECKPOINT_INTERVAL_ITEMS: usize = 16;
const DB_CHECKPOINT_INTERVAL: Duration = Duration::from_millis(150);
const PROGRESS_EMIT_INTERVAL_ITEMS: usize = 5;
const PROGRESS_EMIT_INTERVAL: Duration = Duration::from_millis(80);

struct CopyScratch {
    buf: Vec<u8>,
    verified_dirs: HashSet<PathBuf>,
}

impl CopyScratch {
    fn new() -> Self {
        Self {
            buf: vec![0u8; 1024 * 1024],
            verified_dirs: HashSet::new(),
        }
    }
}

enum CopyOutcome {
    Copied(u64, String),
    Cancelled,
}

#[allow(dead_code)]
pub fn build_plan(task: &Task, tags: &[TagItem]) -> Result<ExecutionPlan> {
    build_plan_for(task, tags, None)
}

pub fn build_plan_for(
    task: &Task,
    tags: &[TagItem],
    free_override: Option<u64>,
) -> Result<ExecutionPlan> {
    let mut issues = Vec::new();
    if !task.scan_complete {
        let unread = task.ignored_reasons.get("unreadable").copied().unwrap_or(0);
        if unread > 0 {
            issues.push(block(
                "",
                format!("有 {unread} 个路径无法读取，扫描未完成，不能归档"),
            ));
        } else {
            issues.push(block("", "扫描尚未完成，不能归档"));
        }
    }
    let needs_task = pattern_uses_task(task);
    let needs_group = pattern_uses_group(task);

    if needs_task && task.task_name.trim().is_empty() {
        issues.push(block("", "当前目录结构包含任务名称，请先填写任务名称"));
    }
    if task.output_root.trim().is_empty() {
        issues.push(block("", "尚未选择输出根目录"));
    }
    if !task.mappings_valid {
        issues.push(issue(
            "E003",
            true,
            "源图片已变化或任务处于待复核状态，请先核对后再归档",
            None,
        ));
    }
    if !task.task_name.trim().is_empty() {
        if let Err(e) = validate_component(&task.task_name, "任务名称") {
            issues.push(issue("E005", true, e.message, None));
        }
    }
    for group in &task.groups {
        if let Err(e) = validate_component(&group.name, "分类名称") {
            issues.push(issue("E005", true, e.message, None));
        }
    }
    for (i, group) in task.groups.iter().enumerate() {
        for other in task.groups.iter().skip(i + 1) {
            if group.name.eq_ignore_ascii_case(&other.name) {
                issues.push(issue(
                    "E001",
                    true,
                    format!(
                        "分类名称「{}」与「{}」在 Windows 下对应同一个目录",
                        group.name, other.name
                    ),
                    None,
                ));
            }
        }
    }

    let seq_map = compute_seq_numbers(task);

    for photo in &task.photos {
        if photo.time_source != "exif_original" || photo.time_uncertain {
            let msg = if photo.time_uncertain && photo.time_source == "exif_original" {
                format!(
                    "{} 缺少 EXIF 时区或与同批时区混排，建议核对顺序（{}）",
                    photo.file_name,
                    photo.capture_text.as_deref().unwrap_or("无时间")
                )
            } else {
                format!(
                    "{} 没有可信的 EXIF 原始拍摄时间（{}）",
                    photo.file_name, photo.time_source
                )
            };
            issues.push(issue("W001", false, msg, Some(photo.id.clone())));
        }
        if photo.equal_timestamp {
            issues.push(issue(
                "W002",
                false,
                format!(
                    "{} 与其他图片时间相同，已按稳定路径次序排列。",
                    photo.file_name
                ),
                Some(photo.id.clone()),
            ));
        }
        if photo.excluded {
            continue;
        }
        if needs_group && photo.group_id.is_none() {
            issues.push(issue(
                "W003",
                true,
                format!("{} 尚未分配分类", photo.file_name),
                Some(photo.id.clone()),
            ));
            continue;
        }
        if photo.corrupt {
            issues.push(issue(
                "E003",
                true,
                format!("图片文件已损坏或无法读取：{}（可选择排除此图）", photo.file_name),
                Some(photo.id.clone()),
            ));
        } else if photo.needs_review || !photo.confirmed {
            issues.push(issue(
                "W004",
                true,
                format!("{} 处于待核对状态", photo.file_name),
                Some(photo.id.clone()),
            ));
        }
        if !source_matches(photo) {
            issues.push(issue(
                "E003",
                true,
                format!("源图片不存在或已变化：{}", photo.file_name),
                Some(photo.id.clone()),
            ));
        }
    }

    let mut items = Vec::new();
    let mut directories = BTreeSet::new();
    let mut seen_targets: BTreeSet<String> = BTreeSet::new();
    if !task.output_root.trim().is_empty() && (!needs_task || !task.task_name.trim().is_empty()) {
        let output = PathBuf::from(&task.output_root);
        let source = PathBuf::from(&task.source_root);
        if trees_conflict(&source, &output) {
            issues.push(issue(
                "E006",
                true,
                "源文件夹与输出目录互相包含或相同，请更换输出根目录",
                None,
            ));
        }
        if !output_writable(&output) {
            issues.push(issue(
                "E004",
                true,
                "输出目录不可写，或上级目录不存在",
                None,
            ));
        }
        let mut active_sorted: Vec<&Photo> = task.photos.iter().filter(|p| !p.excluded).collect();
        active_sorted.sort_by_key(|p| p.sort_index);

        for photo in active_sorted {
            if needs_group && photo.group_id.is_none() {
                continue;
            }
            let seq = seq_map.get(&photo.id).copied().unwrap_or(1);
            let resolved = match resolve_photo_target(task, photo, seq, tags) {
                Ok(r) => r,
                Err(e) => {
                    issues.push(issue("E005", true, e.message, Some(photo.id.clone())));
                    continue;
                }
            };
            let mut cur_dir = output.clone();
            for seg in &resolved.dir_segments {
                cur_dir = cur_dir.join(seg);
                directories.insert(cur_dir.display().to_string());
            }
            let target = cur_dir.join(&resolved.file_name);
            let key = target.to_string_lossy().to_lowercase();
            if !seen_targets.insert(key) {
                issues.push(issue(
                    "E002",
                    true,
                    format!(
                        "目标文件名重复（请检查命名规则是否包含连续编号 {{seq}} 或原文件名）：{}",
                        target.display()
                    ),
                    Some(photo.id.clone()),
                ));
            }
            if path_exists(&target) {
                issues.push(issue(
                    "E002",
                    true,
                    format!("目标已存在，不会覆盖：{}", target.display()),
                    Some(photo.id.clone()),
                ));
            }
            let path_text = target.to_string_lossy();
            if path_text.encode_utf16().count() > 30000 {
                issues.push(issue(
                    "E005",
                    true,
                    format!("目标路径过长：{path_text}"),
                    Some(photo.id.clone()),
                ));
            }
            items.push(PlanItem {
                photo_id: photo.id.clone(),
                source_path: photo.source_path.clone(),
                source_name: photo.file_name.clone(),
                target_path: target.display().to_string(),
                relative_path: resolved.relative_path,
                group_name: resolved.group_name,
                target_name: resolved.file_stem,
                extension: photo.extension.clone(),
                size: photo.size,
                mtime_ns: photo.mtime_ns,
                blocked: false,
            });
        }
        let bytes: u64 = items.iter().map(|i| i.size).sum();
        let free = free_override.or_else(|| util::free_bytes(&output));
        let mut shortfall_bytes = 0u64;
        if let Some(free) = free {
            let margin = (bytes / 20).max(16 * 1024 * 1024);
            let need = bytes.saturating_add(margin);
            if free < need {
                shortfall_bytes = need - free;
                issues.push(issue(
                    "E004",
                    true,
                    format!(
                        "磁盘剩余空间不足。需要约 {} 字节，可用 {} 字节，还差 {} 字节",
                        need, free, shortfall_bytes
                    ),
                    None,
                ));
            }
        }
        if items.is_empty() && !issues.iter().any(|issue| issue.blocks) {
            issues.push(block("", "没有可复制的图片"));
        }
        let blocked = issues.iter().any(|i| i.blocks);
        for item in &mut items {
            item.blocked = blocked;
        }
        return Ok(ExecutionPlan {
            id: util::new_id("plan"),
            task_id: task.id.clone(),
            revision: task.revision,
            blocked,
            copy_count: items.len() as u32,
            blocked_count: issues.iter().filter(|i| i.blocks).count() as u32,
            bytes,
            free_bytes: free,
            shortfall_bytes,
            directories: directories.into_iter().collect(),
            group_counts: group_file_counts(&items),
            created_at: now_iso(),
            issues,
            items,
        });
    }

    let blocked = issues.iter().any(|i| i.blocks)
        || task.output_root.trim().is_empty()
        || (needs_task && task.task_name.trim().is_empty());
    Ok(ExecutionPlan {
        id: util::new_id("plan"),
        task_id: task.id.clone(),
        revision: task.revision,
        blocked,
        copy_count: 0,
        blocked_count: issues.iter().filter(|i| i.blocks).count() as u32,
        bytes: 0,
        free_bytes: None,
        shortfall_bytes: 0,
        directories: Vec::new(),
        group_counts: Vec::new(),
        created_at: now_iso(),
        issues,
        items,
    })
}

fn group_file_counts(items: &[PlanItem]) -> Vec<GroupFileCount> {
    let mut counts = Vec::<GroupFileCount>::new();
    for item in items {
        if let Some(found) = counts
            .iter_mut()
            .find(|row| row.group_name == item.group_name)
        {
            found.count += 1;
        } else {
            counts.push(GroupFileCount {
                group_name: item.group_name.clone(),
                count: 1,
            });
        }
    }
    counts
}

fn source_matches(photo: &Photo) -> bool {
    let path = Path::new(&photo.source_path);
    match util::metadata_any(path) {
        Ok(meta) => meta.len() == photo.size && util::mtime_ns(&meta) == photo.mtime_ns,
        Err(_) => false,
    }
}

fn path_exists(path: &Path) -> bool {
    util::exists_any(path)
}

fn validate_intended_components(path: &Path) -> bool {
    use std::path::Component;
    if path.as_os_str().is_empty() {
        return false;
    }
    for comp in path.components() {
        match comp {
            Component::Prefix(_) | Component::RootDir | Component::CurDir => {}
            Component::ParentDir => return false,
            Component::Normal(part) => {
                let s = part.to_string_lossy();
                if validate_component(&s, "输出目录").is_err() {
                    return false;
                }
            }
        }
    }
    true
}

fn output_writable(output: &Path) -> bool {
    if !validate_intended_components(output) {
        return false;
    }
    if util::exists_any(output) {
        return util::is_dir_any(output) && probe(output);
    }
    let mut cur = output.parent();
    while let Some(ancestor) = cur {
        if ancestor.as_os_str().is_empty() {
            return probe(Path::new("."));
        }
        if util::exists_any(ancestor) {
            return util::is_dir_any(ancestor) && probe(ancestor);
        }
        cur = ancestor.parent();
    }
    false
}

fn probe(dir: &Path) -> bool {
    let path = dir.join(format!(".tuxu-probe-{}", std::process::id()));
    match util::write_any(&path, b"ok") {
        Ok(()) => {
            let _ = util::remove_file_any(&path);
            true
        }
        Err(_) => false,
    }
}

fn trees_conflict(source: &Path, output: &Path) -> bool {
    let s = norm_existing(source);
    let o = norm_intended(output);
    if s == o {
        return true;
    }
    o.starts_with(&(s.clone() + "\\")) || s.starts_with(&(o + "\\"))
}

pub fn check_paths(source_root: &str, output_root: &str) -> PathCheckView {
    let src_trim = source_root.trim();
    let out_trim = output_root.trim();

    let (source_exists, source_is_dir) = if src_trim.is_empty() {
        (false, false)
    } else {
        let s_path = Path::new(src_trim);
        (util::exists_any(s_path), util::is_dir_any(s_path))
    };

    let output_writable_flag = if out_trim.is_empty() {
        false
    } else {
        output_writable(Path::new(out_trim))
    };

    let conflict = !src_trim.is_empty()
        && !out_trim.is_empty()
        && trees_conflict(Path::new(src_trim), Path::new(out_trim));

    let (valid, reason) = if conflict {
        (
            false,
            "源文件夹与输出目录不能互相包含或相同，请把输出目录选在源文件夹之外。".into(),
        )
    } else if !src_trim.is_empty() && !source_exists {
        (false, "源图片文件夹不存在或已断开连接。".into())
    } else if !src_trim.is_empty() && !source_is_dir {
        (false, "所选源路径不是有效的文件夹。".into())
    } else if !out_trim.is_empty() && !output_writable_flag {
        (
            false,
            "输出目录无法写入或上级目录不存在，请重新选择可写目录。".into(),
        )
    } else if src_trim.is_empty() || out_trim.is_empty() {
        (
            false,
            if out_trim.is_empty() {
                "请选择归档输出目录。".into()
            } else {
                "请选择源图片文件夹。".into()
            },
        )
    } else {
        (true, String::new())
    };

    PathCheckView {
        valid,
        source_exists,
        source_is_dir,
        output_writable: output_writable_flag,
        conflict,
        reason,
    }
}

fn norm_existing(path: &Path) -> String {
    let canon = util::canonicalize_any(path).unwrap_or_else(|_| util::plain_path(path));
    canon
        .to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}

fn norm_intended(path: &Path) -> String {
    if util::exists_any(path) {
        return norm_existing(path);
    }
    use std::path::Component;
    let mut resolved = PathBuf::new();
    for comp in util::plain_path(path).components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                let _ = resolved.pop();
            }
            other => resolved.push(other.as_os_str()),
        }
    }
    if util::exists_any(&resolved) {
        return norm_existing(&resolved);
    }
    let mut tail: Vec<String> = Vec::new();
    let mut cur = resolved.clone();
    while let Some(name) = cur.file_name() {
        tail.push(name.to_string_lossy().to_lowercase());
        if !cur.pop() {
            break;
        }
        if util::exists_any(&cur) {
            let mut base = norm_existing(&cur);
            for seg in tail.into_iter().rev() {
                if !base.ends_with('\\') {
                    base.push('\\');
                }
                base.push_str(&seg);
            }
            return base;
        }
    }
    util::plain_path(&resolved)
        .to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}

fn issue(code: &str, blocks: bool, message: impl Into<String>, photo_id: Option<String>) -> Issue {
    Issue {
        code: code.to_string(),
        severity: if blocks {
            "error".into()
        } else {
            "warning".into()
        },
        blocks,
        message: message.into(),
        photo_id,
    }
}

fn block(code: &str, message: impl Into<String>) -> Issue {
    issue(code, true, message, None)
}

fn blocking_after_skips(plan: &ExecutionPlan, skip: &BTreeSet<String>) -> Vec<Issue> {
    let active: Vec<&PlanItem> = plan
        .items
        .iter()
        .filter(|item| !skip.contains(&item.photo_id))
        .collect();
    let mut hard = Vec::new();
    for found in &plan.issues {
        if !found.blocks {
            continue;
        }
        if found.code == "E002" {
            let Some(photo_id) = &found.photo_id else {
                hard.push(found.clone());
                continue;
            };
            if skip.contains(photo_id) {
                continue;
            }
            let Some(item) = plan.items.iter().find(|item| &item.photo_id == photo_id) else {
                hard.push(found.clone());
                continue;
            };
            let key = item.target_path.to_lowercase();
            let duplicated = active
                .iter()
                .filter(|item| item.target_path.to_lowercase() == key)
                .count()
                > 1;
            if duplicated || path_exists(Path::new(&item.target_path)) {
                hard.push(found.clone());
            }
            continue;
        }
        if matches!(found.code.as_str(), "W003" | "W004")
            && found.photo_id.as_ref().is_some_and(|id| skip.contains(id))
        {
            continue;
        }
        if found.code == "E004" && found.message.contains("磁盘剩余空间不足") {
            let bytes: u64 = active.iter().map(|item| item.size).sum();
            let margin = (bytes / 20).max(16 * 1024 * 1024);
            if plan
                .free_bytes
                .is_some_and(|free| free >= bytes.saturating_add(margin))
            {
                continue;
            }
        }
        hard.push(found.clone());
    }
    hard
}

pub fn execute(
    conn: &Connection,
    task: &Task,
    plan: &ExecutionPlan,
    opts: &ExecuteOptions,
    tags: &[TagItem],
    free_override: Option<u64>,
    progress: &mut dyn FnMut(&CopyTick),
) -> Result<ExecuteReport> {
    if task.revision != plan.revision || task.id != plan.task_id {
        return Err(err("", "任务在生成计划后已修改，请重新预览"));
    }
    let fresh = build_plan_for(task, tags, free_override)?;
    let skip: BTreeSet<String> = opts
        .confirmed_skips
        .iter()
        .filter(|id| {
            fresh.issues.iter().any(|issue| {
                issue.blocks
                    && issue.photo_id.as_deref() == Some(id.as_str())
                    && ((issue.code == "E002" && !issue.message.starts_with("目标文件名重复"))
                        || matches!(issue.code.as_str(), "W003" | "W004"))
            })
        })
        .cloned()
        .collect();
    let hard = blocking_after_skips(&fresh, &skip);
    if !hard.is_empty() || (fresh.blocked && skip.is_empty()) {
        return Ok(ExecuteReport {
            job_id: String::new(),
            status: "blocked".into(),
            succeeded: 0,
            failed: 0,
            skipped: 0,
            not_started: fresh.items.len() as u32,
            output_root: task.output_root.clone(),
            items: Vec::new(),
            issues: if hard.is_empty() {
                fresh.issues
            } else {
                hard
            },
        });
    }
    if fresh.items.is_empty() {
        return Ok(ExecuteReport {
            job_id: String::new(),
            status: "blocked".into(),
            succeeded: 0,
            failed: 0,
            skipped: 0,
            not_started: 0,
            output_root: task.output_root.clone(),
            items: Vec::new(),
            issues: vec![block("", "没有可复制的图片")],
        });
    }

    let mut items: Vec<JobItem> = fresh
        .items
        .iter()
        .map(|item| {
            let photo = task.photos.iter().find(|photo| photo.id == item.photo_id);
            JobItem {
                photo_id: item.photo_id.clone(),
                source_path: item.source_path.clone(),
                target_path: item.target_path.clone(),
                relative_path: item.relative_path.clone(),
                target_name: item.target_name.clone(),
                extension: item.extension.clone(),
                group_name: item.group_name.clone(),
                expected_size: item.size,
                expected_mtime_ns: item.mtime_ns,
                status: "not_started".into(),
                message: String::new(),
                code: String::new(),
                temp_path: None,
                sha256: None,
                bytes: 0,
                source_name: item.source_name.clone(),
                capture_text: photo
                    .and_then(|photo| photo.capture_text.clone())
                    .unwrap_or_default(),
                time_source: photo
                    .map(|photo| photo.time_source.clone())
                    .unwrap_or_default(),
            }
        })
        .collect();

    for skip_id in &skip {
        if items.iter().any(|it| &it.photo_id == skip_id) {
            continue;
        }
        let Some(photo) = task.photos.iter().find(|p| &p.id == skip_id) else {
            continue;
        };
        let is_w003 = fresh
            .issues
            .iter()
            .any(|iss| iss.photo_id.as_deref() == Some(skip_id.as_str()) && iss.code == "W003");
        let group_name = photo
            .group_id
            .as_ref()
            .and_then(|gid| task.groups.iter().find(|g| &g.id == gid))
            .map(|g| g.name.clone())
            .unwrap_or_else(|| "未分类".into());
        items.push(JobItem {
            photo_id: photo.id.clone(),
            source_path: photo.source_path.clone(),
            target_path: String::new(),
            relative_path: String::new(),
            target_name: String::new(),
            extension: photo.extension.clone(),
            group_name,
            expected_size: photo.size,
            expected_mtime_ns: photo.mtime_ns,
            status: "skipped".into(),
            message: if is_w003 {
                "用户选择仅归档已就绪部分，跳过未分类图片".into()
            } else {
                "用户选择仅归档已就绪部分，跳过待核对图片".into()
            },
            code: if is_w003 { "W003".into() } else { "W004".into() },
            temp_path: None,
            sha256: None,
            bytes: 0,
            source_name: photo.file_name.clone(),
            capture_text: photo.capture_text.clone().unwrap_or_default(),
            time_source: photo.time_source.clone(),
        });
    }

    let mut job = JobManifest {
        job_id: util::new_id("job"),
        task_id: task.id.clone(),
        output_root: task.output_root.clone(),
        task_name: task.task_name.clone(),
        strict_hash: task.strict_hash,
        status: "running".into(),
        created_at: now_iso(),
        items,
        created_dirs: Vec::new(),
        files: Vec::new(),
    };
    db::save_job(conn, &job)?;

    let total = job.items.len();
    emit_progress(&job, 0, progress);

    if total > 8 && opts.stop_after_temp_index.is_none() && opts.cancel_after_index.is_none() {
        let mut dirty_skips = Vec::new();
        let mut pending_indices = Vec::with_capacity(total);
        for index in 0..total {
            if job.items[index].status == "skipped" {
                continue;
            }
            if skip.contains(&job.items[index].photo_id) {
                mark_skipped_item(&mut job.items[index], &fresh.issues);
                dirty_skips.push(index);
            } else {
                pending_indices.push(index);
            }
        }
        if !dirty_skips.is_empty() {
            db::checkpoint_job(conn, &job, &dirty_skips, false)?;
            emit_progress(&job, total.saturating_sub(1), progress);
        }
        if !pending_indices.is_empty() {
            run_parallel_copy(
                conn,
                &mut job,
                &pending_indices,
                task.strict_hash,
                opts.cancel.as_deref(),
                progress,
                "已复制",
            )?;
            return Ok(report_from(&job, &fresh.issues));
        }
        job.status = final_status(&job);
        db::checkpoint_job(conn, &job, &[], true)?;
        emit_progress(&job, total.saturating_sub(1), progress);
        return Ok(report_from(&job, &fresh.issues));
    }

    let mut scratch = CopyScratch::new();
    let mut dirty_indices: Vec<usize> = Vec::new();
    let mut seen_file_paths: HashSet<String> = job.files.iter().map(|f| f.path.clone()).collect();
    let mut last_db_flush = Instant::now();
    let mut last_progress = Instant::now();
    let mut items_since_progress = 0usize;

    for index in 0..total {
        if cancel_requested(opts) {
            job.status = stopped_status(&job);
            db::checkpoint_job(conn, &job, &dirty_indices, true)?;
            emit_progress(&job, index, progress);
            return Ok(report_from(&job, &fresh.issues));
        }
        if job.items[index].status == "skipped" {
            items_since_progress += 1;
            if should_emit_progress(index, total, items_since_progress, last_progress) {
                emit_progress(&job, index, progress);
                items_since_progress = 0;
                last_progress = Instant::now();
            }
            continue;
        }
        if skip.contains(&job.items[index].photo_id) {
            mark_skipped_item(&mut job.items[index], &fresh.issues);
            dirty_indices.push(index);
            items_since_progress += 1;
            if opts.cancel_after_index == Some(index) {
                job.status = stopped_status(&job);
                db::checkpoint_job(conn, &job, &dirty_indices, true)?;
                emit_progress(&job, index, progress);
                return Ok(report_from(&job, &fresh.issues));
            }
            if should_flush_db(&dirty_indices, last_db_flush) {
                db::checkpoint_job(conn, &job, &dirty_indices, false)?;
                dirty_indices.clear();
                last_db_flush = Instant::now();
            }
            if should_emit_progress(index, total, items_since_progress, last_progress) {
                emit_progress(&job, index, progress);
                items_since_progress = 0;
                last_progress = Instant::now();
            }
            continue;
        }
        if opts.stop_after_temp_index == Some(index) {
            let temp = temp_path(&job.items[index].target_path, &job.job_id, index);
            if let Some(parent) = Path::new(&temp).parent() {
                let _ = fs::create_dir_all(parent);
            }
            let mut handle = util::create_owned_temp(Path::new(&temp))
                .map_err(|e| err("E007", e.to_string()))?;
            db::save_copy_intent(
                conn,
                &CopyIntent {
                    job_id: job.job_id.clone(),
                    index,
                    source: job.items[index].source_path.clone(),
                    target: job.items[index].target_path.clone(),
                    temp: temp.clone(),
                    identity: util::file_identity(&handle)
                        .map_err(|e| err("E007", e.to_string()))?,
                    bytes: job.items[index].expected_size,
                    hash: None,
                },
            )?;
            handle
                .write_all(b"partial")
                .map_err(|e| err("E007", e.to_string()))?;
            drop(handle);
            job.items[index].status = "in_progress".into();
            job.items[index].temp_path = Some(temp);
            job.items[index].code = "E007".into();
            job.items[index].message = "复制在临时文件阶段中断".into();
            dirty_indices.push(index);
            job.status = "interrupted".into();
            db::checkpoint_job(conn, &job, &dirty_indices, true)?;
            emit_progress(&job, index, progress);
            return Ok(report_from(&job, &fresh.issues));
        }
        let strict = task.strict_hash;
        let job_id = job.job_id.clone();
        let mut created = std::mem::take(&mut job.created_dirs);
        let copied = copy_item(
            &mut scratch,
            &mut created,
            &mut job.items[index],
            strict,
            (&job_id, index),
            opts.cancel.as_deref(),
            conn,
        );
        job.created_dirs = created;
        match copied {
            Ok(CopyOutcome::Copied(bytes, hash)) => {
                let path = job.items[index].target_path.clone();
                job.items[index].status = "succeeded".into();
                job.items[index].bytes = bytes;
                job.items[index].sha256 = Some(hash.clone());
                job.items[index].message = "已复制".into();
                job.items[index].code.clear();
                if seen_file_paths.insert(path.clone()) {
                    job.files.push(JobFile {
                        path,
                        size: bytes,
                        sha256: hash,
                        created_by_job: true,
                        identity: None,
                    });
                }
                dirty_indices.push(index);
            }
            Ok(CopyOutcome::Cancelled) => {
                job.items[index].status = "not_started".into();
                job.items[index].temp_path = None;
                job.items[index].code.clear();
                job.items[index].message.clear();
                dirty_indices.push(index);
                job.status = stopped_status(&job);
                db::checkpoint_job(conn, &job, &dirty_indices, true)?;
                emit_progress(&job, index, progress);
                return Ok(report_from(&job, &fresh.issues));
            }
            Err(e) => {
                job.items[index].status = "failed".into();
                job.items[index].code = e.code;
                job.items[index].message = e.message;
                dirty_indices.push(index);
                db::checkpoint_job(conn, &job, &dirty_indices, true)?;
                dirty_indices.clear();
                last_db_flush = Instant::now();
            }
        }
        items_since_progress += 1;
        if opts.cancel_after_index == Some(index) {
            job.status = stopped_status(&job);
            db::checkpoint_job(conn, &job, &dirty_indices, true)?;
            emit_progress(&job, index, progress);
            return Ok(report_from(&job, &fresh.issues));
        }
        if should_flush_db(&dirty_indices, last_db_flush) {
            db::checkpoint_job(conn, &job, &dirty_indices, false)?;
            dirty_indices.clear();
            last_db_flush = Instant::now();
        }
        if should_emit_progress(index, total, items_since_progress, last_progress) {
            emit_progress(&job, index, progress);
            items_since_progress = 0;
            last_progress = Instant::now();
        }
    }
    job.status = final_status(&job);
    db::checkpoint_job(conn, &job, &dirty_indices, true)?;
    emit_progress(&job, total.saturating_sub(1), progress);
    Ok(report_from(&job, &fresh.issues))
}

fn mark_skipped_item(item: &mut JobItem, issues: &[Issue]) {
    let pid = item.photo_id.as_str();
    let is_w003 = issues
        .iter()
        .any(|iss| iss.photo_id.as_deref() == Some(pid) && iss.code == "W003");
    let is_w004 = issues
        .iter()
        .any(|iss| iss.photo_id.as_deref() == Some(pid) && iss.code == "W004");
    item.status = "skipped".into();
    if is_w003 {
        item.code = "W003".into();
        item.message = "用户选择仅归档已就绪部分，跳过未分类图片".into();
    } else if is_w004 {
        item.code = "W004".into();
        item.message = "用户选择仅归档已就绪部分，跳过待核对图片".into();
    } else {
        item.code = "E002".into();
        item.message = "用户确认跳过已存在的目标".into();
    }
}

fn run_parallel_copy(
    conn: &Connection,
    job: &mut JobManifest,
    pending_indices: &[usize],
    strict: bool,
    cancel: Option<&AtomicBool>,
    progress: &mut dyn FnMut(&CopyTick),
    success_msg: &str,
) -> Result<()> {
    let work: Vec<(usize, String, String, u64, i64)> = pending_indices
        .iter()
        .map(|&idx| {
            let item = &job.items[idx];
            (
                idx,
                item.source_path.clone(),
                item.target_path.clone(),
                item.expected_size,
                item.expected_mtime_ns,
            )
        })
        .collect();
    let dirs_lock = std::sync::Mutex::new((
        std::mem::take(&mut job.created_dirs),
        HashSet::<PathBuf>::new(),
    ));
    let cursor = std::sync::atomic::AtomicUsize::new(0);
    let worker_count = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2)
        .clamp(1, 2)
        .min(work.len());
    let job_id = job.job_id.clone();

    let mut dirty_indices: Vec<usize> = Vec::new();
    let mut seen_file_paths: HashSet<String> = job.files.iter().map(|f| f.path.clone()).collect();
    let mut last_db_flush = Instant::now();
    let mut last_progress = Instant::now();
    let mut items_since_progress = 0usize;
    let mut any_cancelled = false;
    let mut first_db_err: Option<crate::error::Error> = None;

    std::thread::scope(|s| {
        enum CopyEvent {
            Intent(CopyIntent, std::sync::mpsc::SyncSender<Result<()>>),
            Done(usize, Result<CopyOutcome>),
        }
        let (tx, rx) = std::sync::mpsc::channel::<CopyEvent>();
        for _ in 0..worker_count {
            let tx_w = tx.clone();
            let cursor_ref = &cursor;
            let work_ref = &work;
            let dirs_ref = &dirs_lock;
            let job_id_ref = &job_id;
            s.spawn(move || {
                let mut buf = vec![0u8; 1024 * 1024];
                loop {
                    if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
                        break;
                    }
                    let pos = cursor_ref.fetch_add(1, Ordering::Relaxed);
                    let Some((
                        item_index,
                        source_path,
                        target_path,
                        expected_size,
                        expected_mtime_ns,
                    )) = work_ref.get(pos)
                    else {
                        break;
                    };
                    let res = copy_item_parts(
                        &mut buf,
                        |parent| {
                            let mut guard =
                                dirs_ref.lock().map_err(|_| err("E004", "目录锁异常"))?;
                            let (created, verified) = &mut *guard;
                            ensure_dir_cached(parent, created, verified)
                        },
                        CopySpec {
                            source_path,
                            target_path,
                            expected_size: *expected_size,
                            expected_mtime_ns: *expected_mtime_ns,
                            strict,
                            job_id: job_id_ref,
                            index: *item_index,
                            cancel,
                        },
                        |intent| {
                            let (reply, ack) = std::sync::mpsc::sync_channel(0);
                            tx_w.send(CopyEvent::Intent(intent.clone(), reply))
                                .map_err(|_| err("E007", "记账线程已停止"))?;
                            ack.recv().map_err(|_| err("E007", "提交意图未确认"))?
                        },
                    );
                    if tx_w.send(CopyEvent::Done(*item_index, res)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);

        let mut deferred = std::collections::VecDeque::new();
        loop {
            let event = match deferred.pop_front().or_else(|| rx.recv().ok()) {
                Some(event) => event,
                None => break,
            };
            let (item_index, res) = match event {
                CopyEvent::Intent(intent, reply) => {
                    let mut intents = vec![intent];
                    let mut replies = vec![reply];
                    while intents.len() < worker_count {
                        match rx.recv_timeout(Duration::from_millis(2)) {
                            Ok(CopyEvent::Intent(intent, reply)) => {
                                intents.push(intent);
                                replies.push(reply);
                            }
                            Ok(done) => deferred.push_back(done),
                            Err(_) => break,
                        }
                    }
                    let result = if first_db_err.is_some() {
                        Err(err("E007", "数据库不可写，已停止提交"))
                    } else {
                        db::save_copy_intents(conn, &intents)
                    };
                    if let Err(error) = &result {
                        first_db_err = Some(error.clone());
                    }
                    for reply in replies {
                        let _ = reply.send(result.clone());
                    }
                    continue;
                }
                CopyEvent::Done(index, result) => (index, result),
            };
            match res {
                Ok(CopyOutcome::Copied(bytes, hash)) => {
                    let path = job.items[item_index].target_path.clone();
                    job.items[item_index].temp_path = None;
                    job.items[item_index].bytes = bytes;
                    job.items[item_index].status = "succeeded".into();
                    job.items[item_index].sha256 = Some(hash.clone());
                    job.items[item_index].message = success_msg.into();
                    job.items[item_index].code.clear();
                    if seen_file_paths.insert(path.clone()) {
                        job.files.push(JobFile {
                            path,
                            size: bytes,
                            sha256: hash,
                            created_by_job: true,
                            identity: None,
                        });
                    }
                    dirty_indices.push(item_index);
                }
                Ok(CopyOutcome::Cancelled) => {
                    any_cancelled = true;
                    job.items[item_index].status = "not_started".into();
                    job.items[item_index].temp_path = None;
                    job.items[item_index].code.clear();
                    job.items[item_index].message.clear();
                    dirty_indices.push(item_index);
                }
                Err(e) => {
                    job.items[item_index].temp_path = None;
                    job.items[item_index].status = "failed".into();
                    job.items[item_index].code = e.code;
                    job.items[item_index].message = e.message;
                    dirty_indices.push(item_index);
                    if first_db_err.is_none() {
                        if let Err(db_e) = db::checkpoint_job(conn, job, &dirty_indices, true) {
                            first_db_err = Some(db_e);
                        } else {
                            dirty_indices.clear();
                            last_db_flush = Instant::now();
                        }
                    }
                }
            }
            items_since_progress += 1;
            if first_db_err.is_none() && should_flush_db(&dirty_indices, last_db_flush) {
                if let Err(db_e) = db::checkpoint_job(conn, job, &dirty_indices, false) {
                    first_db_err = Some(db_e);
                } else {
                    dirty_indices.clear();
                    last_db_flush = Instant::now();
                }
            }
            if items_since_progress >= PROGRESS_EMIT_INTERVAL_ITEMS
                || last_progress.elapsed() >= PROGRESS_EMIT_INTERVAL
            {
                emit_progress(job, item_index, progress);
                items_since_progress = 0;
                last_progress = Instant::now();
            }
        }
    });

    if let Ok((created, _)) = dirs_lock.into_inner() {
        job.created_dirs = created;
    }
    if let Some(db_e) = first_db_err {
        return Err(db_e);
    }
    let cancelled = any_cancelled || cancel.is_some_and(|flag| flag.load(Ordering::Relaxed));
    job.status = if cancelled {
        stopped_status(job)
    } else {
        final_status(job)
    };
    db::checkpoint_job(conn, job, &dirty_indices, true)?;
    emit_progress(job, job.items.len().saturating_sub(1), progress);
    Ok(())
}

fn should_flush_db(dirty_indices: &[usize], last_flush: Instant) -> bool {
    !dirty_indices.is_empty()
        && (dirty_indices.len() >= DB_CHECKPOINT_INTERVAL_ITEMS
            || last_flush.elapsed() >= DB_CHECKPOINT_INTERVAL)
}

fn should_emit_progress(index: usize, total: usize, since: usize, last_emit: Instant) -> bool {
    index == 0
        || index + 1 == total
        || since >= PROGRESS_EMIT_INTERVAL_ITEMS
        || last_emit.elapsed() >= PROGRESS_EMIT_INTERVAL
}

fn cancel_requested(opts: &ExecuteOptions) -> bool {
    opts.cancel
        .as_ref()
        .is_some_and(|flag| flag.load(Ordering::Relaxed))
}

fn stopped_status(job: &JobManifest) -> String {
    let status = final_status(job);
    if status == "success" || job.items.iter().any(|item| item.status == "succeeded") {
        "partial".into()
    } else if job
        .items
        .iter()
        .any(|item| matches!(item.status.as_str(), "not_started" | "in_progress"))
    {
        "interrupted".into()
    } else {
        status
    }
}

fn emit_progress(job: &JobManifest, index: usize, progress: &mut dyn FnMut(&CopyTick)) {
    let mut succeeded = 0u32;
    let mut failed = 0u32;
    let mut skipped = 0u32;
    let mut remaining = 0u32;
    for item in &job.items {
        match item.status.as_str() {
            "succeeded" => succeeded += 1,
            "failed" => failed += 1,
            "skipped" => skipped += 1,
            "not_started" | "in_progress" => remaining += 1,
            _ => {}
        }
    }
    let file_name = job
        .items
        .get(index)
        .map(|item| {
            if item.source_name.is_empty() {
                item.source_path.clone()
            } else {
                item.source_name.clone()
            }
        })
        .unwrap_or_default();
    progress(&CopyTick {
        file_name,
        succeeded,
        failed,
        skipped,
        remaining,
    });
}

pub fn job_csv(job: &JobManifest) -> String {
    let mut out = String::from("\u{feff}");
    out.push_str(
        "作业号,状态,原路径,原文件名,拍摄时间,时间来源,分类,目标名称,目标路径,字节数,错误码\n",
    );
    for item in &job.items {
        let bytes = item.bytes.to_string();
        let fields = [
            job.job_id.as_str(),
            item.status.as_str(),
            item.source_path.as_str(),
            item.source_name.as_str(),
            item.capture_text.as_str(),
            item.time_source.as_str(),
            item.group_name.as_str(),
            item.target_name.as_str(),
            item.target_path.as_str(),
            bytes.as_str(),
            item.code.as_str(),
        ];
        for (index, field) in fields.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            out.push_str(&csv_cell(field));
        }
        out.push('\n');
    }
    out
}

pub fn plan_csv(plan: &ExecutionPlan, confirmed_skips: &[String]) -> String {
    let skip_set: BTreeSet<&str> = confirmed_skips.iter().map(|s| s.as_str()).collect();
    let mut out = String::from("\u{feff}");
    out.push_str("分类,原图片名,目标名称,归档相对路径,目标完整路径,计划动作\n");
    for item in &plan.items {
        let action = if skip_set.contains(item.photo_id.as_str()) {
            "跳过"
        } else {
            "复制归档"
        };
        let fields = [
            item.group_name.as_str(),
            item.source_name.as_str(),
            item.target_name.as_str(),
            item.relative_path.as_str(),
            item.target_path.as_str(),
            action,
        ];
        for (index, field) in fields.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            out.push_str(&csv_cell(field));
        }
        out.push('\n');
    }
    out
}

pub fn job_json(job: &JobManifest) -> Result<String> {
    let rows: Vec<serde_json::Value> = job
        .items
        .iter()
        .map(|item| {
            serde_json::json!({
                "作业号": job.job_id,
                "状态": item.status,
                "原路径": item.source_path,
                "原文件名": item.source_name,
                "拍摄时间": item.capture_text,
                "时间来源": item.time_source,
                "分类": item.group_name,
                "目标名称": item.target_name,
                "目标路径": item.target_path,
                "字节数": item.bytes,
                "错误码": item.code,
            })
        })
        .collect();
    serde_json::to_string_pretty(&rows).map_err(|e| err("", e.to_string()))
}

fn csv_cell(value: &str) -> String {
    let guarded = if value.starts_with(['=', '+', '-', '@', '\t']) {
        format!("'{value}")
    } else {
        value.to_string()
    };
    if guarded.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", guarded.replace('"', "\"\""))
    } else {
        guarded
    }
}

fn copy_item(
    scratch: &mut CopyScratch,
    created_dirs: &mut Vec<String>,
    item: &mut JobItem,
    strict: bool,
    job_item: (&str, usize),
    cancel: Option<&AtomicBool>,
    conn: &Connection,
) -> Result<CopyOutcome> {
    let (job_id, index) = job_item;
    let temp = temp_path(&item.target_path, job_id, index);
    item.temp_path = Some(temp);
    let res = copy_item_parts(
        &mut scratch.buf,
        |parent| ensure_dir_cached(parent, created_dirs, &mut scratch.verified_dirs),
        CopySpec {
            source_path: &item.source_path,
            target_path: &item.target_path,
            expected_size: item.expected_size,
            expected_mtime_ns: item.expected_mtime_ns,
            strict,
            job_id,
            index,
            cancel,
        },
        |intent| db::save_copy_intent(conn, intent),
    );
    item.temp_path = None;
    if let Ok(CopyOutcome::Copied(bytes, _)) = &res {
        item.bytes = *bytes;
    }
    res
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct CopyIntent {
    pub job_id: String,
    pub index: usize,
    pub source: String,
    pub target: String,
    pub temp: String,
    pub identity: String,
    pub bytes: u64,
    pub hash: Option<String>,
}

struct CopySpec<'a> {
    source_path: &'a str,
    target_path: &'a str,
    expected_size: u64,
    expected_mtime_ns: i64,
    strict: bool,
    job_id: &'a str,
    index: usize,
    cancel: Option<&'a AtomicBool>,
}

fn copy_item_parts(
    buf: &mut [u8],
    mut ensure_parent: impl FnMut(&Path) -> Result<()>,
    spec: CopySpec<'_>,
    mut persist: impl FnMut(&CopyIntent) -> Result<()>,
) -> Result<CopyOutcome> {
    let source = Path::new(spec.source_path);
    let target = Path::new(spec.target_path);
    let mut input = util::open_stable_source(source).map_err(|e| {
        err(
            "E003",
            format!("无法稳定读取原图片，请停止同步或关闭占用程序后重试：{e}"),
        )
    })?;
    let meta = input.metadata().map_err(|e| err("E003", e.to_string()))?;
    if meta.len() != spec.expected_size || util::mtime_ns(&meta) != spec.expected_mtime_ns {
        return Err(err("E003", "原图片在计划之后发生变化，请重新扫描并核对"));
    }
    let source_id = util::file_identity(&input).map_err(|e| err("E003", e.to_string()))?;
    if path_exists(target) {
        return Err(err("E002", "目标已存在，已拒绝覆盖"));
    }
    ensure_parent(target.parent().ok_or_else(|| err("E005", "目标路径无效"))?)?;
    let temp = PathBuf::from(temp_path(spec.target_path, spec.job_id, spec.index));
    fault_point("before_temp")?;
    let mut output = util::create_owned_temp(&temp)
        .map_err(|e| err("E007", format!("无法创建受保护的临时副本：{e}")))?;
    let mut intent = CopyIntent {
        job_id: spec.job_id.into(),
        index: spec.index,
        source: spec.source_path.into(),
        target: spec.target_path.into(),
        temp: temp.display().to_string(),
        identity: util::file_identity(&output).map_err(|e| err("E007", e.to_string()))?,
        bytes: spec.expected_size,
        hash: None,
    };
    let copied = (|| {
        persist(&intent)?;
        let Some((bytes, hash)) = write_temp(&mut input, &mut output, buf, spec.cancel)? else {
            return Ok(CopyOutcome::Cancelled);
        };
        let after = input.metadata().map_err(|e| err("E003", e.to_string()))?;
        if bytes != spec.expected_size
            || after.len() != meta.len()
            || util::mtime_ns(&after) != util::mtime_ns(&meta)
            || util::file_identity(&input).map_err(|e| err("E003", e.to_string()))? != source_id
        {
            return Err(err("E003", "原图片在复制过程中发生变化，请停止同步后重试"));
        }
        output
            .sync_all()
            .map_err(|e| err("E007", format!("无法把副本写入磁盘：{e}")))?;
        if spec.strict
            && sha256_file_with_buf(&temp, buf).map_err(|e| err("E007", e.to_string()))? != hash
        {
            return Err(err("E007", "严格校验失败，副本回读哈希不一致"));
        }
        fault_point("after_verify")?;
        intent.hash = Some(hash.clone());
        persist(&intent)?;
        fault_point("before_commit")?;
        Ok(CopyOutcome::Copied(bytes, hash))
    })();
    match copied {
        Ok(CopyOutcome::Copied(bytes, hash)) => {
            if let Err(e) = util::commit_owned_handle(&output, target) {
                let _ = util::delete_owned_handle(&output);
                return Err(err(
                    if path_exists(target) { "E002" } else { "E007" },
                    format!("无法提交副本（未覆盖）：{e}"),
                ));
            }
            fault_point("after_commit")?;
            if util::file_identity(&output).map_err(|e| err("E007", e.to_string()))?
                != intent.identity
            {
                return Err(err("E007", "提交后文件身份不一致，已停止"));
            }
            Ok(CopyOutcome::Copied(bytes, hash))
        }
        other => {
            let _ = util::delete_owned_handle(&output);
            other
        }
    }
}

fn sha256_file_with_buf(path: &Path, buf: &mut [u8]) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    let mut file = util::open_any(path)?;
    let mut hasher = Sha256::new();
    loop {
        let n = file.read(buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn write_temp(
    input: &mut File,
    output: &mut File,
    buf: &mut [u8],
    cancel: Option<&AtomicBool>,
) -> Result<Option<(u64, String)>> {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    loop {
        if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            return Ok(None);
        }
        let n = input.read(buf).map_err(|e| err("E007", e.to_string()))?;
        if n == 0 {
            break;
        }
        output
            .write_all(&buf[..n])
            .map_err(|e| err("E007", e.to_string()))?;
        hasher.update(&buf[..n]);
        total += n as u64;
        fault_point("mid_write")?;
        #[cfg(feature = "fault-injection")]
        if std::env::var("TUXU_TEST_PAUSE_BYTES")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            == Some(total)
        {
            let marker = std::env::var("TUXU_TEST_MARKER").expect("marker");
            std::fs::write(&marker, b"source locked").expect("marker write");
            while !Path::new(&format!("{marker}.resume")).exists() {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
    Ok(Some((total, hex::encode(hasher.finalize()))))
}

pub(crate) fn fault_point(_stage: &str) -> Result<()> {
    #[cfg(feature = "fault-injection")]
    if std::env::var("TUXU_TEST_FAULT").as_deref() == Ok(_stage) {
        if std::env::var("TUXU_TEST_FAULT_MODE").as_deref() == Ok("io") {
            return Err(err("E007", format!("测试 I/O 故障：{_stage}")));
        }
        if matches!(
            std::env::var("TUXU_TEST_FAULT_MODE").as_deref(),
            Ok("kill" | "pause")
        ) {
            std::fs::write(
                std::env::var_os("TUXU_TEST_MARKER").expect("test marker"),
                _stage,
            )
            .expect("marker");
            loop {
                let marker = std::env::var("TUXU_TEST_MARKER").expect("marker");
                if std::env::var("TUXU_TEST_FAULT_MODE").as_deref() == Ok("pause")
                    && Path::new(&format!("{marker}.resume")).exists()
                {
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        std::process::exit(86);
    }
    Ok(())
}

fn ensure_dir_cached(
    dir: &Path,
    created: &mut Vec<String>,
    verified_dirs: &mut HashSet<PathBuf>,
) -> Result<()> {
    if verified_dirs.contains(dir) {
        return Ok(());
    }
    ensure_dir(dir, created)?;
    verified_dirs.insert(dir.to_path_buf());
    Ok(())
}

fn ensure_dir(dir: &Path, created: &mut Vec<String>) -> Result<()> {
    if path_exists(dir) {
        if util::is_dir_any(dir) {
            return Ok(());
        }
        return Err(err("E004", format!("{} 已存在且不是目录", dir.display())));
    }
    let mut missing = Vec::new();
    let mut cur = dir.to_path_buf();
    while !path_exists(&cur) {
        missing.push(cur.clone());
        if !cur.pop() {
            break;
        }
    }
    for path in missing.into_iter().rev() {
        util::create_dir_any(&path)
            .map_err(|e| err("E004", format!("无法创建目录 {}：{e}", path.display())))?;
        let text = path.display().to_string();
        if !created.iter().any(|c| c.eq_ignore_ascii_case(&text)) {
            created.push(text);
        }
    }
    Ok(())
}

fn temp_path(target: &str, job_id: &str, index: usize) -> String {
    let path = PathBuf::from(target);
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    parent
        .join(format!(".{job_id}.{index}.tuxu-part"))
        .display()
        .to_string()
}

fn final_status(job: &JobManifest) -> String {
    let succeeded = job.items.iter().filter(|i| i.status == "succeeded").count();
    let failed = job.items.iter().filter(|i| i.status == "failed").count();
    let skipped = job.items.iter().filter(|i| i.status == "skipped").count();
    if !job.items.is_empty() && succeeded == job.items.len() {
        "success".into()
    } else if succeeded > 0 {
        "partial".into()
    } else if failed > 0 {
        "failed".into()
    } else if skipped > 0 {
        "skipped".into()
    } else {
        "failed".into()
    }
}

fn report_from(job: &JobManifest, warnings: &[Issue]) -> ExecuteReport {
    let succeeded = job.items.iter().filter(|i| i.status == "succeeded").count() as u32;
    let failed = job.items.iter().filter(|i| i.status == "failed").count() as u32;
    let skipped = job.items.iter().filter(|i| i.status == "skipped").count() as u32;
    let not_started = job
        .items
        .iter()
        .filter(|i| i.status == "not_started" || i.status == "in_progress")
        .count() as u32;
    let status = if job.status == "interrupted" {
        "interrupted".to_string()
    } else if succeeded > 0 && (failed > 0 || skipped > 0 || not_started > 0) {
        "partial".to_string()
    } else if failed == 0
        && skipped == 0
        && not_started == 0
        && succeeded == job.items.len() as u32
        && succeeded > 0
    {
        "success".to_string()
    } else if succeeded == 0 && failed > 0 {
        "failed".to_string()
    } else if succeeded == 0 && failed == 0 && skipped > 0 && not_started == 0 {
        "skipped".to_string()
    } else {
        job.status.clone()
    };
    ExecuteReport {
        job_id: job.job_id.clone(),
        status,
        succeeded,
        failed,
        skipped,
        not_started,
        output_root: job.output_root.clone(),
        items: job
            .items
            .iter()
            .map(|i| ItemResult {
                photo_id: i.photo_id.clone(),
                source_path: i.source_path.clone(),
                target_path: i.target_path.clone(),
                status: i.status.clone(),
                message: i.message.clone(),
                code: i.code.clone(),
                bytes: i.bytes,
            })
            .collect(),
        issues: warnings
            .iter()
            .filter(|i| !i.blocks)
            .cloned()
            .chain(
                job.items
                    .iter()
                    .filter(|i| i.status == "failed")
                    .map(|i| Issue {
                        code: i.code.clone(),
                        severity: "error".into(),
                        blocks: true,
                        message: i.message.clone(),
                        photo_id: Some(i.photo_id.clone()),
                    }),
            )
            .collect(),
    }
}

pub(crate) fn reconcile_intents(conn: &Connection, job: &mut JobManifest) -> Result<()> {
    if job.status == "undone" {
        return Ok(());
    }
    for item in &mut job.items {
        if item.status == "undo_pending" && !path_exists(Path::new(&item.target_path)) {
            item.status = "undone".into();
            item.message = "已恢复中断的撤销记录".into();
        }
    }
    job.files.retain(|f| {
        !job.items
            .iter()
            .any(|i| i.target_path == f.path && i.status == "undone")
    });
    for intent in db::copy_intents(conn, &job.job_id)? {
        let Some(item) = job.items.get_mut(intent.index) else {
            continue;
        };
        if intent.job_id != job.job_id
            || intent.source != item.source_path
            || intent.target != item.target_path
            || intent.temp != temp_path(&item.target_path, &job.job_id, intent.index)
        {
            return Err(err("E007", "恢复凭据与作业不一致，拒绝自动处理"));
        }
        if item.status == "undone" {
            continue;
        }
        if path_exists(Path::new(&intent.target)) {
            if intent.hash.as_ref().is_some_and(|h| {
                owned_fingerprint_ok(&intent.target, intent.bytes, h, Some(&intent.identity))
            }) {
                if item.status != "undo_pending" {
                    item.status = "succeeded".into();
                }
                item.bytes = intent.bytes;
                item.sha256 = intent.hash.clone();
                item.code.clear();
                item.message = "已核对持久化提交凭据".into();
                item.temp_path = None;
            } else {
                item.status = "failed".into();
                item.code = "E002".into();
                item.message = "目标归属或内容不符，需人工核对；不会覆盖或自动认领".into();
            }
        } else if item.status != "succeeded" && path_exists(Path::new(&intent.temp)) {
            if identity_matches(&intent.temp, &intent.identity) {
                item.temp_path = Some(intent.temp.clone());
                item.status = "in_progress".into();
            } else {
                item.temp_path = None;
                item.status = "failed".into();
                item.code = "E007".into();
                item.message = "残留临时文件归属不明，保留现场，请导出诊断".into();
            }
        }
        if let Some(file) = job.files.iter_mut().find(|f| f.path == intent.target) {
            file.identity = Some(intent.identity.clone());
        } else if matches!(item.status.as_str(), "succeeded" | "undo_pending") {
            job.files.push(JobFile {
                path: intent.target,
                size: intent.bytes,
                sha256: intent.hash.unwrap_or_default(),
                created_by_job: true,
                identity: Some(intent.identity),
            });
        }
    }
    Ok(())
}

fn identity_matches(path: &str, expected: &str) -> bool {
    util::open_stable_source(Path::new(path))
        .and_then(|f| util::file_identity(&f))
        .is_ok_and(|id| id == expected)
}

pub fn recovery_view(job: &JobManifest) -> RecoveryView {
    let mut temps = BTreeSet::new();
    for item in &job.items {
        if let Some(temp) = &item.temp_path {
            if path_exists(Path::new(temp)) {
                temps.insert(temp.clone());
            }
        }
        if !item.target_path.is_empty() {
            if let Some(parent) = Path::new(&item.target_path).parent() {
                for path in crate::scan::list_part_files(parent) {
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    if name.contains(&format!(".{}", job.job_id)) {
                        temps.insert(path.display().to_string());
                    }
                }
            }
        }
    }
    RecoveryView {
        job_id: job.job_id.clone(),
        status: job.status.clone(),
        succeeded: job.items.iter().filter(|i| i.status == "succeeded").count() as u32,
        failed: job.items.iter().filter(|i| i.status == "failed").count() as u32,
        not_started: job
            .items
            .iter()
            .filter(|i| i.status == "not_started")
            .count() as u32,
        in_progress: job
            .items
            .iter()
            .filter(|i| i.status == "in_progress")
            .count() as u32,
        leftover_temps: temps.into_iter().collect(),
    }
}

#[allow(dead_code)]
pub fn retry(conn: &Connection, job: &mut JobManifest) -> Result<ExecuteReport> {
    retry_with(conn, job, None, &mut |_| {})
}

pub fn retry_with(
    conn: &Connection,
    job: &mut JobManifest,
    cancel: Option<&AtomicBool>,
    progress: &mut dyn FnMut(&CopyTick),
) -> Result<ExecuteReport> {
    reconcile_intents(conn, job)?;
    if job.status == "undoing"
        || job.status == "undone"
        || job
            .items
            .iter()
            .any(|i| matches!(i.status.as_str(), "undo_pending" | "undone"))
    {
        return Err(err(
            "E007",
            "本作业已经开始撤销，请继续撤销；如需重新归档请返回任务生成新计划",
        ));
    }
    let total = job.items.len();
    let mut scratch = CopyScratch::new();
    let mut dirty_indices: Vec<usize> = Vec::new();
    let mut seen_file_paths: HashSet<String> = job.files.iter().map(|f| f.path.clone()).collect();
    let mut last_db_flush = Instant::now();
    let mut last_progress = Instant::now();
    let mut items_since_progress = 0usize;
    emit_progress(job, 0, progress);

    for index in 0..total {
        if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            job.status = stopped_status(job);
            db::checkpoint_job(conn, job, &dirty_indices, true)?;
            emit_progress(job, index, progress);
            return Ok(report_from(job, &[]));
        }
        if job.items[index].status == "skipped" {
            items_since_progress += 1;
            if should_emit_progress(index, total, items_since_progress, last_progress) {
                emit_progress(job, index, progress);
                items_since_progress = 0;
                last_progress = Instant::now();
            }
            continue;
        }
        if job.items[index].status == "succeeded" {
            let path = job.items[index].target_path.clone();
            let bytes = job.items[index].bytes;
            let hash = job.items[index].sha256.clone();
            if hash.as_ref().is_some_and(|hash| {
                owned_fingerprint_ok(
                    &path,
                    bytes,
                    hash,
                    job.files
                        .iter()
                        .find(|f| f.path == path)
                        .and_then(|f| f.identity.as_ref()),
                )
            }) {
                items_since_progress += 1;
                if should_emit_progress(index, total, items_since_progress, last_progress) {
                    emit_progress(job, index, progress);
                    items_since_progress = 0;
                    last_progress = Instant::now();
                }
                continue;
            }
            job.items[index].status = "failed".into();
            job.items[index].code = "E002".into();
            job.items[index].message = "已成功的目标后来被修改，重试不会覆盖".into();
            dirty_indices.push(index);
            items_since_progress += 1;
            if should_flush_db(&dirty_indices, last_db_flush) {
                db::checkpoint_job(conn, job, &dirty_indices, false)?;
                dirty_indices.clear();
                last_db_flush = Instant::now();
            }
            if should_emit_progress(index, total, items_since_progress, last_progress) {
                emit_progress(job, index, progress);
                items_since_progress = 0;
                last_progress = Instant::now();
            }
            continue;
        }
        let target = job.items[index].target_path.clone();
        if path_exists(Path::new(&target)) {
            job.items[index].status = "failed".into();
            job.items[index].code = "E002".into();
            job.items[index].message = "目标已存在且未通过归属核对，重试没有覆盖".into();
            dirty_indices.push(index);
            items_since_progress += 1;
            if should_flush_db(&dirty_indices, last_db_flush) {
                db::checkpoint_job(conn, job, &dirty_indices, false)?;
                dirty_indices.clear();
                last_db_flush = Instant::now();
            }
            if should_emit_progress(index, total, items_since_progress, last_progress) {
                emit_progress(job, index, progress);
                items_since_progress = 0;
                last_progress = Instant::now();
            }
            continue;
        }
        if let Some(temp) = job.items[index].temp_path.clone() {
            if path_exists(Path::new(&temp)) {
                remove_owned_temp(conn, &job.job_id, index, &temp)?;
            }
            job.items[index].temp_path = None;
        }
        job.items[index].status = "not_started".into();
        let strict = job.strict_hash;
        let job_id = job.job_id.clone();
        let mut created = std::mem::take(&mut job.created_dirs);
        let copied = copy_item(
            &mut scratch,
            &mut created,
            &mut job.items[index],
            strict,
            (&job_id, index),
            cancel,
            conn,
        );
        job.created_dirs = created;
        match copied {
            Ok(CopyOutcome::Copied(bytes, hash)) => {
                let path = job.items[index].target_path.clone();
                job.items[index].status = "succeeded".into();
                job.items[index].bytes = bytes;
                job.items[index].sha256 = Some(hash.clone());
                job.items[index].message = "重试已复制".into();
                job.items[index].code.clear();
                if seen_file_paths.insert(path.clone()) {
                    job.files.push(JobFile {
                        path,
                        size: bytes,
                        sha256: hash,
                        created_by_job: true,
                        identity: None,
                    });
                }
                dirty_indices.push(index);
            }
            Ok(CopyOutcome::Cancelled) => {
                job.items[index].status = "not_started".into();
                job.items[index].temp_path = None;
                job.items[index].code.clear();
                job.items[index].message.clear();
                dirty_indices.push(index);
                job.status = stopped_status(job);
                db::checkpoint_job(conn, job, &dirty_indices, true)?;
                emit_progress(job, index, progress);
                return Ok(report_from(job, &[]));
            }
            Err(e) => {
                job.items[index].status = "failed".into();
                job.items[index].code = e.code;
                job.items[index].message = e.message;
                dirty_indices.push(index);
                db::checkpoint_job(conn, job, &dirty_indices, true)?;
                dirty_indices.clear();
                last_db_flush = Instant::now();
            }
        }
        items_since_progress += 1;
        if should_flush_db(&dirty_indices, last_db_flush) {
            db::checkpoint_job(conn, job, &dirty_indices, false)?;
            dirty_indices.clear();
            last_db_flush = Instant::now();
        }
        if should_emit_progress(index, total, items_since_progress, last_progress) {
            emit_progress(job, index, progress);
            items_since_progress = 0;
            last_progress = Instant::now();
        }
    }
    job.status = final_status(job);
    db::checkpoint_job(conn, job, &dirty_indices, true)?;
    emit_progress(job, total.saturating_sub(1), progress);
    Ok(report_from(job, &[]))
}

pub fn undo_preview(job: &JobManifest) -> UndoPreview {
    let mut will_delete = Vec::new();
    let mut will_refuse = Vec::new();
    let mut reasons = Vec::new();
    for file in &job.files {
        if !file.created_by_job {
            continue;
        }
        match classify_undo(file) {
            UndoClass::Delete => will_delete.push(file.path.clone()),
            UndoClass::Refuse(reason) => {
                reasons.push(format!("{}：{reason}", file.path));
                will_refuse.push(file.path.clone());
            }
        }
    }
    UndoPreview {
        job_id: job.job_id.clone(),
        will_delete,
        will_refuse,
        refuse_reasons: reasons,
    }
}

enum UndoClass {
    Delete,
    Refuse(String),
}

fn verified_delete_handle(file: &JobFile) -> Result<File> {
    let identity = file
        .identity
        .as_ref()
        .ok_or_else(|| err("E007", "旧记录缺少文件归属凭据，保留文件；请人工核对"))?;
    let mut handle = util::open_owned_for_delete(Path::new(&file.path))
        .map_err(|e| err("E007", format!("无法锁定副本，保留文件：{e}")))?;
    if util::file_identity(&handle).map_err(|e| err("E007", e.to_string()))? != *identity {
        return Err(err("E007", "文件已被替换，拒绝删除外来文件"));
    }
    if handle
        .metadata()
        .map_err(|e| err("E007", e.to_string()))?
        .len()
        != file.size
        || hash_handle(&mut handle).map_err(|e| err("E007", e.to_string()))? != file.sha256
    {
        return Err(err("E007", "文件内容已变化，拒绝删除"));
    }
    Ok(handle)
}

fn hash_handle(file: &mut File) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    file.seek(SeekFrom::Start(0))?;
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(hex::encode(hash.finalize()))
}

fn classify_undo(file: &JobFile) -> UndoClass {
    match verified_delete_handle(file) {
        Ok(_) => UndoClass::Delete,
        Err(e) => UndoClass::Refuse(e.message),
    }
}

fn remove_owned_temp(conn: &Connection, job_id: &str, index: usize, temp: &str) -> Result<()> {
    let intent = db::copy_intents(conn, job_id)?
        .into_iter()
        .find(|i| i.index == index && i.temp == temp)
        .ok_or_else(|| err("E007", "临时文件缺归属凭据，保留现场"))?;
    let file =
        util::open_owned_for_delete(Path::new(temp)).map_err(|e| err("E007", e.to_string()))?;
    if util::file_identity(&file).map_err(|e| err("E007", e.to_string()))? != intent.identity {
        return Err(err("E007", "临时文件已被替换，保留现场"));
    }
    util::delete_owned_handle(&file).map_err(|e| err("E007", e.to_string()))
}

fn owned_fingerprint_ok(path: &str, size: u64, sha: &str, identity: Option<&String>) -> bool {
    let Some(identity) = identity else {
        return false;
    };
    let Ok(mut handle) = util::open_stable_source(Path::new(path)) else {
        return false;
    };
    handle.metadata().is_ok_and(|m| m.len() == size)
        && util::file_identity(&handle).is_ok_and(|id| id == *identity)
        && hash_handle(&mut handle).is_ok_and(|hash| hash.eq_ignore_ascii_case(sha))
}

pub fn undo_job(conn: &Connection, job: &mut JobManifest) -> Result<UndoReport> {
    if job.status == "undone" {
        return Ok(UndoReport {
            job_id: job.job_id.clone(),
            deleted: Vec::new(),
            refused: Vec::new(),
            refuse_reasons: Vec::new(),
            removed_dirs: Vec::new(),
        });
    }
    let preview = undo_preview(job);
    let mut deleted = Vec::new();
    let mut refused = preview.will_refuse.clone();
    let mut reasons = preview.refuse_reasons.clone();
    let mut delete_failed = false;
    job.status = "undoing".into();
    for path in &preview.will_delete {
        let Some(index) = job.items.iter().position(|i| &i.target_path == path) else {
            continue;
        };
        let file = job
            .files
            .iter()
            .find(|f| &f.path == path)
            .expect("preview file");
        let deletion = verified_delete_handle(file).and_then(|handle| {
            job.items[index].status = "undo_pending".into();
            db::checkpoint_job(conn, job, &[index], false)?;
            fault_point("undo_before_delete")?;
            fault_point("undo_verified")?;
            util::delete_owned_handle(&handle).map_err(|e| err("E007", e.to_string()))?;
            drop(handle);
            fault_point("undo_after_delete")?;
            Ok(())
        });
        match deletion {
            Ok(()) => {
                deleted.push(path.clone());
                job.items[index].status = "undone".into();
                fault_point("undo_checkpoint")?;
                db::checkpoint_job(conn, job, &[index], false)?;
            }
            Err(e) => {
                delete_failed = true;
                refused.push(path.clone());
                reasons.push(format!("{path}：{}", e.message));
            }
        }
    }
    for (index, item) in job.items.iter_mut().enumerate() {
        if let Some(temp) = item.temp_path.clone() {
            if path_exists(Path::new(&temp)) {
                match remove_owned_temp(conn, &job.job_id, index, &temp) {
                    Ok(()) => item.temp_path = None,
                    Err(e) => {
                        delete_failed = true;
                        refused.push(temp.clone());
                        reasons.push(format!("{temp}：{}", e.message));
                    }
                }
            } else {
                item.temp_path = None;
            }
        }
    }
    let mut removed_dirs = Vec::new();
    let mut dirs = job.created_dirs.clone();
    dirs.sort_by_key(|d| std::cmp::Reverse(d.len()));
    for dir in dirs {
        let path = PathBuf::from(&dir);
        if path == Path::new(&job.output_root) {
            continue;
        }
        if !util::is_dir_any(&path) {
            continue;
        }
        let empty = util::read_dir_any(&path)
            .map(|mut rd| rd.next().is_none())
            .unwrap_or(false);
        if empty && util::remove_dir_any(&path).is_ok() {
            removed_dirs.push(dir);
        }
    }
    job.files.retain(|file| !deleted.contains(&file.path));
    job.status = if delete_failed || !refused.is_empty() {
        "partial".into()
    } else {
        "undone".into()
    };
    db::save_job(conn, job)?;
    Ok(UndoReport {
        job_id: job.job_id.clone(),
        deleted,
        refused,
        refuse_reasons: reasons,
        removed_dirs,
    })
}
