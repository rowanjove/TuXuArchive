use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

pub const APP_VERSION: &str = "1.0.0";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NamingRule {
    #[serde(default = "default_preset")]
    pub preset: String,
    #[serde(default = "default_pattern")]
    pub pattern: String,
    #[serde(default)]
    pub prefix: String,
    #[serde(default)]
    pub suffix: String,
    #[serde(default = "default_date_format")]
    pub date_format: String,
    #[serde(default = "default_true")]
    pub include_group: bool,
    #[serde(default)]
    pub include_tag: bool,
    #[serde(default)]
    pub include_original: bool,
    #[serde(default = "default_seq_scope")]
    pub seq_scope: String,
    #[serde(default = "default_seq_start")]
    pub seq_start: u32,
    #[serde(default = "default_seq_digits")]
    pub seq_digits: u32,
    #[serde(default = "default_separator")]
    pub separator: String,
}

fn default_preset() -> String {
    "date_group_seq".into()
}
fn default_pattern() -> String {
    "{date}_{group}_{seq}".into()
}
fn default_date_format() -> String {
    "YYYYMMDD".into()
}
fn default_seq_scope() -> String {
    "per_group".into()
}
fn default_seq_start() -> u32 {
    1
}
fn default_seq_digits() -> u32 {
    3
}
fn default_separator() -> String {
    "_".into()
}
fn default_true() -> bool {
    true
}
fn default_dir_layout() -> String {
    "task_and_group".into()
}
fn default_dir_pattern() -> String {
    "{task}/{group}".into()
}

impl Default for NamingRule {
    fn default() -> Self {
        Self {
            preset: default_preset(),
            pattern: default_pattern(),
            prefix: String::new(),
            suffix: String::new(),
            date_format: default_date_format(),
            include_group: true,
            include_tag: false,
            include_original: false,
            seq_scope: default_seq_scope(),
            seq_start: default_seq_start(),
            seq_digits: default_seq_digits(),
            separator: default_separator(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagItem {
    pub id: String,
    pub name: String,
    pub short_label: String,
    pub aliases: Vec<String>,
    pub sort_order: i64,
    pub enabled: bool,
    pub source: String,
    #[serde(default)]
    pub category: String,
    pub updated_at: String,
}

fn default_quality_status() -> String {
    "unchecked".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PhotoQuality {
    #[serde(default = "default_quality_status")]
    pub status: String,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    #[serde(default)]
    pub issues: Vec<String>,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub luma_mean: Option<f32>,
    #[serde(default)]
    pub sharpness: Option<f32>,
    #[serde(default)]
    pub pixel_checked: bool,
}

impl Default for PhotoQuality {
    fn default() -> Self {
        Self {
            status: default_quality_status(),
            width: None,
            height: None,
            issues: Vec::new(),
            summary: String::new(),
            luma_mean: None,
            sharpness: None,
            pixel_checked: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityTick {
    pub processed: u32,
    pub total: u32,
    pub current_file: String,
    pub cancelled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityReport {
    pub total: u32,
    pub checked: u32,
    pub ok_count: u32,
    pub warn_count: u32,
    pub bad_count: u32,
    pub truncated_count: u32,
    pub low_res_count: u32,
    pub blur_count: u32,
    pub underexposed_count: u32,
    pub overexposed_count: u32,
    pub low_contrast_count: u32,
    pub cancelled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Photo {
    pub id: String,
    pub source_path: String,
    pub file_name: String,
    pub extension: String,
    pub size: u64,
    pub mtime_ns: i64,
    pub capture_text: Option<String>,
    pub capture_sort_key: Option<i64>,
    #[serde(default)]
    pub exif_offset: Option<String>,
    #[serde(default)]
    pub sort_time_text: Option<String>,
    #[serde(default)]
    pub time_uncertain: bool,
    pub time_source: String,
    pub time_note: String,
    pub equal_timestamp: bool,
    pub tie_break: String,
    pub tie_break_is_capture_order: bool,
    pub corrupt: bool,
    pub preview_state: String,
    #[serde(default)]
    pub quality: PhotoQuality,
    pub sort_index: u32,
    #[serde(default)]
    pub original_scan_index: u32,
    pub group_id: Option<String>,
    pub tag_id: Option<String>,
    #[serde(default)]
    pub custom_name: Option<String>,
    pub confirmed: bool,
    pub needs_review: bool,
    pub excluded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PresetTagItem {
    pub tag_id: String,
    pub name: String,
    pub short_label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Group {
    pub id: String,
    pub name: String,
    pub sort_order: u32,
    #[serde(default)]
    pub preset_id: Option<String>,
    #[serde(default)]
    pub preset_name: String,
    #[serde(default)]
    pub suggested_tags: Vec<PresetTagItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub source_root: String,
    pub recursive: bool,
    pub scan_complete: bool,
    pub task_name: String,
    pub output_root: String,
    #[serde(default = "default_dir_layout")]
    pub dir_layout: String,
    #[serde(default = "default_dir_pattern")]
    pub dir_pattern: String,
    #[serde(default)]
    pub naming_rule: NamingRule,
    pub strict_hash: bool,
    pub status: String,
    pub mappings_valid: bool,
    pub revision: u64,
    pub photos: Vec<Photo>,
    pub groups: Vec<Group>,
    pub ignored_count: u32,
    pub ignored_reasons: BTreeMap<String, u32>,
    #[serde(default)]
    pub favorite_tag_ids: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Counts {
    pub scanned: u32,
    pub grouped: u32,
    pub ready: u32,
    pub ungrouped: u32,
    pub excluded: u32,
    pub needs_review: u32,
    pub corrupt: u32,
    #[serde(default)]
    pub fallback_time_count: u32,
    #[serde(default)]
    pub time_uncertain_count: u32,
    #[serde(default)]
    pub quality_warn_count: u32,
    #[serde(default)]
    pub quality_bad_count: u32,
    #[serde(default)]
    pub quality_unchecked_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathCheckView {
    pub valid: bool,
    pub source_exists: bool,
    pub source_is_dir: bool,
    pub output_writable: bool,
    pub conflict: bool,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupView {
    pub id: String,
    pub name: String,
    pub sort_order: u32,
    pub count: u32,
    pub ready_count: u32,
    pub tagged_count: u32,
    pub needs_review: u32,
    pub status: String,
    #[serde(default)]
    pub start_time: Option<String>,
    #[serde(default)]
    pub end_time: Option<String>,
    #[serde(default)]
    pub preset_id: Option<String>,
    #[serde(default)]
    pub preset_name: String,
    #[serde(default)]
    pub suggested_tags: Vec<PresetTagItem>,
    #[serde(default)]
    pub duplicate_count: u32,
    #[serde(default)]
    pub quality_issue_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskDetail {
    pub task_id: String,
    pub source_root: String,
    pub output_root: String,
    pub task_name: String,
    pub dir_layout: String,
    pub dir_pattern: String,
    pub naming_rule: NamingRule,
    pub dir_needs_task: bool,
    pub dir_needs_group: bool,
    pub scan_complete: bool,
    pub recursive: bool,
    pub strict_hash: bool,
    pub status: String,
    pub mappings_valid: bool,
    pub revision: u64,
    pub counts: Counts,
    pub groups: Vec<GroupView>,
    pub ignored_count: u32,
    pub ignored_reasons: BTreeMap<String, u32>,
    pub time_span: Option<(String, String)>,
    pub can_undo: bool,
    pub can_redo: bool,
    #[serde(default)]
    pub last_undo_label: Option<String>,
    #[serde(default)]
    pub last_redo_label: Option<String>,
    pub sample_preview_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhotoView {
    pub id: String,
    pub sort_index: u32,
    pub original_scan_index: u32,
    pub file_name: String,
    pub source_path: String,
    pub extension: String,
    pub size: u64,
    pub capture_text: Option<String>,
    pub exif_offset: Option<String>,
    pub sort_time_text: Option<String>,
    pub time_uncertain: bool,
    pub time_source: String,
    pub time_note: String,
    pub equal_timestamp: bool,
    pub tie_break: String,
    pub tie_break_is_capture_order: bool,
    pub order_note: String,
    pub corrupt: bool,
    pub preview_state: String,
    #[serde(default)]
    pub quality: PhotoQuality,
    pub group_id: Option<String>,
    pub group_name: Option<String>,
    pub tag_id: Option<String>,
    pub tag_name: Option<String>,
    pub short_label: Option<String>,
    pub custom_name: Option<String>,
    pub preview_stem: String,
    pub preview_rel_path: String,
    pub confirmed: bool,
    pub needs_review: bool,
    pub excluded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhotoPage {
    pub total: usize,
    pub offset: usize,
    pub items: Vec<PhotoView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Issue {
    pub code: String,
    pub severity: String,
    pub blocks: bool,
    pub message: String,
    pub photo_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanItem {
    pub photo_id: String,
    pub source_path: String,
    pub source_name: String,
    pub target_path: String,
    pub relative_path: String,
    pub group_name: String,
    pub target_name: String,
    pub extension: String,
    pub size: u64,
    pub mtime_ns: i64,
    pub blocked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionPlan {
    pub id: String,
    pub task_id: String,
    pub revision: u64,
    pub blocked: bool,
    pub issues: Vec<Issue>,
    pub items: Vec<PlanItem>,
    pub copy_count: u32,
    pub blocked_count: u32,
    pub bytes: u64,
    pub free_bytes: Option<u64>,
    pub shortfall_bytes: u64,
    pub directories: Vec<String>,
    pub group_counts: Vec<GroupFileCount>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupFileCount {
    pub group_name: String,
    pub count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemResult {
    pub photo_id: String,
    pub source_path: String,
    pub target_path: String,
    pub status: String,
    pub message: String,
    pub code: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecuteReport {
    pub job_id: String,
    pub status: String,
    pub succeeded: u32,
    pub failed: u32,
    pub skipped: u32,
    pub not_started: u32,
    pub output_root: String,
    pub items: Vec<ItemResult>,
    pub issues: Vec<Issue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobFile {
    pub path: String,
    pub size: u64,
    pub sha256: String,
    pub created_by_job: bool,
    #[serde(default)]
    pub identity: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobItem {
    pub photo_id: String,
    pub source_path: String,
    pub target_path: String,
    pub relative_path: String,
    pub target_name: String,
    pub extension: String,
    pub group_name: String,
    pub expected_size: u64,
    pub expected_mtime_ns: i64,
    pub status: String,
    pub message: String,
    pub code: String,
    pub temp_path: Option<String>,
    pub sha256: Option<String>,
    pub bytes: u64,
    #[serde(default)]
    pub source_name: String,
    #[serde(default)]
    pub capture_text: String,
    #[serde(default)]
    pub time_source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobManifest {
    pub job_id: String,
    pub task_id: String,
    pub output_root: String,
    pub task_name: String,
    pub strict_hash: bool,
    pub status: String,
    pub created_at: String,
    pub items: Vec<JobItem>,
    pub created_dirs: Vec<String>,
    pub files: Vec<JobFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UndoPreview {
    pub job_id: String,
    pub will_delete: Vec<String>,
    pub will_refuse: Vec<String>,
    pub refuse_reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UndoReport {
    pub job_id: String,
    pub deleted: Vec<String>,
    pub refused: Vec<String>,
    pub refuse_reasons: Vec<String>,
    pub removed_dirs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DraftSummary {
    pub id: String,
    pub source_root: String,
    pub task_name: String,
    pub status: String,
    pub updated_at: String,
    pub photo_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryView {
    pub job_id: String,
    pub status: String,
    pub succeeded: u32,
    pub failed: u32,
    pub not_started: u32,
    pub in_progress: u32,
    pub leftover_temps: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportReport {
    pub added: u32,
    pub updated: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelfCheckReport {
    pub windows_ok: bool,
    pub windows_version: String,
    pub arch: String,
    pub manifest_status: String,
    pub webview2_version: Option<String>,
    pub webview2_ok: bool,
    pub fixed_runtime: bool,
    pub data_dir: String,
    pub data_dir_writable: bool,
    pub database_open: bool,
    pub leftover_temps: Vec<String>,
    pub startup: String,
    pub reason: String,
    pub app_version: String,
    pub program_dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClearReport {
    pub removed_files: u32,
    pub note: String,
}

#[derive(Clone, Copy)]
pub struct ScanOptions {
    pub recursive: bool,
}

#[derive(Clone, Default)]
pub struct ExecuteOptions {
    pub confirmed_skips: Vec<String>,
    pub stop_after_temp_index: Option<usize>,
    pub cancel_after_index: Option<usize>,
    pub cancel: Option<Arc<AtomicBool>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanTick {
    pub discovered: u32,
    pub ignored: u32,
    pub current_file: String,
    pub cancelled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanControl {
    Continue,
    Cancel,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CopyTick {
    pub file_name: String,
    pub succeeded: u32,
    pub failed: u32,
    pub skipped: u32,
    pub remaining: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskTemplate {
    pub id: String,
    pub name: String,
    pub task_name: String,
    pub dir_layout: String,
    pub dir_pattern: String,
    pub naming_rule: NamingRule,
    pub group_names: Vec<String>,
    pub tag_ids: Vec<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryRow {
    pub job_id: String,
    pub task_name: String,
    pub status: String,
    pub output_root: String,
    pub created_at: String,
    pub photo_count: u32,
    pub group_count: u32,
    pub failed: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostics {
    pub version: String,
    pub windows_version: String,
    pub webview2_ok: bool,
    pub webview2_version: Option<String>,
    pub program_dir: String,
    pub data_dir: String,
    pub last_self_check: String,
    pub leftover_temp_count: usize,
    pub attempted_download: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewRisk {
    pub affected: u32,
    pub file_names: Vec<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingsView {
    pub output_root: String,
    pub open_folder_after: bool,
    pub retention_days: u32,
    pub strict_hash: bool,
    #[serde(default = "default_theme")]
    pub theme: String,
}

fn default_theme() -> String {
    "light".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupPreset {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub scene_type: String,
    #[serde(default)]
    pub group_names: Vec<String>,
    #[serde(default)]
    pub items: Vec<PresetTagItem>,
    #[serde(default)]
    pub default_naming: Option<NamingRule>,
    pub version: u32,
    pub source: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThumbWindow {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone)]
pub struct EditSnap {
    pub label: String,
    pub groups: Vec<Group>,
    pub dir_layout: String,
    pub dir_pattern: String,
    pub naming_rule: NamingRule,
    pub photos: Vec<PhotoSnap>,
}

#[derive(Debug, Clone)]
pub struct PhotoSnap {
    pub id: String,
    pub source_path: String,
    pub sort_index: u32,
    pub group_id: Option<String>,
    pub tag_id: Option<String>,
    pub custom_name: Option<String>,
    pub confirmed: bool,
    pub needs_review: bool,
    pub excluded: bool,
}

pub fn dir_pattern_for_layout(layout: &str, custom_pattern: &str) -> String {
    match layout {
        "task_and_group" => "{task}/{group}".into(),
        "group_only" => "{group}".into(),
        "task_only" => "{task}".into(),
        "date_and_group" => "{month}/{group}".into(),
        "flat" => String::new(),
        "custom" => custom_pattern.trim().to_string(),
        _ => "{task}/{group}".into(),
    }
}

pub fn pattern_uses_group(task: &Task) -> bool {
    let dir_pat = dir_pattern_for_layout(&task.dir_layout, &task.dir_pattern);
    dir_pat.contains("{group}")
}

pub fn pattern_uses_task(task: &Task) -> bool {
    let dir_pat = dir_pattern_for_layout(&task.dir_layout, &task.dir_pattern);
    dir_pat.contains("{task}") || task.naming_rule.pattern.contains("{task}")
}

pub fn counts_of(task: &Task) -> Counts {
    let active: Vec<&Photo> = task.photos.iter().filter(|p| !p.excluded).collect();
    let scanned = active.len() as u32;
    let needs_group = pattern_uses_group(task);
    let grouped = active.iter().filter(|p| p.group_id.is_some()).count() as u32;
    let ready = active
        .iter()
        .filter(|p| {
            (!needs_group || p.group_id.is_some()) && p.confirmed && !p.needs_review && !p.corrupt
        })
        .count() as u32;
    let ungrouped = active.iter().filter(|p| p.group_id.is_none()).count() as u32;
    let fallback_time_count = active
        .iter()
        .filter(|p| p.time_source != "exif_original")
        .count() as u32;
    let time_uncertain_count = active.iter().filter(|p| p.time_uncertain).count() as u32;
    let quality_warn_count = active.iter().filter(|p| p.quality.status == "warn").count() as u32;
    let quality_bad_count = active.iter().filter(|p| p.quality.status == "bad").count() as u32;
    let quality_unchecked_count = active
        .iter()
        .filter(|p| !p.quality.pixel_checked && p.preview_state == "available" && !p.corrupt)
        .count() as u32;
    Counts {
        scanned,
        grouped,
        ready,
        ungrouped,
        excluded: task.photos.iter().filter(|p| p.excluded).count() as u32,
        needs_review: active.iter().filter(|p| p.needs_review).count() as u32,
        corrupt: task.photos.iter().filter(|p| p.corrupt).count() as u32,
        fallback_time_count,
        time_uncertain_count,
        quality_warn_count,
        quality_bad_count,
        quality_unchecked_count,
    }
}
