use base64::Engine;
use serde::Serialize;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, State};
use tuxu_core::{
    render_thumbnail_standalone, run_quality_inspection_batch, DraftSummary, ExecuteOptions,
    ExecuteReport, ExecutionPlan, GroupPreset, ImportReport, JobManifest, NamingRule, Organizer,
    QualityReport, RecoveryView, ScanControl, ScanOptions, SelfCheckReport, TagItem, TaskDetail,
    UndoPreview, UndoReport,
};

struct AppState {
    org: Arc<Mutex<Organizer>>,
    initial_check: SelfCheckReport,
    scan_cancel: Arc<AtomicBool>,
    copy_cancel: Arc<AtomicBool>,
    quality_cancel: Arc<AtomicBool>,
    job_running: Arc<AtomicBool>,
}

struct RunningGuard(Arc<AtomicBool>);

impl Drop for RunningGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

fn acquire_job_guard(flag: &Arc<AtomicBool>) -> Result<RunningGuard, String> {
    if flag
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err("已有归档或恢复任务正在执行，请等待完成或先取消当前任务".into());
    }
    Ok(RunningGuard(Arc::clone(flag)))
}

struct InstanceGuard(*mut c_void);

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

#[derive(Serialize)]
struct Boot {
    check: SelfCheckReport,
    drafts: Vec<DraftSummary>,
    recovery: Vec<RecoveryView>,
    tags: Vec<TagItem>,
    presets: Vec<GroupPreset>,
}

#[derive(Serialize)]
struct ThumbDto {
    available: bool,
    mime: String,
    data_base64: String,
    reason: String,
}

extern "system" {
    fn CreateMutexW(attrs: *mut c_void, initial: i32, name: *const u16) -> *mut c_void;
    fn GetLastError() -> u32;
    fn CloseHandle(handle: *mut c_void) -> i32;
    fn MessageBoxW(hwnd: *mut c_void, text: *const u16, caption: *const u16, kind: u32) -> i32;
}

fn data_dir() -> PathBuf {
    if let Some(value) = std::env::var_os("TUXU_DATA_DIR") {
        return PathBuf::from(value);
    }
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("TuXuArchive").join("data")
}

fn single_instance() -> Option<InstanceGuard> {
    let mut name: Vec<u16> = "Local\\TuXuArchive\0".encode_utf16().collect();
    unsafe {
        let handle = CreateMutexW(std::ptr::null_mut(), 1, name.as_mut_ptr());
        if handle.is_null() {
            return Some(InstanceGuard(std::ptr::null_mut()));
        }
        if GetLastError() == 183 {
            CloseHandle(handle);
            return None;
        }
        Some(InstanceGuard(handle))
    }
}

fn tell(text: &str) {
    if std::env::var("TUXU_NO_UI").ok().as_deref() == Some("1") {
        eprintln!("{text}");
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
            0x00000030,
        );
    }
}

fn lock<'a>(
    state: &'a State<'a, AppState>,
) -> Result<std::sync::MutexGuard<'a, Organizer>, String> {
    state.org.lock().map_err(|e| e.to_string())
}

async fn run_task<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| format!("后台任务异常中止：{e}"))?
}

async fn run_blocking<T: Send + 'static>(
    state: &State<'_, AppState>,
    f: impl FnOnce(&mut Organizer) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let org = Arc::clone(&state.org);
    run_task(move || {
        let mut guard = org.lock().map_err(|e| e.to_string())?;
        f(&mut guard)
    })
    .await
}

#[tauri::command]
async fn bootstrap(state: State<'_, AppState>) -> Result<Boot, String> {
    let check = state.initial_check.clone();
    run_blocking(&state, move |org| {
        Ok(Boot {
            check,
            drafts: org.list_drafts().map_err(|e| e.to_string())?,
            recovery: org.recovery().map_err(|e| e.to_string())?,
            tags: org.tags().map_err(|e| e.to_string())?,
            presets: org.list_presets().map_err(|e| e.to_string())?,
        })
    })
    .await
}

#[tauri::command]
async fn scan_folder(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
    recursive: bool,
    replace: bool,
) -> Result<TaskDetail, String> {
    state.scan_cancel.store(false, Ordering::Relaxed);
    let cancel = Arc::clone(&state.scan_cancel);
    run_blocking(&state, move |org| {
        org.scan_folder_controlled(
            Path::new(&path),
            ScanOptions { recursive },
            replace,
            None,
            |tick| {
                let _ = app.emit("scan-progress", tick);
                if cancel.load(Ordering::Relaxed) {
                    ScanControl::Cancel
                } else {
                    ScanControl::Continue
                }
            },
        )
        .map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
fn cancel_scan(state: State<AppState>) {
    state.scan_cancel.store(true, Ordering::Relaxed);
}

#[tauri::command]
fn cancel_copy(state: State<AppState>) {
    state.copy_cancel.store(true, Ordering::Relaxed);
}

#[tauri::command]
fn cancel_quality(state: State<AppState>) {
    state.quality_cancel.store(true, Ordering::Relaxed);
}

#[tauri::command]
async fn inspect_quality(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<QualityReport, String> {
    state.quality_cancel.store(false, Ordering::Relaxed);
    let cancel = Arc::clone(&state.quality_cancel);
    let org = Arc::clone(&state.org);
    tauri::async_runtime::spawn_blocking(move || {
        let (task_id, photos) = {
            let guard = org.lock().map_err(|e| e.to_string())?;
            guard.quality_snapshot().map_err(|e| e.to_string())?
        };
        let (updates, cancelled) = run_quality_inspection_batch(&photos, |tick| {
            let _ = app.emit("quality-progress", tick);
            if cancel.load(Ordering::Relaxed) {
                ScanControl::Cancel
            } else {
                ScanControl::Continue
            }
        });
        let mut guard = org.lock().map_err(|e| e.to_string())?;
        guard
            .apply_quality_batch(&task_id, updates)
            .map_err(|e| e.to_string())?;
        guard.quality_report(cancelled).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("后台画质检测异常中止：{e}"))?
}

#[tauri::command]
fn task_detail(state: State<AppState>) -> Result<TaskDetail, String> {
    lock(&state)?.detail().map_err(|e| e.to_string())
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
fn photos(
    state: State<AppState>,
    group_id: Option<String>,
    query: Option<String>,
    only_unconfirmed: Option<bool>,
    only_quality_issue: Option<bool>,
    only_time_anomaly: Option<bool>,
    only_excluded: Option<bool>,
    offset: usize,
    limit: usize,
) -> Result<tuxu_core::PhotoPage, String> {
    lock(&state)?
        .photos_filtered(
            group_id.as_deref(),
            query.as_deref(),
            only_unconfirmed.unwrap_or(false),
            only_quality_issue.unwrap_or(false),
            only_time_anomaly.unwrap_or(false),
            only_excluded.unwrap_or(false),
            offset,
            limit,
        )
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn thumb_window(
    state: State<AppState>,
    total: usize,
    scroll_index: usize,
    visible: usize,
    neighbor: usize,
) -> Result<tuxu_core::ThumbWindow, String> {
    Ok(lock(&state)?.thumb_window(total, scroll_index, visible, neighbor))
}

#[tauri::command]
async fn thumbnail(
    state: State<'_, AppState>,
    photo_id: String,
    max_edge: u32,
) -> Result<ThumbDto, String> {
    let (cache_root, photo) = lock(&state)?
        .thumb_input(&photo_id)
        .map_err(|e| e.to_string())?;
    run_task(move || {
        Ok(match render_thumbnail_standalone(&cache_root, &photo, max_edge) {
            tuxu_core::ResultThumb::Jpeg(bytes) => ThumbDto {
                available: true,
                mime: "image/jpeg".into(),
                data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
                reason: String::new(),
            },
            tuxu_core::ResultThumb::Unavailable { reason } => ThumbDto {
                available: false,
                mime: String::new(),
                data_base64: String::new(),
                reason,
            },
        })
    })
    .await
}

#[tauri::command]
fn set_task_name(state: State<AppState>, name: String) -> Result<TaskDetail, String> {
    lock(&state)?.set_task_name(&name).map_err(|e| e.to_string())
}

#[tauri::command]
fn set_dir_layout(
    state: State<AppState>,
    layout: String,
    custom_pattern: String,
) -> Result<TaskDetail, String> {
    lock(&state)?
        .set_dir_layout(&layout, &custom_pattern)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn set_naming_rule(state: State<AppState>, rule: NamingRule) -> Result<TaskDetail, String> {
    lock(&state)?.set_naming_rule(rule).map_err(|e| e.to_string())
}

#[tauri::command]
fn set_output_root(state: State<AppState>, path: String) -> Result<Option<TaskDetail>, String> {
    lock(&state)?
        .set_output_root_remembered(Path::new(&path))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn check_paths(
    state: State<AppState>,
    source_root: String,
    output_root: String,
) -> Result<tuxu_core::PathCheckView, String> {
    Ok(lock(&state)?.check_paths(&source_root, &output_root))
}

#[tauri::command]
fn set_strict(state: State<AppState>, strict: bool) -> Result<TaskDetail, String> {
    lock(&state)?.set_strict(strict).map_err(|e| e.to_string())
}

#[tauri::command]
fn set_groups_text(state: State<AppState>, text: String) -> Result<TaskDetail, String> {
    lock(&state)?.set_groups_text(&text).map_err(|e| e.to_string())
}

#[tauri::command]
fn add_group(state: State<AppState>, name: String) -> Result<TaskDetail, String> {
    lock(&state)?.add_group(&name).map_err(|e| e.to_string())
}

#[tauri::command]
fn rename_group(
    state: State<AppState>,
    group_id: String,
    new_name: String,
) -> Result<TaskDetail, String> {
    lock(&state)?
        .rename_group(&group_id, &new_name)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn delete_group(state: State<AppState>, group_id: String) -> Result<TaskDetail, String> {
    lock(&state)?.delete_group(&group_id).map_err(|e| e.to_string())
}

#[tauri::command]
fn reorder_group_to(
    state: State<AppState>,
    from_index: usize,
    to_index: usize,
) -> Result<TaskDetail, String> {
    lock(&state)?
        .reorder_group_to(from_index, to_index)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn sort_groups_by_name(state: State<AppState>) -> Result<TaskDetail, String> {
    lock(&state)?.sort_groups_by_name().map_err(|e| e.to_string())
}

#[tauri::command]
fn group_by_time(state: State<AppState>, granularity: String) -> Result<TaskDetail, String> {
    lock(&state)?
        .group_by_time(&granularity)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn group_by_time_gap(
    state: State<AppState>,
    gap_minutes: u32,
    prefix: String,
) -> Result<TaskDetail, String> {
    lock(&state)?
        .group_by_time_gap(gap_minutes, &prefix)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn group_by_fixed_size(
    state: State<AppState>,
    per_group: u32,
    prefix: String,
) -> Result<TaskDetail, String> {
    lock(&state)?
        .group_by_fixed_size(per_group, &prefix)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn group_evenly(state: State<AppState>) -> Result<TaskDetail, String> {
    lock(&state)?.group_evenly().map_err(|e| e.to_string())
}

#[tauri::command]
fn set_quantities(state: State<AppState>, counts: Vec<u32>) -> Result<TaskDetail, String> {
    lock(&state)?.set_quantities(&counts).map_err(|e| e.to_string())
}

#[tauri::command]
fn apply_preset(
    state: State<AppState>,
    preset_id: String,
    distribute_evenly: bool,
    apply_naming: bool,
) -> Result<TaskDetail, String> {
    lock(&state)?
        .apply_preset(&preset_id, distribute_evenly, apply_naming)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn assign_photos_to_group(
    state: State<AppState>,
    photo_ids: Vec<String>,
    group_id: String,
) -> Result<TaskDetail, String> {
    lock(&state)?
        .assign_photos_to_group(&photo_ids, &group_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn create_group_for_photos(
    state: State<AppState>,
    group_name: String,
    photo_ids: Vec<String>,
) -> Result<TaskDetail, String> {
    lock(&state)?
        .create_group_for_photos(&group_name, &photo_ids)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn move_photo(state: State<AppState>, photo_id: String, delta: i32) -> Result<TaskDetail, String> {
    lock(&state)?
        .move_photo(&photo_id, delta)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn resort_by_mtime(state: State<AppState>) -> Result<TaskDetail, String> {
    lock(&state)?.resort_by_mtime().map_err(|e| e.to_string())
}

#[tauri::command]
fn resort_by_filename(state: State<AppState>) -> Result<TaskDetail, String> {
    lock(&state)?.resort_by_filename().map_err(|e| e.to_string())
}

#[tauri::command]
fn assign_tag(
    state: State<AppState>,
    photo_id: String,
    tag_id: Option<String>,
) -> Result<TaskDetail, String> {
    lock(&state)?
        .assign_tag(&photo_id, tag_id.as_deref())
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn batch_assign_tag(
    state: State<AppState>,
    photo_ids: Vec<String>,
    tag_id: Option<String>,
) -> Result<TaskDetail, String> {
    lock(&state)?
        .batch_assign_tag(&photo_ids, tag_id.as_deref())
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn set_custom_name(
    state: State<AppState>,
    photo_id: String,
    custom_name: Option<String>,
) -> Result<TaskDetail, String> {
    lock(&state)?
        .set_custom_name(&photo_id, custom_name.as_deref())
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn confirm_photo(state: State<AppState>, photo_id: String) -> Result<TaskDetail, String> {
    lock(&state)?.confirm_photo(&photo_id).map_err(|e| e.to_string())
}

#[tauri::command]
fn batch_confirm_photos(
    state: State<AppState>,
    photo_ids: Vec<String>,
) -> Result<TaskDetail, String> {
    lock(&state)?
        .batch_confirm_photos(&photo_ids)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn confirm_all_ready(state: State<AppState>) -> Result<TaskDetail, String> {
    lock(&state)?.confirm_all_ready().map_err(|e| e.to_string())
}

#[tauri::command]
fn exclude_photo(
    state: State<AppState>,
    photo_id: String,
    excluded: bool,
) -> Result<TaskDetail, String> {
    lock(&state)?
        .exclude_photo(&photo_id, excluded)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn batch_exclude_photos(
    state: State<AppState>,
    photo_ids: Vec<String>,
    excluded: bool,
) -> Result<TaskDetail, String> {
    lock(&state)?
        .batch_exclude_photos(&photo_ids, excluded)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn acknowledge_review(state: State<AppState>) -> Result<TaskDetail, String> {
    lock(&state)?.acknowledge_review().map_err(|e| e.to_string())
}

#[tauri::command]
fn review_risk(state: State<AppState>) -> Result<tuxu_core::ReviewRisk, String> {
    lock(&state)?.review_risk().map_err(|e| e.to_string())
}

#[tauri::command]
fn undo_edit(state: State<AppState>) -> Result<TaskDetail, String> {
    lock(&state)?.undo_edit().map_err(|e| e.to_string())
}

#[tauri::command]
fn redo_edit(state: State<AppState>) -> Result<TaskDetail, String> {
    lock(&state)?.redo_edit().map_err(|e| e.to_string())
}

#[tauri::command]
async fn build_plan(state: State<'_, AppState>) -> Result<ExecutionPlan, String> {
    run_blocking(&state, |org| org.build_plan().map_err(|e| e.to_string())).await
}

#[tauri::command]
async fn execute_plan(
    app: AppHandle,
    state: State<'_, AppState>,
    plan_id: String,
    confirmed_skips: Vec<String>,
) -> Result<ExecuteReport, String> {
    let _guard = acquire_job_guard(&state.job_running)?;
    state.copy_cancel.store(false, Ordering::Relaxed);
    let cancel = Arc::clone(&state.copy_cancel);
    let prepared = lock(&state)?
        .prepare_execute(&plan_id)
        .map_err(|e| e.to_string())?;
    let opts = ExecuteOptions {
        confirmed_skips,
        cancel: Some(cancel),
        ..Default::default()
    };
    run_task(move || {
        Organizer::execute_prepared(prepared, opts, |tick| {
            let _ = app.emit("copy-progress", tick);
        })
        .map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
fn recovery(state: State<AppState>) -> Result<Vec<RecoveryView>, String> {
    lock(&state)?.recovery().map_err(|e| e.to_string())
}

#[tauri::command]
async fn retry_job(
    app: AppHandle,
    state: State<'_, AppState>,
    job_id: String,
) -> Result<ExecuteReport, String> {
    let _guard = acquire_job_guard(&state.job_running)?;
    state.copy_cancel.store(false, Ordering::Relaxed);
    let cancel = Arc::clone(&state.copy_cancel);
    let db_path = lock(&state)?.db_path().to_path_buf();
    run_task(move || {
        Organizer::retry_job_with(&db_path, &job_id, Some(&cancel), |tick| {
            let _ = app.emit("copy-progress", tick);
        })
        .map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
fn undo_preview(state: State<AppState>, job_id: String) -> Result<UndoPreview, String> {
    lock(&state)?.undo_preview(&job_id).map_err(|e| e.to_string())
}

#[tauri::command]
async fn undo_job(state: State<'_, AppState>, job_id: String) -> Result<UndoReport, String> {
    let _guard = acquire_job_guard(&state.job_running)?;
    run_blocking(&state, move |org| org.undo_job(&job_id).map_err(|e| e.to_string())).await
}

#[tauri::command]
fn list_drafts(state: State<AppState>) -> Result<Vec<DraftSummary>, String> {
    lock(&state)?.list_drafts().map_err(|e| e.to_string())
}

#[tauri::command]
async fn open_draft(state: State<'_, AppState>, id: String) -> Result<TaskDetail, String> {
    run_blocking(&state, move |org| org.open_draft(&id).map_err(|e| e.to_string())).await
}

#[tauri::command]
fn tags(state: State<AppState>) -> Result<Vec<TagItem>, String> {
    lock(&state)?.tags().map_err(|e| e.to_string())
}

#[tauri::command]
fn tag_create(
    state: State<AppState>,
    name: String,
    short_label: String,
    aliases: Vec<String>,
) -> Result<TagItem, String> {
    lock(&state)?
        .tag_create(name, short_label, aliases)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn tag_update(
    state: State<AppState>,
    id: String,
    name: String,
    short_label: String,
    aliases: Vec<String>,
) -> Result<(), String> {
    lock(&state)?
        .tag_update(&id, name, short_label, aliases)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn tag_set_enabled(state: State<AppState>, id: String, enabled: bool) -> Result<(), String> {
    lock(&state)?
        .tag_set_enabled(&id, enabled)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn tag_delete(state: State<AppState>, id: String) -> Result<(), String> {
    lock(&state)?.tag_delete(&id).map_err(|e| e.to_string())
}

#[tauri::command]
fn tag_reorder(state: State<AppState>, ids: Vec<String>) -> Result<(), String> {
    lock(&state)?.tag_reorder(&ids).map_err(|e| e.to_string())
}

#[tauri::command]
fn import_tags_file(state: State<AppState>, path: String) -> Result<ImportReport, String> {
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    let text = tuxu_core::decode_text_bytes(&bytes);
    if path.to_ascii_lowercase().ends_with(".json") {
        lock(&state)?.import_tags_json(&text).map_err(|e| e.to_string())
    } else {
        lock(&state)?.import_tags_text(&text).map_err(|e| e.to_string())
    }
}

#[tauri::command]
fn export_tags_file(state: State<AppState>, path: String) -> Result<(), String> {
    let json = lock(&state)?.export_tags_json().map_err(|e| e.to_string())?;
    std::fs::write(&path, json.as_bytes()).map_err(|e| e.to_string())
}

#[tauri::command]
fn list_presets(state: State<AppState>) -> Result<Vec<GroupPreset>, String> {
    lock(&state)?.list_presets().map_err(|e| e.to_string())
}

#[tauri::command]
fn save_preset(
    state: State<AppState>,
    id: Option<String>,
    name: String,
    description: String,
    scene_type: String,
    group_names: Vec<String>,
    default_naming: Option<NamingRule>,
) -> Result<GroupPreset, String> {
    lock(&state)?
        .save_preset(id, name, description, scene_type, group_names, default_naming)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn duplicate_preset(
    state: State<AppState>,
    id: String,
    new_name: Option<String>,
) -> Result<GroupPreset, String> {
    lock(&state)?
        .duplicate_preset(&id, new_name.as_deref())
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn delete_preset(state: State<AppState>, id: String) -> Result<(), String> {
    lock(&state)?.delete_preset(&id).map_err(|e| e.to_string())
}

#[tauri::command]
fn export_presets_file(state: State<AppState>, path: String) -> Result<(), String> {
    let json = lock(&state)?
        .export_presets_json()
        .map_err(|e| e.to_string())?;
    std::fs::write(&path, json.as_bytes()).map_err(|e| e.to_string())
}

#[tauri::command]
fn import_presets_file(state: State<AppState>, path: String) -> Result<ImportReport, String> {
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    let text = tuxu_core::decode_text_bytes(&bytes);
    lock(&state)?
        .import_presets_json(&text)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn save_template(state: State<AppState>, name: String) -> Result<tuxu_core::TaskTemplate, String> {
    lock(&state)?.save_template(&name).map_err(|e| e.to_string())
}

#[tauri::command]
fn list_templates(state: State<AppState>) -> Result<Vec<tuxu_core::TaskTemplate>, String> {
    lock(&state)?.list_templates().map_err(|e| e.to_string())
}

#[tauri::command]
fn apply_template(state: State<AppState>, id: String) -> Result<TaskDetail, String> {
    lock(&state)?.apply_template(&id).map_err(|e| e.to_string())
}

#[tauri::command]
fn delete_template(state: State<AppState>, id: String) -> Result<(), String> {
    lock(&state)?.delete_template(&id).map_err(|e| e.to_string())
}

#[tauri::command]
fn history(state: State<AppState>) -> Result<Vec<tuxu_core::HistoryRow>, String> {
    lock(&state)?.history().map_err(|e| e.to_string())
}

#[tauri::command]
fn job_manifest(state: State<AppState>, job_id: String) -> Result<JobManifest, String> {
    lock(&state)?.job_manifest(&job_id).map_err(|e| e.to_string())
}

#[tauri::command]
fn settings(state: State<AppState>) -> Result<tuxu_core::SettingsView, String> {
    lock(&state)?.settings().map_err(|e| e.to_string())
}

#[tauri::command]
fn set_open_folder_after(
    state: State<AppState>,
    enabled: bool,
) -> Result<tuxu_core::SettingsView, String> {
    lock(&state)?
        .set_open_folder_after(enabled)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn set_retention_days(
    state: State<AppState>,
    days: u32,
) -> Result<tuxu_core::SettingsView, String> {
    lock(&state)?
        .set_retention_days(days)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn set_theme(state: State<AppState>, theme: String) -> Result<tuxu_core::SettingsView, String> {
    lock(&state)?.set_theme(&theme).map_err(|e| e.to_string())
}

#[tauri::command]
fn clear_remembered_output(state: State<AppState>) -> Result<tuxu_core::SettingsView, String> {
    lock(&state)?
        .clear_remembered_output()
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn cleanup_retention(
    state: State<'_, AppState>,
    confirmed: bool,
) -> Result<tuxu_core::ClearReport, String> {
    run_blocking(&state, move |org| {
        org.cleanup_retention(confirmed).map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
fn clear_thumbnail_cache(state: State<AppState>) -> Result<tuxu_core::ClearReport, String> {
    lock(&state)?
        .clear_thumbnail_cache()
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn clear_history(state: State<AppState>) -> Result<tuxu_core::ClearReport, String> {
    lock(&state)?.clear_history().map_err(|e| e.to_string())
}

#[tauri::command]
fn diagnostics(state: State<AppState>) -> Result<tuxu_core::Diagnostics, String> {
    let org = lock(&state)?;
    let dir = org.data_dir().to_path_buf();
    let opened = org.database_open();
    let _ = tuxu_core::write_self_check_log(&dir, &tuxu_core::self_check(&dir, opened));
    Ok(org.diagnostics())
}

#[tauri::command]
fn suggest_export_dir(state: State<AppState>) -> Result<String, String> {
    Ok(lock(&state)?.export_dir().display().to_string())
}

#[tauri::command]
async fn export_job_file(
    state: State<'_, AppState>,
    job_id: String,
    path: String,
    kind: String,
) -> Result<(), String> {
    let org = Arc::clone(&state.org);
    run_task(move || {
        let text = {
            let guard = org.lock().map_err(|e| e.to_string())?;
            if kind == "csv" {
                guard.export_job_csv(&job_id).map_err(|e| e.to_string())?
            } else {
                guard.export_job_json(&job_id).map_err(|e| e.to_string())?
            }
        };
        std::fs::write(&path, text.as_bytes()).map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
async fn export_plan_csv_file(
    state: State<'_, AppState>,
    confirmed_skips: Vec<String>,
    path: String,
) -> Result<(), String> {
    let org = Arc::clone(&state.org);
    run_task(move || {
        let text = {
            let mut guard = org.lock().map_err(|e| e.to_string())?;
            guard
                .export_plan_csv(&confirmed_skips)
                .map_err(|e| e.to_string())?
        };
        std::fs::write(&path, text.as_bytes()).map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
fn open_output_dir(state: State<AppState>, job_id: String) -> Result<String, String> {
    let path = lock(&state)?
        .task_output_dir(&job_id)
        .map_err(|e| e.to_string())?;
    std::process::Command::new("explorer")
        .arg(&path)
        .spawn()
        .map_err(|e| format!("无法打开归档目录：{e}"))?;
    Ok(path)
}

#[tauri::command]
fn open_source_file(state: State<AppState>, photo_id: String) -> Result<(), String> {
    let path = lock(&state)?
        .source_file_for_viewing(&photo_id)
        .map_err(|e| e.to_string())?;
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = std::ffi::OsStr::new(&path)
        .encode_wide()
        .chain(Some(0))
        .collect();
    let result = unsafe {
        windows_sys::Win32::UI::Shell::ShellExecuteW(
            std::ptr::null_mut(),
            std::ptr::null(),
            wide.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1,
        )
    };
    if result as isize <= 32 {
        return Err("无法打开原文件，请安装支持该格式的图片查看器，或检查文件是否存在".into());
    }
    Ok(())
}

fn fit_main_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let Ok(Some(monitor)) = window.current_monitor() else {
        return;
    };
    let scale = monitor.scale_factor();
    if scale <= 0.0 {
        return;
    }
    let area = monitor.work_area();
    let avail_w = area.size.width as f64 / scale;
    let avail_h = area.size.height as f64 / scale;
    if avail_w < 1380.0 || avail_h < 860.0 {
        let width = (avail_w - 24.0).clamp(800.0, 1380.0).min(avail_w);
        let height = (avail_h - 24.0).clamp(400.0, 860.0).min(avail_h);
        let _ = window.set_size(tauri::Size::Logical(tauri::LogicalSize { width, height }));
        let _ = window.center();
    }
}

pub fn run() {
    let Some(_instance_guard) = single_instance() else {
        tell("图序归档已在运行中。");
        std::process::exit(1);
    };
    let dir = data_dir();
    let org = match Organizer::open(&dir) {
        Ok(org) => org,
        Err(err) => {
            tell(&format!("无法打开数据库：{err}"));
            std::process::exit(1);
        }
    };
    let report = tuxu_core::self_check(org.data_dir(), org.database_open());
    if let Err(err) = tuxu_core::write_self_check_log(org.data_dir(), &report) {
        tell(&format!("无法写入启动自检日志：{err}"));
        std::process::exit(1);
    }
    if report.startup != "ok" {
        tell(&report.reason);
        std::process::exit(1);
    }
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            fit_main_window(app.handle());
            Ok(())
        })
        .manage(AppState {
            org: Arc::new(Mutex::new(org)),
            initial_check: report,
            scan_cancel: Arc::new(AtomicBool::new(false)),
            copy_cancel: Arc::new(AtomicBool::new(false)),
            quality_cancel: Arc::new(AtomicBool::new(false)),
            job_running: Arc::new(AtomicBool::new(false)),
        })
        .invoke_handler(tauri::generate_handler![
            bootstrap,
            scan_folder,
            cancel_scan,
            cancel_copy,
            cancel_quality,
            inspect_quality,
            task_detail,
            photos,
            thumb_window,
            thumbnail,
            set_task_name,
            set_dir_layout,
            set_naming_rule,
            set_output_root,
            check_paths,
            set_strict,
            set_groups_text,
            add_group,
            rename_group,
            delete_group,
            reorder_group_to,
            sort_groups_by_name,
            group_by_time,
            group_by_time_gap,
            group_by_fixed_size,
            group_evenly,
            set_quantities,
            apply_preset,
            assign_photos_to_group,
            create_group_for_photos,
            move_photo,
            resort_by_mtime,
            resort_by_filename,
            assign_tag,
            batch_assign_tag,
            set_custom_name,
            confirm_photo,
            batch_confirm_photos,
            confirm_all_ready,
            exclude_photo,
            batch_exclude_photos,
            acknowledge_review,
            review_risk,
            undo_edit,
            redo_edit,
            build_plan,
            execute_plan,
            recovery,
            retry_job,
            undo_preview,
            undo_job,
            list_drafts,
            open_draft,
            tags,
            tag_create,
            tag_update,
            tag_set_enabled,
            tag_delete,
            tag_reorder,
            import_tags_file,
            export_tags_file,
            list_presets,
            save_preset,
            duplicate_preset,
            delete_preset,
            export_presets_file,
            import_presets_file,
            save_template,
            list_templates,
            apply_template,
            delete_template,
            history,
            job_manifest,
            settings,
            set_open_folder_after,
            set_retention_days,
            set_theme,
            clear_remembered_output,
            cleanup_retention,
            clear_thumbnail_cache,
            clear_history,
            diagnostics,
            suggest_export_dir,
            export_job_file,
            export_plan_csv_file,
            open_output_dir,
            open_source_file
        ])
        .run(tauri::generate_context!())
        .expect("启动窗口失败");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_job_guard_rejects_second_holder_and_releases_on_drop() {
        let flag = Arc::new(AtomicBool::new(false));
        {
            let _first = acquire_job_guard(&flag).expect("first acquisition must succeed");
            assert!(flag.load(Ordering::SeqCst));
            let err = acquire_job_guard(&flag)
                .err()
                .expect("second acquisition must be rejected");
            assert!(err.contains("正在执行"));
        }
        assert!(!flag.load(Ordering::SeqCst));
        assert!(acquire_job_guard(&flag).is_ok());
    }
}
