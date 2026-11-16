import "./styles.css";
import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import * as tauriDialog from "@tauri-apps/plugin-dialog";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { listen as tauriListen } from "@tauri-apps/api/event";
import {
  ARCHIVE_BUCKETS,
  CACHE_CLEAR_NOTE,
  DIR_LAYOUT_OPTIONS,
  NAMING_PRESETS,
  archiveBoard,
  buildArchivePreviewGroups,
  canOpenStep,
  computeSelectionRange,
  contextualHelpContent,
  escapeHtml,
  formatArchivePlanCsv,
  importStepDone,
  parseCountsPaste,
  parseGroupBatchText,
  plainError,
  previewSamplePath,
  shortcutAction,
  validateComponentName,
  validateImportPaths,
  type DirLayoutKey,
  type NamingRuleDto,
  type PlanItemDto,
  type StepId,
  type TaskProgress,
} from "./workflow";

type TagItem = {
  id: string;
  name: string;
  short_label: string;
  aliases: string[];
  sort_order: number;
  enabled: boolean;
  source: string;
  category: string;
  updated_at: string;
};

type GroupPreset = {
  id: string;
  name: string;
  description: string;
  scene_type: string;
  group_names: string[];
  default_naming?: NamingRuleDto | null;
  version: number;
  source: string;
};

type GroupView = {
  id: string;
  name: string;
  sort_order: number;
  count: number;
  ready_count: number;
  tagged_count: number;
  needs_review: number;
  status: string;
  start_time?: string | null;
  end_time?: string | null;
};

type Counts = {
  scanned: number;
  grouped: number;
  ready: number;
  ungrouped: number;
  excluded: number;
  needs_review: number;
  corrupt: number;
  fallback_time_count: number;
  time_uncertain_count: number;
  quality_warn_count: number;
  quality_bad_count: number;
  quality_unchecked_count: number;
};

type TaskDetail = {
  task_id: string;
  source_root: string;
  output_root: string;
  task_name: string;
  dir_layout: string;
  dir_pattern: string;
  naming_rule: NamingRuleDto;
  dir_needs_task: boolean;
  dir_needs_group: boolean;
  scan_complete: boolean;
  recursive: boolean;
  strict_hash: boolean;
  status: string;
  mappings_valid: boolean;
  revision: number;
  counts: Counts;
  groups: GroupView[];
  ignored_count: number;
  ignored_reasons: Record<string, number>;
  time_span?: [string, string] | null;
  can_undo: boolean;
  can_redo: boolean;
  last_undo_label?: string | null;
  last_redo_label?: string | null;
  sample_preview_path: string;
};

type PhotoQuality = {
  status: string;
  width?: number | null;
  height?: number | null;
  issues: string[];
  summary: string;
  pixel_checked: boolean;
};

type PhotoView = {
  id: string;
  sort_index: number;
  original_scan_index: number;
  file_name: string;
  source_path: string;
  extension: string;
  size: number;
  capture_text?: string | null;
  sort_time_text?: string | null;
  time_uncertain: boolean;
  time_source: string;
  order_note: string;
  corrupt: boolean;
  preview_state: string;
  quality: PhotoQuality;
  group_id?: string | null;
  group_name?: string | null;
  tag_id?: string | null;
  tag_name?: string | null;
  short_label?: string | null;
  custom_name?: string | null;
  preview_stem: string;
  preview_rel_path: string;
  confirmed: boolean;
  needs_review: boolean;
  excluded: boolean;
};

type PhotoPage = {
  total: number;
  offset: number;
  items: PhotoView[];
};

type ExecutionPlan = {
  id: string;
  task_id: string;
  revision: number;
  blocked: boolean;
  issues: Array<{ code: string; severity: string; blocks: boolean; message: string; photo_id?: string | null }>;
  items: PlanItemDto[];
  copy_count: number;
  blocked_count: number;
  bytes: number;
  free_bytes?: number | null;
  shortfall_bytes: number;
  directories: string[];
  group_counts: Array<{ group_name: string; count: number }>;
};

type ExecuteReport = {
  job_id: string;
  status: string;
  succeeded: number;
  failed: number;
  skipped: number;
  not_started: number;
  output_root: string;
  items: Array<{ photo_id: string; source_path: string; target_path: string; status: string; message: string; code: string; bytes: number }>;
  issues: Array<{ code: string; severity: string; blocks: boolean; message: string }>;
};

type DraftSummary = { id: string; source_root: string; task_name: string; status: string; updated_at: string; photo_count: number };
type RecoveryView = { job_id: string; status: string; succeeded: number; failed: number; not_started: number; in_progress: number; leftover_temps: string[] };
type TaskTemplate = { id: string; name: string; task_name: string; dir_layout: string; dir_pattern: string; naming_rule: NamingRuleDto; group_names: string[]; created_at: string };
type HistoryRow = { job_id: string; task_name: string; status: string; output_root: string; created_at: string; photo_count: number; group_count: number; failed: number };
type SettingsView = { output_root: string; open_folder_after: boolean; retention_days: number; strict_hash: boolean; theme: string };
type UndoPreview = { job_id: string; will_delete: string[]; will_refuse: string[]; refuse_reasons: string[] };

type ViewId = StepId | "presets" | "tags" | "templates" | "history" | "settings" | "help";

const appState = {
  view: "import" as ViewId,
  lastWorkStep: "import" as StepId,
  detail: null as TaskDetail | null,
  draftReopened: false,
  scanning: false,
  copying: false,
  tags: [] as TagItem[],
  presets: [] as GroupPreset[],
  templates: [] as TaskTemplate[],
  drafts: [] as DraftSummary[],
  recovery: [] as RecoveryView[],
  settings: null as SettingsView | null,
  // Step 2 manual grouping
  manualPhotos: [] as PhotoView[],
  manualSelectedIds: new Set<string>(),
  manualAnchorId: null as string | null,
  // Step 3 preview & detail
  previewFilter: "all" as "all" | "ungrouped" | "review" | "ready",
  activeGroupId: null as string | null, // null = all
  previewPhotos: [] as PhotoView[],
  previewSelectedIds: new Set<string>(),
  previewMode: "batch" as "batch" | "detail",
  detailSubMode: "single" as "single" | "grid",
  detailPhotoId: null as string | null,
  detailAnchorId: null as string | null,
  detailSelectedIds: new Set<string>(),
  viewerZoomActual: false,
  viewerRotation: 0,
  // Step 4 archive
  plan: null as ExecutionPlan | null,
  lastReport: null as ExecuteReport | null,
  collapsedGroups: new Set<string>(),
  // Thumbnail cache
  thumbCache: new Map<string, string>(),
  thumbInFlight: new Map<string, Promise<string | null>>(),
};

function el<T extends HTMLElement = HTMLElement>(id: string): T {
  const node = document.getElementById(id);
  if (!node) throw new Error(`Missing DOM node #${id}`);
  return node as T;
}

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  return tauriInvoke<T>(cmd, args);
}

async function runAction(fn: () => Promise<void> | void): Promise<void> {
  try {
    await fn();
  } catch (err) {
    showStatus(plainError(err), "error");
  }
}

function resetTaskUiState() {
  appState.manualSelectedIds.clear();
  appState.manualAnchorId = null;
  appState.previewSelectedIds.clear();
  appState.activeGroupId = null;
  appState.previewFilter = "all";
  appState.previewMode = "batch";
  appState.detailPhotoId = null;
  appState.detailAnchorId = null;
  appState.detailSelectedIds.clear();
  appState.viewerZoomActual = false;
  appState.viewerRotation = 0;
  appState.plan = null;
  appState.lastReport = null;
  appState.collapsedGroups.clear();
}

function showStatus(text: string, kind: "info" | "ok" | "warn" | "error" = "info", action?: { label: string; onClick: () => void }) {
  const bar = el("status-bar");
  bar.className = `status-bar ${kind}`;
  el("status-text").textContent = text;
  const actBtn = el<HTMLButtonElement>("status-action");
  if (action) {
    actBtn.textContent = action.label;
    actBtn.classList.remove("hidden");
    actBtn.onclick = action.onClick;
  } else {
    actBtn.classList.add("hidden");
    actBtn.onclick = null;
  }
}

function hideStatus() {
  el("status-bar").classList.add("hidden");
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let val = bytes / 1024;
  let u = 0;
  while (val >= 1024 && u < units.length - 1) {
    val /= 1024;
    u++;
  }
  return `${val.toFixed(1)} ${units[u]}`;
}

function showModal(opts: {
  title: string;
  bodyHtml?: string;
  bodyNode?: HTMLElement;
  buttons: Array<{ label: string; primary?: boolean; danger?: boolean; value?: string; onClick?: (dlg: HTMLDialogElement) => void | Promise<void> }>;
}) {
  const dlg = el<HTMLDialogElement>("app-dialog");
  if (dlg.open) {
    dlg.close("dismiss");
  }
  el("dialog-title").textContent = opts.title;
  const body = el("dialog-body");
  body.innerHTML = "";
  if (opts.bodyNode) {
    body.appendChild(opts.bodyNode);
  } else if (opts.bodyHtml) {
    body.innerHTML = opts.bodyHtml;
  }
  const footer = el("dialog-footer");
  footer.innerHTML = "";
  for (const btnCfg of opts.buttons) {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.textContent = btnCfg.label;
    if (btnCfg.primary) btn.classList.add("primary");
    if (btnCfg.danger) btn.classList.add("danger");
    btn.onclick = async () => {
      if (btnCfg.onClick) {
        await btnCfg.onClick(dlg);
      } else {
        dlg.close(btnCfg.value ?? "");
      }
    };
    footer.appendChild(btn);
  }
  if (!dlg.open) {
    dlg.showModal();
  }
}

function showConfirmModal(title: string, message: string, confirmLabel = "确认", danger = false): Promise<boolean> {
  return new Promise((resolve) => {
    const dlg = el<HTMLDialogElement>("app-dialog");
    const onClose = () => {
      const ok = dlg.returnValue === "confirm";
      dlg.returnValue = "";
      dlg.removeEventListener("close", onClose);
      resolve(ok);
    };
    dlg.addEventListener("close", onClose);
    showModal({
      title,
      bodyHtml: `<div style="white-space:pre-wrap;line-height:1.6">${escapeHtml(message)}</div>`,
      buttons: [
        { label: "取消", value: "cancel", onClick: (d) => d.close("cancel") },
        { label: confirmLabel, primary: !danger, danger, value: "confirm", onClick: (d) => d.close("confirm") },
      ],
    });
  });
}

function getProgress(): TaskProgress {
  const d = appState.detail;
  return {
    scanComplete: d?.scan_complete ?? false,
    draftReopened: appState.draftReopened,
    groupCount: d?.groups.length ?? 0,
    dirNeedsGroup: d?.dir_needs_group ?? true,
    dirNeedsTask: d?.dir_needs_task ?? true,
    hasTaskName: Boolean(d?.task_name.trim()),
    grouped: d?.counts.grouped ?? 0,
    readyCount: d?.counts.ready ?? 0,
    totalActive: d?.counts.scanned ?? 0,
  };
}

async function loadThumb(photoId: string, maxEdge = 160): Promise<string | null> {
  const key = `${photoId}:${maxEdge}`;
  const cached = appState.thumbCache.get(key);
  if (cached) return cached;
  const inflight = appState.thumbInFlight.get(key);
  if (inflight) return inflight;
  const p = call<{ available: boolean; mime: string; data_base64: string }>("thumbnail", {
    photoId,
    maxEdge,
  })
    .then((res) => {
      if (res.available && res.data_base64) {
        const url = `data:${res.mime};base64,${res.data_base64}`;
        if (appState.thumbCache.size > 400) {
          const first = appState.thumbCache.keys().next().value;
          if (first) appState.thumbCache.delete(first);
        }
        appState.thumbCache.set(key, url);
        return url;
      }
      return null;
    })
    .catch(() => null)
    .finally(() => {
      appState.thumbInFlight.delete(key);
    });
  appState.thumbInFlight.set(key, p);
  return p;
}

function attachThumbImg(img: HTMLImageElement, photoId: string, maxEdge = 160) {
  const key = `${photoId}:${maxEdge}`;
  const cached = appState.thumbCache.get(key);
  if (cached) {
    img.src = cached;
    return;
  }
  loadThumb(photoId, maxEdge).then((url) => {
    if (url && img.isConnected) {
      img.src = url;
    }
  });
}

function switchView(view: ViewId) {
  appState.view = view;
  if (view === "import" || view === "rules" || view === "preview" || view === "archive") {
    appState.lastWorkStep = view;
  }
  const panels: Record<ViewId, string> = {
    import: "step-import",
    rules: "step-rules",
    preview: "step-preview",
    archive: "step-archive",
    presets: "view-presets",
    tags: "view-tags",
    templates: "view-templates",
    history: "view-history",
    settings: "view-settings",
    help: "view-help",
  };
  for (const [k, pid] of Object.entries(panels)) {
    el(pid).classList.toggle("hidden", k !== view);
  }
  el("manage-menu").classList.add("hidden");
  el("btn-manage").setAttribute("aria-expanded", "false");
  renderTopbar();

  if (view === "rules") renderRulesStep();
  if (view === "preview") refreshPreviewStep();
  if (view === "archive") refreshArchivePlan();
  if (view === "presets") renderPresetsView();
  if (view === "tags") renderTagsView();
  if (view === "templates") renderTemplatesView();
  if (view === "history") renderHistoryView();
  if (view === "settings") renderSettingsView();
}

function renderTopbar() {
  const d = appState.detail;
  const prog = getProgress();
  const ctx = el("top-context");
  if (!d) {
    ctx.textContent = "等待导入图片";
  } else {
    ctx.textContent = `${d.task_name || "未命名任务"} · ${d.counts.scanned} 张`;
  }
  const steps: StepId[] = ["import", "rules", "preview", "archive"];
  for (const st of steps) {
    const btn = el<HTMLButtonElement>(`step-btn-${st}`);
    btn.classList.toggle("active", appState.view === st);
    btn.disabled = !canOpenStep(st, prog);
    const done =
      st === "import"
        ? importStepDone(prog)
        : st === "rules"
          ? importStepDone(prog) && (!prog.dirNeedsGroup || prog.grouped > 0)
          : st === "preview"
            ? prog.readyCount > 0 && prog.readyCount === prog.totalActive
            : d?.status === "archived";
    btn.classList.toggle("done", Boolean(done));
  }
  const undoBtn = el<HTMLButtonElement>("btn-undo");
  const redoBtn = el<HTMLButtonElement>("btn-redo");
  undoBtn.disabled = !(d?.can_undo);
  redoBtn.disabled = !(d?.can_redo);
  undoBtn.title = d?.last_undo_label ? `撤销：${d.last_undo_label} (Ctrl+Z)` : "撤销上一步调整 (Ctrl+Z)";
  redoBtn.title = d?.last_redo_label ? `重做：${d.last_redo_label} (Ctrl+Y)` : "重做 (Ctrl+Y)";
}

async function renderImportStep() {
  const d = appState.detail;
  const outRoot = d?.output_root || appState.settings?.output_root || "";
  el("scan-folder-label").textContent = d?.source_root || "请拖入或选择待整理的图片目录";
  el("scan-folder-badge").textContent = d
    ? d.scan_complete
      ? `已导入 ${d.counts.scanned} 张`
      : `部分扫描 ${d.counts.scanned} 张`
    : "未选择";
  el("scan-folder-badge").className = d ? "pill ok" : "pill";
  el("import-output-label").textContent = outRoot || "尚未设置输出目录";
  el<HTMLInputElement>("task-name-input").value = d?.task_name ?? "";
  el("btn-rescan").classList.toggle("hidden", !d);
  el("btn-save-as-template").classList.toggle("hidden", !d);

  if (d) {
    const ignored = d.ignored_count > 0 ? `（已忽略 ${d.ignored_count} 个非图片文件）` : "";
    const span = d.time_span ? ` · 拍摄时段 ${d.time_span[0]} ~ ${d.time_span[1]}` : "";
    el("scan-stats-text").textContent = `有效图片 ${d.counts.scanned} 张${ignored}${span}`;
    el("scan-summary-card").classList.remove("hidden");
    const timeWarn = d.counts.fallback_time_count + d.counts.time_uncertain_count;
    const warnWrap = el("scan-warning-wrap");
    if (timeWarn > 0) {
      warnWrap.classList.remove("hidden");
      el("scan-warning-line").textContent = `提示：有 ${timeWarn} 张图片缺少可靠 EXIF 时区或拍摄时间，已使用文件时间兜底。`;
      el("btn-review-time-anomaly").classList.remove("hidden");
    } else {
      warnWrap.classList.add("hidden");
    }
    el("scan-quality-bar").classList.remove("hidden");
    const qPill = el("scan-quality-pill");
    const qText = el("scan-quality-text");
    if (d.counts.quality_bad_count + d.counts.quality_warn_count > 0) {
      qPill.className = "pill warn";
      qPill.textContent = "发现画质提醒";
      qText.textContent = `异常 ${d.counts.quality_bad_count} 张 · 提示 ${d.counts.quality_warn_count} 张`;
    } else if (d.counts.quality_unchecked_count === 0) {
      qPill.className = "pill ok";
      qPill.textContent = "画质检测完成";
      qText.textContent = "未发现明显损坏或画质异常";
    } else {
      qPill.className = "pill";
      qPill.textContent = `待检 ${d.counts.quality_unchecked_count} 张`;
      qText.textContent = "可随时检测模糊/过暗/过曝图片（不拦截正常归档）";
    }
  } else {
    el("scan-summary-card").classList.add("hidden");
    el("scan-stats-text").textContent = "支持 JPG / PNG / BMP / WEBP / TIFF / HEIC / RAW 等常见格式";
  }

  let backendValid = true;
  let backendReason = "";
  if (d?.source_root || outRoot) {
    try {
      const res = await call<{ valid: boolean; reason: string }>("check_paths", {
        sourceRoot: d?.source_root ?? "",
        outputRoot: outRoot,
      });
      backendValid = res.valid;
      backendReason = res.reason;
    } catch {
      // keep client check
    }
  }
  const check = validateImportPaths({
    sourceRoot: d?.source_root ?? "",
    outputRoot: outRoot,
    backendValid,
    backendReason,
  });
  const banner = el("import-path-check");
  const nextBtn = el<HTMLButtonElement>("btn-goto-rules");
  if (!d) {
    banner.className = "path-check-banner info";
    el("import-path-check-title").textContent = "请先选择源图片文件夹和输出位置";
    el("import-path-check-detail").textContent = "拖入或点击选择文件夹即可自动扫描。";
    nextBtn.disabled = true;
  } else if (!check.ok) {
    banner.className = "path-check-banner error";
    el("import-path-check-title").textContent = "路径需要调整";
    el("import-path-check-detail").textContent = check.reason;
    nextBtn.disabled = false;
  } else {
    banner.className = "path-check-banner ok";
    el("import-path-check-title").textContent = "路径检查通过，可以进入下一步";
    el("import-path-check-detail").textContent = `源目录只读保护已就绪，输出将写入 ${outRoot}`;
    nextBtn.disabled = false;
  }
  el("import-footer-hint").textContent = d
    ? `已就绪 ${d.counts.scanned} 张图片，点击右侧进入分类与命名配置`
    : "请先选择源图片文件夹和归档输出目录";

  // Recovery & Drafts & Templates
  const recBox = el("recovery-banner");
  if (appState.recovery.length > 0) {
    const rec = appState.recovery[0];
    recBox.classList.remove("hidden");
    recBox.innerHTML = `<div class="row-flex" style="justify-content:space-between">
      <span>发现未完成归档作业（已复制 ${rec.succeeded} 张，剩余 ${rec.not_started + rec.in_progress} 张）</span>
      <button id="btn-resume-recovery" type="button" class="small primary">继续恢复执行</button>
    </div>`;
    el("btn-resume-recovery").onclick = () => resumeJob(rec.job_id);
  } else {
    recBox.classList.add("hidden");
  }

  const draftsBox = el("recent-drafts-list");
  if (appState.drafts.length === 0) {
    draftsBox.innerHTML = `<div class="muted">暂无未完成草稿，可从左侧导入图片开始新任务。</div>`;
  } else {
    draftsBox.innerHTML = "";
    for (const draft of appState.drafts.slice(0, 6)) {
      const row = document.createElement("div");
      row.className = "list-row-card";
      row.innerHTML = `<div class="col-flex" style="gap:2px;min-width:0">
        <div style="font-weight:700;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">${escapeHtml(draft.task_name || "未命名任务")}</div>
        <div class="muted mono" style="font-size:12px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">${escapeHtml(draft.source_root)} · ${draft.photo_count} 张</div>
      </div>`;
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = "small";
      btn.textContent = "打开";
      btn.onclick = () => openDraft(draft.id);
      row.appendChild(btn);
      draftsBox.appendChild(row);
    }
  }

  const tplBox = el("quick-templates-list");
  if (appState.templates.length === 0) {
    tplBox.innerHTML = `<div class="muted" style="font-size:12.5px">暂无已保存模板（完成配置后可保存复用）</div>`;
  } else {
    tplBox.innerHTML = "";
    for (const tpl of appState.templates.slice(0, 4)) {
      const row = document.createElement("div");
      row.className = "row-flex";
      row.style.justifyContent = "space-between";
      row.innerHTML = `<span style="font-size:13px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">${escapeHtml(tpl.name)}</span>`;
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = "small";
      btn.textContent = "套用";
      btn.disabled = !appState.detail;
      btn.onclick = () => applyTemplateById(tpl.id);
      row.appendChild(btn);
      tplBox.appendChild(row);
    }
  }
}

function readNamingFormValues(): NamingRuleDto {
  return {
    preset: el<HTMLSelectElement>("naming-preset-select").value,
    pattern: el<HTMLInputElement>("rule-pattern-input").value.trim() || "{date}_{group}_{seq}",
    prefix: el<HTMLInputElement>("rule-prefix").value,
    suffix: el<HTMLInputElement>("rule-suffix").value,
    date_format: el<HTMLSelectElement>("rule-date-format").value,
    include_group: el<HTMLInputElement>("chk-rule-group").checked,
    include_tag: el<HTMLInputElement>("chk-rule-tag").checked,
    include_original: el<HTMLInputElement>("chk-rule-orig").checked,
    seq_scope: el<HTMLSelectElement>("rule-seq-scope").value,
    seq_start: Math.max(1, Number(el<HTMLInputElement>("rule-seq-start").value) || 1),
    seq_digits: Math.max(1, Math.min(6, Number(el<HTMLSelectElement>("rule-seq-digits").value) || 3)),
    separator: el<HTMLSelectElement>("rule-separator").value,
  };
}

function updateLiveSamplePreview() {
  const d = appState.detail;
  const rule = readNamingFormValues();
  const sample = previewSamplePath({
    taskName: d?.task_name || "示例归档任务",
    dirLayout: d?.dir_layout || "task_and_group",
    dirPattern: el<HTMLInputElement>("custom-dir-input").value || d?.dir_pattern || "{task}/{group}",
    groupName: d?.groups[0]?.name || "风景",
    tagName: appState.tags[0]?.name || "精选",
    origName: "IMG_0001.jpg",
    rule,
  });
  const outPrefix = d?.output_root ? `${d.output_root}\\` : "输出目录\\";
  el("rules-live-sample").textContent = `${outPrefix}${sample}`;
  el("custom-pattern-row").classList.toggle("hidden", rule.preset !== "custom");
}

function renderRulesStep() {
  const d = appState.detail;
  if (!d) return;

  // Dir layout chips
  const dirGrid = el("dir-layout-grid");
  dirGrid.innerHTML = "";
  for (const opt of DIR_LAYOUT_OPTIONS) {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = `dir-chip${d.dir_layout === opt.key ? " active" : ""}`;
    btn.innerHTML = `<span class="dir-chip-title">${escapeHtml(opt.title)}</span><span class="dir-chip-desc">${escapeHtml(opt.desc)}</span>`;
    btn.onclick = async () => {
      const custom = el<HTMLInputElement>("custom-dir-input").value || opt.pattern;
      await applyDirLayout(opt.key, custom);
    };
    dirGrid.appendChild(btn);
  }
  el("custom-dir-row").classList.toggle("hidden", d.dir_layout !== "custom");
  el<HTMLInputElement>("custom-dir-input").value = d.dir_pattern;

  // Naming presets select
  const npSel = el<HTMLSelectElement>("naming-preset-select");
  if (npSel.options.length === 0) {
    for (const np of NAMING_PRESETS) {
      const opt = document.createElement("option");
      opt.value = np.key;
      opt.textContent = np.label;
      npSel.appendChild(opt);
    }
  }
  const r = d.naming_rule;
  npSel.value = r.preset;
  el<HTMLInputElement>("rule-prefix").value = r.prefix;
  el<HTMLInputElement>("rule-suffix").value = r.suffix;
  el<HTMLSelectElement>("rule-date-format").value = r.date_format;
  el<HTMLSelectElement>("rule-separator").value = r.separator;
  el<HTMLSelectElement>("rule-seq-scope").value = r.seq_scope;
  el<HTMLSelectElement>("rule-seq-digits").value = String(r.seq_digits);
  el<HTMLInputElement>("rule-seq-start").value = String(r.seq_start);
  el<HTMLInputElement>("chk-rule-group").checked = r.include_group;
  el<HTMLInputElement>("chk-rule-tag").checked = r.include_tag;
  el<HTMLInputElement>("chk-rule-orig").checked = r.include_original;
  el<HTMLInputElement>("rule-pattern-input").value = r.pattern;
  updateLiveSamplePreview();

  // Presets dropdown
  const presetSel = el<HTMLSelectElement>("rules-preset-select");
  presetSel.innerHTML = "";
  for (const p of appState.presets) {
    const opt = document.createElement("option");
    opt.value = p.id;
    opt.textContent = `${p.name}（${p.group_names.length} 类）`;
    presetSel.appendChild(opt);
  }

  // Manual target group dropdown
  const manualSel = el<HTMLSelectElement>("select-manual-target-group");
  manualSel.innerHTML = "";
  for (const g of d.groups) {
    const opt = document.createElement("option");
    opt.value = g.id;
    opt.textContent = `${g.name} (${g.count}张)`;
    manualSel.appendChild(opt);
  }

  el("rules-alloc-pill").textContent = `已归类 ${d.counts.grouped} / ${d.counts.scanned} 张`;
  el("rules-alloc-pill").className =
    d.dir_needs_group && d.counts.ungrouped > 0 ? "pill warn" : "pill ok";

  // Groups list
  const gList = el("groups-manage-list");
  if (d.groups.length === 0) {
    gList.innerHTML = `<div class="muted" style="padding:10px">尚未创建分类；可使用上方“按时间/按数量/按预设方案”一键生成，或手动添加分类。${
      !d.dir_needs_group ? "（当前为平铺/仅任务目录模式，也可直接进入下一步）" : ""
    }</div>`;
  } else {
    gList.innerHTML = "";
    d.groups.forEach((g, idx) => {
      const row = document.createElement("div");
      row.className = "group-row";
      const timeRange = g.start_time ? ` · ${g.start_time.slice(0, 10)}` : "";
      row.innerHTML = `<div class="row-flex grow" style="min-width:0">
        <span style="font-weight:700;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">${escapeHtml(g.name)}</span>
        <span class="pill info">${g.count} 张</span>
        <span class="muted" style="font-size:12.5px">${escapeHtml(timeRange)}</span>
      </div>`;
      const btns = document.createElement("div");
      btns.className = "row-flex";
      const up = document.createElement("button");
      up.type = "button";
      up.className = "small";
      up.textContent = "↑";
      up.disabled = idx === 0;
      up.onclick = () => reorderGroup(idx, idx - 1);
      const down = document.createElement("button");
      down.type = "button";
      down.className = "small";
      down.textContent = "↓";
      down.disabled = idx === d.groups.length - 1;
      down.onclick = () => reorderGroup(idx, idx + 1);
      const ren = document.createElement("button");
      ren.type = "button";
      ren.className = "small";
      ren.textContent = "改名";
      ren.onclick = () => promptRenameGroup(g);
      const del = document.createElement("button");
      del.type = "button";
      del.className = "small danger";
      del.textContent = "删除";
      del.onclick = () => deleteGroup(g.id);
      btns.append(up, down, ren, del);
      row.appendChild(btns);
      gList.appendChild(row);
    });
  }

  if (!el("grp-pane-manual").classList.contains("hidden")) {
    refreshManualGrid();
  }
}

async function refreshManualGrid() {
  const page = await call<PhotoPage>("photos", {
    groupId: null,
    query: null,
    onlyUnconfirmed: false,
    onlyQualityIssue: false,
    onlyTimeAnomaly: false,
    onlyExcluded: false,
    offset: 0,
    limit: 240,
  });
  appState.manualPhotos = page.items.filter((p) => !p.excluded);
  const grid = el("manual-thumb-grid");
  grid.innerHTML = "";
  for (const p of appState.manualPhotos) {
    const card = document.createElement("div");
    card.className = `manual-thumb-card${appState.manualSelectedIds.has(p.id) ? " selected" : ""}`;
    card.innerHTML = `<img alt="${escapeHtml(p.file_name)}" />
      <div style="overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-weight:600">${escapeHtml(p.file_name)}</div>
      <div class="muted" style="overflow:hidden;text-overflow:ellipsis;white-space:nowrap">${escapeHtml(p.group_name || "未分类")}</div>`;
    const img = card.querySelector("img")!;
    attachThumbImg(img, p.id, 140);
    card.onclick = (ev) => {
      if (ev.shiftKey && appState.manualAnchorId) {
        const ids = computeSelectionRange(appState.manualPhotos, appState.manualAnchorId, p.id);
        for (const id of ids) appState.manualSelectedIds.add(id);
      } else if (ev.ctrlKey || ev.metaKey) {
        if (appState.manualSelectedIds.has(p.id)) appState.manualSelectedIds.delete(p.id);
        else appState.manualSelectedIds.add(p.id);
        appState.manualAnchorId = p.id;
      } else {
        appState.manualSelectedIds.clear();
        appState.manualSelectedIds.add(p.id);
        appState.manualAnchorId = p.id;
      }
      el("manual-select-count").textContent = `已选 ${appState.manualSelectedIds.size} 张（支持 Ctrl/Shift 多选）`;
      for (const child of Array.from(grid.children)) {
        child.classList.remove("selected");
      }
      appState.manualPhotos.forEach((ph, idx) => {
        if (appState.manualSelectedIds.has(ph.id)) {
          grid.children[idx]?.classList.add("selected");
        }
      });
    };
    grid.appendChild(card);
  }
  el("manual-select-count").textContent = `已选 ${appState.manualSelectedIds.size} 张（支持 Ctrl/Shift 多选）`;
}

async function refreshPreviewStep() {
  const d = appState.detail;
  if (!d) return;
  el("preview-batch").classList.toggle("hidden", appState.previewMode !== "batch");
  el("preview-detail").classList.toggle("hidden", appState.previewMode !== "detail");

  // Populate group filter cards
  const navBox = el("preview-group-cards");
  navBox.innerHTML = "";

  const allCard = document.createElement("button");
  allCard.type = "button";
  allCard.className = `group-nav-card${appState.activeGroupId === null ? " active" : ""}`;
  allCard.innerHTML = `<div class="row-flex" style="justify-content:space-between"><span style="font-weight:700">全部图片</span><span class="pill info">${d.counts.scanned} 张</span></div>
    <div class="muted" style="font-size:12px">已就绪 ${d.counts.ready} · 未分类 ${d.counts.ungrouped}</div>`;
  allCard.onclick = () => {
    appState.activeGroupId = null;
    refreshPreviewPhotos();
  };
  navBox.appendChild(allCard);

  if (d.counts.ungrouped > 0) {
    const ugCard = document.createElement("button");
    ugCard.type = "button";
    ugCard.className = `group-nav-card${appState.activeGroupId === "__ungrouped__" ? " active" : ""}`;
    ugCard.innerHTML = `<div class="row-flex" style="justify-content:space-between"><span style="font-weight:700">未分类图片</span><span class="pill warn">${d.counts.ungrouped} 张</span></div>`;
    ugCard.onclick = () => {
      appState.activeGroupId = "__ungrouped__";
      refreshPreviewPhotos();
    };
    navBox.appendChild(ugCard);
  }

  for (const g of d.groups) {
    if (appState.previewFilter === "ungrouped") continue;
    if (appState.previewFilter === "review" && g.needs_review === 0) continue;
    if (appState.previewFilter === "ready" && (g.count === 0 || g.ready_count < g.count)) continue;
    const card = document.createElement("button");
    card.type = "button";
    card.className = `group-nav-card${appState.activeGroupId === g.id ? " active" : ""}`;
    const pillCls = g.needs_review > 0 ? "warn" : g.ready_count === g.count && g.count > 0 ? "ok" : "info";
    card.innerHTML = `<div class="row-flex" style="justify-content:space-between"><span style="font-weight:700;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">${escapeHtml(g.name)}</span><span class="pill ${pillCls}">${g.count} 张</span></div>
      <div class="muted" style="font-size:12px">${escapeHtml(g.status)}${g.start_time ? ` · ${g.start_time.slice(0, 10)}` : ""}</div>`;
    card.onclick = () => {
      appState.activeGroupId = g.id;
      refreshPreviewPhotos();
    };
    navBox.appendChild(card);
  }

  // Fill batch selectors
  const moveSel = el<HTMLSelectElement>("preview-batch-move-select");
  moveSel.innerHTML = "";
  for (const g of d.groups) {
    const opt = document.createElement("option");
    opt.value = g.id;
    opt.textContent = g.name;
    moveSel.appendChild(opt);
  }
  const tagSel = el<HTMLSelectElement>("preview-batch-tag-select");
  tagSel.innerHTML = `<option value="">(清除标签)</option>`;
  for (const t of appState.tags.filter((x) => x.enabled)) {
    const opt = document.createElement("option");
    opt.value = t.id;
    opt.textContent = t.name;
    tagSel.appendChild(opt);
  }

  await refreshPreviewPhotos();
}

async function refreshPreviewPhotos() {
  const d = appState.detail;
  if (!d) return;
  const q = el<HTMLInputElement>("preview-search-input").value.trim() || null;
  const onlyUnconfirmed = appState.previewFilter === "review";
  const page = await call<PhotoPage>("photos", {
    groupId: appState.activeGroupId,
    query: q,
    onlyUnconfirmed,
    onlyQualityIssue: false,
    onlyTimeAnomaly: false,
    onlyExcluded: false,
    offset: 0,
    limit: 300,
  });
  appState.previewPhotos = page.items;
  const activeGroup = d.groups.find((g) => g.id === appState.activeGroupId);
  el("preview-pane-title").textContent =
    appState.activeGroupId === "__ungrouped__"
      ? "未分类图片"
      : activeGroup
        ? `分类：${activeGroup.name}`
        : "全部图片目标预览";
  el("preview-pane-pill").textContent = `${page.total} 张`;

  // Highlight active card
  for (const btn of Array.from(el("preview-group-cards").children)) {
    btn.classList.remove("active");
  }

  const list = el("preview-photo-rows");
  if (page.items.length === 0) {
    list.innerHTML = `<div class="muted" style="padding:16px">当前筛选条件下没有图片。</div>`;
  } else {
    list.innerHTML = "";
    for (const p of page.items) {
      const row = document.createElement("div");
      row.className = `preview-photo-row${p.excluded ? " excluded" : ""}`;

      const cb = document.createElement("input");
      cb.type = "checkbox";
      cb.checked = appState.previewSelectedIds.has(p.id);
      cb.onchange = () => {
        if (cb.checked) appState.previewSelectedIds.add(p.id);
        else appState.previewSelectedIds.delete(p.id);
      };

      const thumb = document.createElement("div");
      thumb.className = "preview-thumb-box";
      thumb.title = "点击进入大图精修";
      const img = document.createElement("img");
      img.alt = p.file_name;
      attachThumbImg(img, p.id, 140);
      thumb.appendChild(img);
      thumb.onclick = () => openDetailForPhoto(p.id);

      const info = document.createElement("div");
      info.className = "col-flex";
      info.style.gap = "3px";
      const badge = p.excluded
        ? `<span class="pill danger">已排除</span>`
        : p.needs_review
          ? `<span class="pill warn">待核对</span>`
          : `<span class="pill ok">就绪</span>`;
      const tagBadge = p.tag_name ? `<span class="pill info">${escapeHtml(p.tag_name)}</span>` : "";
      const customBadge = p.custom_name ? `<span class="pill">自定义名</span>` : "";
      info.innerHTML = `<div class="row-flex" style="gap:6px;flex-wrap:wrap">
        <span style="font-weight:700">${escapeHtml(p.file_name)}</span>
        <span class="muted" style="font-size:12px">${escapeHtml(p.capture_text || "无拍摄时间")}</span>
        ${badge}${tagBadge}${customBadge}
      </div>
      <div class="mono" style="font-size:12.5px;color:var(--accent-strong);word-break:break-all">→ ${escapeHtml(p.preview_rel_path)}</div>`;

      const actions = document.createElement("div");
      actions.className = "row-flex";
      const upBtn = document.createElement("button");
      upBtn.type = "button";
      upBtn.className = "small";
      upBtn.textContent = "↑";
      upBtn.title = "上移顺序";
      upBtn.onclick = () => movePhotoOrder(p.id, -1);

      const downBtn = document.createElement("button");
      downBtn.type = "button";
      downBtn.className = "small";
      downBtn.textContent = "↓";
      downBtn.title = "下移顺序";
      downBtn.onclick = () => movePhotoOrder(p.id, 1);

      const editBtn = document.createElement("button");
      editBtn.type = "button";
      editBtn.className = "small";
      editBtn.textContent = "精修";
      editBtn.onclick = () => openDetailForPhoto(p.id);

      const exBtn = document.createElement("button");
      exBtn.type = "button";
      exBtn.className = `small${p.excluded ? "" : " danger"}`;
      exBtn.textContent = p.excluded ? "恢复" : "排除";
      exBtn.onclick = () => toggleExcludePhoto(p.id, !p.excluded);

      actions.append(upBtn, downBtn, editBtn, exBtn);
      row.append(cb, thumb, info, actions);
      list.appendChild(row);
    }
  }

  if (appState.previewMode === "detail") {
    renderDetailStage();
  }
}

function openDetailForPhoto(photoId: string) {
  appState.previewMode = "detail";
  appState.detailPhotoId = photoId;
  appState.viewerRotation = 0;
  el("preview-batch").classList.add("hidden");
  el("preview-detail").classList.remove("hidden");
  renderDetailStage();
}

function renderDetailStage() {
  const d = appState.detail;
  if (!d) return;
  const photos = appState.previewPhotos;
  if (photos.length === 0) {
    el("stage-photo-name").textContent = "当前列表无图片";
    el("viewer-img").classList.add("hidden");
    el("viewer-empty").classList.remove("hidden");
    return;
  }
  let cur = photos.find((p) => p.id === appState.detailPhotoId) ?? photos[0];
  appState.detailPhotoId = cur.id;

  // Populate group filter select in detail toolbar
  const grpSel = el<HTMLSelectElement>("detail-group-select");
  grpSel.innerHTML = `<option value="">全部图片</option>`;
  for (const g of d.groups) {
    const opt = document.createElement("option");
    opt.value = g.id;
    opt.textContent = `${g.name} (${g.count})`;
    grpSel.appendChild(opt);
  }
  grpSel.value = appState.activeGroupId && appState.activeGroupId !== "__ungrouped__" ? appState.activeGroupId : "";
  el("detail-group-stats").textContent = `第 ${photos.indexOf(cur) + 1} / ${photos.length} 张`;

  el("photo-single-stage").classList.toggle("hidden", appState.detailSubMode !== "single");
  el("photo-grid-stage").classList.toggle("hidden", appState.detailSubMode !== "grid");
  el("btn-view-single").classList.toggle("active", appState.detailSubMode === "single");
  el("btn-view-grid").classList.toggle("active", appState.detailSubMode === "grid");

  // Single stage
  el("stage-photo-name").textContent = cur.file_name;
  el("stage-photo-time").textContent = cur.capture_text || "无拍摄时间";
  el("stage-target-path").textContent = `目标路径：${cur.preview_rel_path}`;
  el("stage-order-note").textContent = cur.order_note || "";
  el("stage-review-pill").classList.toggle("hidden", !cur.needs_review);
  el("btn-ack-review").classList.toggle("hidden", !cur.needs_review);
  el("btn-exclude-cur").textContent = cur.excluded ? "恢复此图" : "排除此图";

  const qPill = el("stage-quality-pill");
  if (cur.quality.status === "bad" || cur.quality.status === "warn") {
    qPill.className = `pill ${cur.quality.status === "bad" ? "danger" : "warn"}`;
    qPill.textContent = cur.quality.summary || "画质提示";
    qPill.classList.remove("hidden");
  } else {
    qPill.classList.add("hidden");
  }

  const viewerImg = el<HTMLImageElement>("viewer-img");
  const viewerEmpty = el("viewer-empty");
  const canvas = el("viewer-canvas");
  canvas.classList.toggle("actual", appState.viewerZoomActual);
  viewerImg.style.transform = `rotate(${appState.viewerRotation}deg)`;
  loadThumb(cur.id, 1024).then((url) => {
    if (appState.detailPhotoId !== cur.id) return;
    if (url) {
      viewerImg.src = url;
      viewerImg.classList.remove("hidden");
      viewerEmpty.classList.add("hidden");
    } else {
      viewerImg.classList.add("hidden");
      viewerEmpty.classList.remove("hidden");
    }
  });

  // Filmstrip
  const strip = el("filmstrip-box");
  strip.innerHTML = "";
  for (const p of photos.slice(0, 120)) {
    const item = document.createElement("div");
    item.className = `filmstrip-item${p.id === cur.id ? " active" : ""}`;
    const im = document.createElement("img");
    im.alt = p.file_name;
    attachThumbImg(im, p.id, 120);
    item.appendChild(im);
    item.onclick = () => {
      appState.detailPhotoId = p.id;
      appState.viewerRotation = 0;
      renderDetailStage();
    };
    strip.appendChild(item);
  }

  // Grid mode
  if (appState.detailSubMode === "grid") {
    const gBox = el("photo-grid-box");
    gBox.innerHTML = "";
    for (const p of photos) {
      const card = document.createElement("div");
      card.className = `grid-photo-card${appState.detailSelectedIds.has(p.id) ? " selected" : ""}`;
      card.innerHTML = `<img alt="${escapeHtml(p.file_name)}" />
        <div style="font-size:12px;font-weight:700;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">${escapeHtml(p.file_name)}</div>
        <div class="muted" style="font-size:11.5px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">${escapeHtml(p.preview_stem)}</div>`;
      attachThumbImg(card.querySelector("img")!, p.id, 160);
      card.onclick = (ev) => {
        if (ev.shiftKey && appState.detailAnchorId) {
          for (const id of computeSelectionRange(photos, appState.detailAnchorId, p.id)) {
            appState.detailSelectedIds.add(id);
          }
        } else if (ev.ctrlKey || ev.metaKey) {
          if (appState.detailSelectedIds.has(p.id)) appState.detailSelectedIds.delete(p.id);
          else appState.detailSelectedIds.add(p.id);
          appState.detailAnchorId = p.id;
        } else {
          appState.detailSelectedIds.clear();
          appState.detailSelectedIds.add(p.id);
          appState.detailAnchorId = p.id;
        }
        renderDetailStage();
      };
      gBox.appendChild(card);
    }
    el("grid-select-info").textContent = `已选 ${appState.detailSelectedIds.size} 张（按住 Ctrl 点选、Shift 连选）`;
  }

  // Right panel: group select, custom name, tag palette
  const assignGrp = el<HTMLSelectElement>("detail-assign-group");
  assignGrp.innerHTML = `<option value="">（未分类）</option>`;
  for (const g of d.groups) {
    const opt = document.createElement("option");
    opt.value = g.id;
    opt.textContent = g.name;
    assignGrp.appendChild(opt);
  }
  assignGrp.value = cur.group_id ?? "";
  el<HTMLInputElement>("detail-custom-name").value = cur.custom_name ?? "";
  renderTagPalette(cur);
}

function renderTagPalette(cur: PhotoView) {
  const q = el<HTMLInputElement>("palette-search").value.trim().toLowerCase();
  const list = el("palette-list");
  list.innerHTML = "";
  const enabledTags = appState.tags.filter((t) => {
    if (!t.enabled) return false;
    if (!q) return true;
    return (
      t.name.toLowerCase().includes(q) ||
      t.short_label.toLowerCase().includes(q) ||
      t.aliases.some((a) => a.toLowerCase().includes(q))
    );
  });
  enabledTags.forEach((t, idx) => {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "palette-item";
    if (cur.tag_id === t.id) btn.classList.add("primary");
    const shortcut = idx < 9 ? `<span class="pill">${idx + 1}</span>` : "";
    btn.innerHTML = `<span>${escapeHtml(t.name)}</span>${shortcut}`;
    btn.onclick = () => assignTagToPhoto(cur.id, t.id);
    list.appendChild(btn);
  });
}

async function refreshArchivePlan() {
  const d = appState.detail;
  if (!d) return;
  el("arch-source-display").textContent = d.source_root;
  el<HTMLInputElement>("output-root-input").value = d.output_root || "";
  el<HTMLInputElement>("chk-strict-hash").checked = d.strict_hash;

  const plan = await call<ExecutionPlan>("build_plan");
  appState.plan = plan;

  const partial = el<HTMLInputElement>("chk-partial-archive").checked;
  const skipExisting = el<HTMLInputElement>("chk-skip-existing").checked;
  const skipIds = new Set<string>();
  for (const iss of plan.issues) {
    if (!iss.blocks || !iss.photo_id) continue;
    if (partial && (iss.code === "W003" || iss.code === "W004")) skipIds.add(iss.photo_id);
    if (skipExisting && iss.code === "E002" && iss.message.startsWith("目标已存在")) {
      skipIds.add(iss.photo_id);
    }
  }
  const hardBlocks = plan.issues.filter((iss) => {
    if (!iss.blocks) return false;
    if (iss.photo_id && skipIds.has(iss.photo_id)) return false;
    return true;
  });
  const readyCount = plan.items.filter((it) => !skipIds.has(it.photo_id)).length;

  el("arch-stat-ready").textContent = String(readyCount);
  el("arch-stat-issues").textContent = String(hardBlocks.length);
  el("arch-stat-disk").textContent =
    plan.free_bytes != null ? `${formatBytes(plan.free_bytes)} / ${formatBytes(plan.bytes)}` : formatBytes(plan.bytes);

  const statusPill = el("archive-board-status");
  const execBtn = el<HTMLButtonElement>("btn-execute-archive");
  if (hardBlocks.length > 0 || readyCount === 0) {
    statusPill.className = "pill danger";
    statusPill.textContent = `有 ${hardBlocks.length} 项阻断问题`;
    execBtn.disabled = true;
    el("archive-footer-hint").textContent = "请先解决左侧阻断性问题，或勾选“仅归档已就绪部分/跳过已存在”";
  } else {
    statusPill.className = "pill ok";
    statusPill.textContent = "检查通过，可安全归档";
    execBtn.disabled = false;
    el("archive-footer-hint").textContent = `准备复制 ${readyCount} 张图片到 ${d.output_root}`;
  }

  // Board
  const board = archiveBoard(plan.issues, readyCount);
  const boardEl = el("archive-board");
  boardEl.innerHTML = "";
  for (const bName of ARCHIVE_BUCKETS) {
    const rows = board[bName];
    if (rows.length === 0) continue;
    const card = document.createElement("div");
    card.className = "bucket-card";
    const colorCls = bName === "冲突" ? "danger" : bName === "未完成" || bName === "时间异常" ? "warn" : "ok";
    card.innerHTML = `<div class="row-flex" style="justify-content:space-between">
      <span style="font-weight:700">${bName}</span>
      <span class="pill ${colorCls}">${rows.length}</span>
    </div>`;
    for (const iss of rows.slice(0, 30)) {
      const line = document.createElement("div");
      line.style.fontSize = "13px";
      line.textContent = iss.message;
      card.appendChild(line);
    }
    if (rows.length > 30) {
      const more = document.createElement("div");
      more.className = "muted";
      more.style.fontSize = "12px";
      more.textContent = `另有 ${rows.length - 30} 条…`;
      card.appendChild(more);
    }
    boardEl.appendChild(card);
  }

  // Target preview / last report
  renderArchiveResultOrPreview(skipIds);
}

function renderArchiveResultOrPreview(skipIds: Set<string>) {
  const box = el("archive-result-box");
  if (appState.lastReport && appState.lastReport.job_id) {
    const rep = appState.lastReport;
    el("arch-preview-count-pill").textContent = `完成 ${rep.succeeded} 张`;
    box.innerHTML = `<div class="card" style="background:var(--ok-soft);border-color:#a9dfc0;gap:8px;display:flex;flex-direction:column">
      <div style="font-weight:800;font-size:15.5px;color:var(--ok)">归档作业已完成（状态：${escapeHtml(rep.status)}）</div>
      <div>成功复制：<b>${rep.succeeded}</b> 张 · 跳过：<b>${rep.skipped}</b> 张 · 失败：<b>${rep.failed}</b> 张</div>
      <div class="row-flex" style="flex-wrap:wrap">
        <button id="btn-rep-open-dir" type="button" class="small primary">打开输出文件夹</button>
        <button id="btn-rep-export-csv" type="button" class="small">导出 CSV 清单</button>
        <button id="btn-rep-export-json" type="button" class="small">导出 JSON 清单</button>
        <button id="btn-rep-undo" type="button" class="small danger">撤销本次归档</button>
        <button id="btn-rep-back-preview" type="button" class="small">返回预览模式</button>
      </div>
    </div>`;
    el("btn-rep-open-dir").onclick = () => call("open_output_dir", { jobId: rep.job_id });
    el("btn-rep-export-csv").onclick = () => exportJobFile(rep.job_id, "csv");
    el("btn-rep-export-json").onclick = () => exportJobFile(rep.job_id, "json");
    el("btn-rep-undo").onclick = () => previewAndUndoJob(rep.job_id);
    el("btn-rep-back-preview").onclick = () => {
      appState.lastReport = null;
      refreshArchivePlan();
    };
    return;
  }

  const plan = appState.plan;
  if (!plan) return;
  const q = el<HTMLInputElement>("arch-preview-search").value.trim().toLowerCase();
  const onlyAbnormal = el<HTMLInputElement>("chk-arch-preview-abnormal").checked;
  const filtered = plan.items.filter((it) => {
    const isSkip = skipIds.has(it.photo_id);
    if (onlyAbnormal && !isSkip && !it.blocked) return false;
    if (!q) return true;
    return (
      it.source_name.toLowerCase().includes(q) ||
      it.group_name.toLowerCase().includes(q) ||
      it.relative_path.toLowerCase().includes(q)
    );
  });
  el("arch-preview-count-pill").textContent = `${filtered.length} 条映射`;
  const groups = buildArchivePreviewGroups(filtered);
  if (groups.length === 0) {
    box.innerHTML = `<div class="muted" style="padding:14px">没有可预览的映射项。</div>`;
    return;
  }
  box.innerHTML = "";
  for (const grp of groups) {
    const wrap = document.createElement("div");
    wrap.className = "preview-group-box";
    const collapsed = appState.collapsedGroups.has(grp.groupName);
    const head = document.createElement("div");
    head.className = "preview-group-head";
    head.innerHTML = `<span>${escapeHtml(grp.groupName)}</span><span class="pill info">${grp.items.length} 张 ${collapsed ? "▸" : "▾"}</span>`;
    head.onclick = () => {
      if (collapsed) appState.collapsedGroups.delete(grp.groupName);
      else appState.collapsedGroups.add(grp.groupName);
      renderArchiveResultOrPreview(skipIds);
    };
    wrap.appendChild(head);
    if (!collapsed) {
      const rows = document.createElement("div");
      rows.className = "preview-group-rows";
      for (const it of grp.items.slice(0, 120)) {
        const line = document.createElement("div");
        line.className = "row-flex";
        line.style.justifyContent = "space-between";
        line.innerHTML = `<span class="muted" style="flex-shrink:0">${escapeHtml(it.source_name)}</span>
          <span class="mono" style="overflow:hidden;text-overflow:ellipsis;white-space:nowrap">→ ${escapeHtml(it.relative_path)}</span>`;
        rows.appendChild(line);
      }
      if (grp.items.length > 120) {
        const more = document.createElement("div");
        more.className = "muted";
        more.textContent = `另有 ${grp.items.length - 120} 张（导出 CSV 可查看全部）`;
        rows.appendChild(more);
      }
      wrap.appendChild(rows);
    }
    box.appendChild(wrap);
  }
}

// Actions
async function scanFolder(path: string, replace = true) {
  appState.scanning = true;
  el("scan-progress-box").classList.remove("hidden");
  el("scan-progress-fill").style.width = "10%";
  el("scan-progress-text").textContent = "正在扫描图片...";
  try {
    const recursive = el<HTMLInputElement>("chk-recursive").checked;
    const detail = await call<TaskDetail>("scan_folder", { path, recursive, replace });
    if (replace) {
      resetTaskUiState();
      appState.thumbCache.clear();
    } else {
      appState.lastReport = null;
      appState.plan = null;
    }
    appState.detail = detail;
    appState.draftReopened = false;
    appState.drafts = await call<DraftSummary[]>("list_drafts");
    await renderImportStep();
    renderTopbar();
    showStatus(`扫描完成：共导入 ${detail.counts.scanned} 张有效图片`, "ok");
  } catch (err) {
    showStatus(plainError(err), "error");
  } finally {
    appState.scanning = false;
    el("scan-progress-box").classList.add("hidden");
  }
}

async function openDraft(id: string) {
  try {
    const detail = await call<TaskDetail>("open_draft", { id });
    resetTaskUiState();
    appState.detail = detail;
    appState.draftReopened = true;
    await renderImportStep();
    renderTopbar();
    showStatus("已恢复任务草稿", "ok");
  } catch (err) {
    showStatus(plainError(err), "error");
  }
}

async function applyDirLayout(layout: DirLayoutKey, customPattern: string) {
  try {
    appState.detail = await call<TaskDetail>("set_dir_layout", { layout, customPattern });
    renderRulesStep();
    renderTopbar();
  } catch (err) {
    showStatus(plainError(err), "error");
  }
}

async function applyNamingRuleFromForm() {
  try {
    const rule = readNamingFormValues();
    appState.detail = await call<TaskDetail>("set_naming_rule", { rule });
    renderRulesStep();
    renderTopbar();
    showStatus("命名规则已更新", "ok");
  } catch (err) {
    showStatus(plainError(err), "error");
  }
}

async function reorderGroup(fromIndex: number, toIndex: number) {
  try {
    appState.detail = await call<TaskDetail>("reorder_group_to", { fromIndex, toIndex });
    renderRulesStep();
    renderTopbar();
  } catch (err) {
    showStatus(plainError(err), "error");
  }
}

async function deleteGroup(groupId: string) {
  try {
    appState.detail = await call<TaskDetail>("delete_group", { groupId });
    renderRulesStep();
    renderTopbar();
  } catch (err) {
    showStatus(plainError(err), "error");
  }
}

function promptRenameGroup(g: GroupView) {
  const input = document.createElement("input");
  input.value = g.name;
  input.style.width = "100%";
  showModal({
    title: `重命名分类「${g.name}」`,
    bodyNode: input,
    buttons: [
      { label: "取消", onClick: (d) => d.close() },
      {
        label: "保存",
        primary: true,
        onClick: (dlg) =>
          runAction(async () => {
            const val = input.value.trim();
            const errMsg = validateComponentName(val, "分类名称");
            if (errMsg) {
              showStatus(errMsg, "error");
              return;
            }
            appState.detail = await call<TaskDetail>("rename_group", { groupId: g.id, newName: val });
            dlg.close();
            renderRulesStep();
            renderTopbar();
          }),
      },
    ],
  });
}

async function movePhotoOrder(photoId: string, delta: number) {
  try {
    appState.detail = await call<TaskDetail>("move_photo", { photoId, delta });
    await refreshPreviewPhotos();
    renderTopbar();
  } catch (err) {
    showStatus(plainError(err), "error");
  }
}

async function toggleExcludePhoto(photoId: string, excluded: boolean) {
  try {
    appState.detail = await call<TaskDetail>("exclude_photo", { photoId, excluded });
    await refreshPreviewStep();
    renderTopbar();
  } catch (err) {
    showStatus(plainError(err), "error");
  }
}

async function assignTagToPhoto(photoId: string, tagId: string | null) {
  try {
    appState.detail = await call<TaskDetail>("assign_tag", { photoId, tagId });
    await refreshPreviewStep();
    renderTopbar();
  } catch (err) {
    showStatus(plainError(err), "error");
  }
}

async function executeArchive() {
  if (!appState.plan || appState.copying) return;
  const partial = el<HTMLInputElement>("chk-partial-archive").checked;
  const skipExisting = el<HTMLInputElement>("chk-skip-existing").checked;
  const confirmedSkips: string[] = [];
  for (const iss of appState.plan.issues) {
    if (!iss.blocks || !iss.photo_id) continue;
    if (partial && (iss.code === "W003" || iss.code === "W004")) confirmedSkips.push(iss.photo_id);
    if (skipExisting && iss.code === "E002" && iss.message.startsWith("目标已存在")) {
      confirmedSkips.push(iss.photo_id);
    }
  }
  appState.copying = true;
  el("copy-progress-card").classList.remove("hidden");
  el<HTMLButtonElement>("btn-execute-archive").disabled = true;
  try {
    const report = await call<ExecuteReport>("execute_plan", {
      planId: appState.plan.id,
      confirmedSkips,
    });
    appState.lastReport = report;
    appState.detail = await call<TaskDetail>("task_detail");
    await refreshArchivePlan();
    renderTopbar();
    if (report.status === "success") {
      showStatus(`归档成功：已安全复制 ${report.succeeded} 张图片`, "ok");
      if (appState.settings?.open_folder_after) {
        await call("open_output_dir", { jobId: report.job_id });
      }
    } else {
      showStatus(`归档结束（${report.status}）：成功 ${report.succeeded}，失败 ${report.failed}`, "warn");
    }
  } catch (err) {
    showStatus(plainError(err), "error");
  } finally {
    appState.copying = false;
    el("copy-progress-card").classList.add("hidden");
    el<HTMLButtonElement>("btn-execute-archive").disabled = false;
  }
}

async function resumeJob(jobId: string) {
  try {
    el("copy-progress-card").classList.remove("hidden");
    const report = await call<ExecuteReport>("retry_job", { jobId });
    appState.recovery = await call<RecoveryView[]>("recovery");
    showStatus(`恢复执行完成：成功 ${report.succeeded}，失败 ${report.failed}`, report.failed > 0 ? "warn" : "ok");
    await renderImportStep();
  } catch (err) {
    showStatus(plainError(err), "error");
  } finally {
    el("copy-progress-card").classList.add("hidden");
  }
}

async function exportJobFile(jobId: string, kind: "csv" | "json") {
  try {
    const dir = await call<string>("suggest_export_dir");
    const target = await tauriDialog.save({
      defaultPath: `${dir}\\归档清单_${jobId}.${kind}`,
      filters: [{ name: kind.toUpperCase(), extensions: [kind] }],
    });
    if (!target) return;
    await call("export_job_file", { jobId, path: target, kind });
    showStatus(`已导出清单到 ${target}`, "ok");
  } catch (err) {
    showStatus(plainError(err), "error");
  }
}

async function previewAndUndoJob(jobId: string) {
  try {
    const preview = await call<UndoPreview>("undo_preview", { jobId });
    const msg = `将安全删除本次创建的副本：${preview.will_delete.length} 个\n将保留并跳过（已被修改或缺少凭据）：${preview.will_refuse.length} 个\n\n原图不会被触碰。确认执行撤销？`;
    if (!(await showConfirmModal("撤销归档确认", msg, "确认撤销本次副本", true))) return;
    const report = await call<{ deleted: string[]; refused: string[]; removed_dirs: string[] }>("undo_job", { jobId });
    showStatus(`已撤销删除 ${report.deleted.length} 个副本，清理空目录 ${report.removed_dirs.length} 个，保留 ${report.refused.length} 个`, "ok");
    appState.lastReport = null;
    if (appState.view === "history") renderHistoryView();
    if (appState.view === "archive") refreshArchivePlan();
  } catch (err) {
    showStatus(plainError(err), "error");
  }
}

async function applyTemplateById(id: string) {
  try {
    const detail = await call<TaskDetail>("apply_template", { id });
    resetTaskUiState();
    appState.detail = detail;
    renderTopbar();
    switchView("rules");
    showStatus("已套用任务模板", "ok");
  } catch (err) {
    showStatus(plainError(err), "error");
  }
}

// Tool views
async function renderPresetsView() {
  appState.presets = await call<GroupPreset[]>("list_presets");
  const q = el<HTMLInputElement>("preset-search-input").value.trim().toLowerCase();
  const filtered = appState.presets.filter(
    (p) =>
      !q ||
      p.name.toLowerCase().includes(q) ||
      p.scene_type.toLowerCase().includes(q) ||
      p.group_names.some((g) => g.toLowerCase().includes(q)),
  );
  el("preset-count-pill").textContent = `${filtered.length} 套`;
  const list = el("presets-manage-list");
  list.innerHTML = "";
  for (const p of filtered) {
    const card = document.createElement("div");
    card.className = "list-row-card";
    card.innerHTML = `<div class="col-flex" style="gap:3px;min-width:0">
      <div class="row-flex"><span style="font-weight:700">${escapeHtml(p.name)}</span><span class="pill info">${escapeHtml(p.scene_type)}</span><span class="pill">${p.source === "example" ? "通用示例" : "自定义"}</span></div>
      <div class="muted" style="font-size:12.5px">分类：${escapeHtml(p.group_names.join(" / "))}</div>
    </div>`;
    const btns = document.createElement("div");
    btns.className = "row-flex";
    const dup = document.createElement("button");
    dup.type = "button";
    dup.className = "small";
    dup.textContent = "复制";
    dup.onclick = async () => {
      await call("duplicate_preset", { id: p.id, newName: null });
      renderPresetsView();
    };
    const del = document.createElement("button");
    del.type = "button";
    del.className = "small danger";
    del.textContent = "删除";
    del.onclick = async () => {
      await call("delete_preset", { id: p.id });
      renderPresetsView();
    };
    btns.append(dup, del);
    card.appendChild(btns);
    list.appendChild(card);
  }
}

async function renderTagsView() {
  appState.tags = await call<TagItem[]>("tags");
  const q = el<HTMLInputElement>("tag-search-input").value.trim().toLowerCase();
  const filtered = appState.tags.filter(
    (t) =>
      !q ||
      t.name.toLowerCase().includes(q) ||
      t.category.toLowerCase().includes(q) ||
      t.aliases.some((a) => a.toLowerCase().includes(q)),
  );
  el("tag-count-pill").textContent = `${filtered.length} 条`;
  const list = el("tags-manage-list");
  list.innerHTML = "";
  for (const t of filtered) {
    const row = document.createElement("div");
    row.className = "list-row-card";
    row.innerHTML = `<div class="row-flex"><span style="font-weight:700">${escapeHtml(t.name)}</span><span class="pill">${escapeHtml(t.category)}</span></div>`;
    const btns = document.createElement("div");
    btns.className = "row-flex";
    const toggle = document.createElement("button");
    toggle.type = "button";
    toggle.className = "small";
    toggle.textContent = t.enabled ? "已启用" : "已停用";
    toggle.onclick = async () => {
      await call("tag_set_enabled", { id: t.id, enabled: !t.enabled });
      renderTagsView();
    };
    const del = document.createElement("button");
    del.type = "button";
    del.className = "small danger";
    del.textContent = "删除";
    del.onclick = async () => {
      await call("tag_delete", { id: t.id });
      renderTagsView();
    };
    btns.append(toggle, del);
    row.appendChild(btns);
    list.appendChild(row);
  }
}

async function renderTemplatesView() {
  appState.templates = await call<TaskTemplate[]>("list_templates");
  const list = el("templates-manage-list");
  if (appState.templates.length === 0) {
    list.innerHTML = `<div class="muted" style="padding:14px">暂无已保存的任务模板。</div>`;
    return;
  }
  list.innerHTML = "";
  for (const tpl of appState.templates) {
    const card = document.createElement("div");
    card.className = "list-row-card";
    card.innerHTML = `<div class="col-flex" style="gap:2px">
      <div style="font-weight:700">${escapeHtml(tpl.name)}</div>
      <div class="muted" style="font-size:12.5px">层级：${escapeHtml(tpl.dir_pattern || "平铺")} · 规则：${escapeHtml(tpl.naming_rule.pattern)} · 分类数：${tpl.group_names.length}</div>
    </div>`;
    const btns = document.createElement("div");
    btns.className = "row-flex";
    const applyBtn = document.createElement("button");
    applyBtn.type = "button";
    applyBtn.className = "small primary";
    applyBtn.textContent = "套用到当前任务";
    applyBtn.disabled = !appState.detail;
    applyBtn.onclick = () => applyTemplateById(tpl.id);
    const delBtn = document.createElement("button");
    delBtn.type = "button";
    delBtn.className = "small danger";
    delBtn.textContent = "删除";
    delBtn.onclick = async () => {
      await call("delete_template", { id: tpl.id });
      renderTemplatesView();
    };
    btns.append(applyBtn, delBtn);
    card.appendChild(btns);
    list.appendChild(card);
  }
}

async function renderHistoryView() {
  const [drafts, hist] = await Promise.all([
    call<DraftSummary[]>("list_drafts"),
    call<HistoryRow[]>("history"),
  ]);
  appState.drafts = drafts;
  const list = el("history-manage-list");
  list.innerHTML = "";
  for (const row of hist) {
    const card = document.createElement("div");
    card.className = "list-row-card";
    card.innerHTML = `<div class="col-flex" style="gap:2px;min-width:0">
      <div class="row-flex"><span style="font-weight:700">${escapeHtml(row.task_name || "归档作业")}</span><span class="pill">${escapeHtml(row.status)}</span><span class="muted" style="font-size:12px">${escapeHtml(row.created_at)}</span></div>
      <div class="muted mono" style="font-size:12px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">${escapeHtml(row.output_root)} · ${row.photo_count} 张</div>
    </div>`;
    const btns = document.createElement("div");
    btns.className = "row-flex";
    const openDir = document.createElement("button");
    openDir.type = "button";
    openDir.className = "small";
    openDir.textContent = "打开目录";
    openDir.onclick = () => runAction(() => call("open_output_dir", { jobId: row.job_id }).then(() => undefined));
    const csvBtn = document.createElement("button");
    csvBtn.type = "button";
    csvBtn.className = "small";
    csvBtn.textContent = "导出CSV";
    csvBtn.onclick = () => exportJobFile(row.job_id, "csv");
    const undoBtn = document.createElement("button");
    undoBtn.type = "button";
    undoBtn.className = "small danger";
    undoBtn.textContent = "安全撤销";
    undoBtn.disabled = row.status === "undone";
    undoBtn.onclick = () => previewAndUndoJob(row.job_id);
    btns.append(openDir, csvBtn, undoBtn);
    card.appendChild(btns);
    list.appendChild(card);
  }
  if (hist.length === 0) {
    list.innerHTML = `<div class="muted" style="padding:14px">暂无归档历史记录。</div>`;
  }
}

async function renderSettingsView() {
  const s = await call<SettingsView>("settings");
  appState.settings = s;
  el<HTMLInputElement>("setting-output-root").value = s.output_root;
  el<HTMLInputElement>("setting-open-after").checked = s.open_folder_after;
  el<HTMLInputElement>("setting-retention").value = String(s.retention_days);
  el("cache-note-text").textContent = CACHE_CLEAR_NOTE;
}

function openHelpDialog() {
  const stepKey: StepId | "manage" =
    appState.view === "import" || appState.view === "rules" || appState.view === "preview" || appState.view === "archive"
      ? appState.view
      : "manage";
  const content = contextualHelpContent(stepKey);
  showModal({
    title: content.title,
    bodyHtml: `<ul style="margin:0;padding-left:20px;display:flex;flex-direction:column;gap:8px">${content.bullets
      .map((b) => `<li>${escapeHtml(b)}</li>`)
      .join("")}</ul>`,
    buttons: [{ label: "知道了", primary: true, onClick: (d) => d.close() }],
  });
}

function wireEvents() {
  // Topbar
  for (const st of ["import", "rules", "preview", "archive"] as StepId[]) {
    el(`step-btn-${st}`).onclick = () => switchView(st);
  }
  el("btn-top-help").onclick = openHelpDialog;
  el("btn-undo").onclick = async () => {
    try {
      appState.detail = await call<TaskDetail>("undo_edit");
      renderTopbar();
      if (appState.view === "rules") renderRulesStep();
      if (appState.view === "preview") refreshPreviewStep();
    } catch (err) {
      showStatus(plainError(err), "error");
    }
  };
  el("btn-redo").onclick = async () => {
    try {
      appState.detail = await call<TaskDetail>("redo_edit");
      renderTopbar();
      if (appState.view === "rules") renderRulesStep();
      if (appState.view === "preview") refreshPreviewStep();
    } catch (err) {
      showStatus(plainError(err), "error");
    }
  };
  el("btn-manage").onclick = () => {
    const menu = el("manage-menu");
    const open = menu.classList.toggle("hidden");
    el("btn-manage").setAttribute("aria-expanded", String(!open));
  };
  document.addEventListener("click", (e) => {
    const target = e.target as HTMLElement;
    if (!target.closest(".manage-wrap")) {
      el("manage-menu").classList.add("hidden");
      el("btn-manage").setAttribute("aria-expanded", "false");
    }
  });
  for (const id of ["presets", "tags", "templates", "history", "settings"] as ViewId[]) {
    el(`nav-${id}`).onclick = () => switchView(id);
  }
  document.querySelectorAll("[data-back-work]").forEach((btn) => {
    (btn as HTMLElement).onclick = () => switchView(appState.lastWorkStep);
  });
  el("status-close").onclick = hideStatus;
  el("dialog-close").onclick = () => el<HTMLDialogElement>("app-dialog").close();

  // Step 1
  el("btn-pick-folder").onclick = async () => {
    const picked = await tauriDialog.open({ directory: true, multiple: false, title: "选择源图片文件夹" });
    if (typeof picked === "string" && picked) await scanFolder(picked, true);
  };
  el("btn-rescan").onclick = async () => {
    if (appState.detail) await scanFolder(appState.detail.source_root, false);
  };
  el("btn-cancel-scan").onclick = () => call("cancel_scan");
  el("btn-import-pick-output").onclick = async () => {
    const picked = await tauriDialog.open({ directory: true, multiple: false, title: "选择归档输出目录" });
    if (typeof picked === "string" && picked) {
      const updated = await call<TaskDetail | null>("set_output_root", { path: picked });
      if (updated) appState.detail = updated;
      appState.settings = await call<SettingsView>("settings");
      await renderImportStep();
    }
  };
  el<HTMLInputElement>("task-name-input").onchange = async () => {
    if (!appState.detail) return;
    try {
      appState.detail = await call<TaskDetail>("set_task_name", {
        name: el<HTMLInputElement>("task-name-input").value,
      });
      renderTopbar();
    } catch (err) {
      showStatus(plainError(err), "error");
    }
  };
  el("btn-inspect-quality").onclick = async () => {
    el("quality-progress-box").classList.remove("hidden");
    el("btn-cancel-quality").classList.remove("hidden");
    try {
      await call("inspect_quality");
      appState.detail = await call<TaskDetail>("task_detail");
      await renderImportStep();
      showStatus("全片画质检测完成", "ok");
    } catch (err) {
      showStatus(plainError(err), "error");
    } finally {
      el("quality-progress-box").classList.add("hidden");
      el("btn-cancel-quality").classList.add("hidden");
    }
  };
  el("btn-cancel-quality").onclick = () => call("cancel_quality");
  el("btn-review-time-anomaly").onclick = () => {
    appState.previewFilter = "review";
    switchView("preview");
  };
  el("btn-open-history").onclick = () => switchView("history");
  el("btn-open-templates").onclick = () => switchView("templates");
  el("btn-save-as-template").onclick = () => {
    const inp = document.createElement("input");
    inp.placeholder = "输入模板名称（如 旅行相册常用配置）";
    inp.style.width = "100%";
    showModal({
      title: "保存当前任务配置为模板",
      bodyNode: inp,
      buttons: [
        { label: "取消", onClick: (d) => d.close() },
        {
          label: "保存模板",
          primary: true,
          onClick: (d) =>
            runAction(async () => {
              await call("save_template", { name: inp.value });
              appState.templates = await call<TaskTemplate[]>("list_templates");
              d.close();
              await renderImportStep();
              showStatus("已保存为可复用模板", "ok");
            }),
        },
      ],
    });
  };
  el("btn-goto-rules").onclick = () => switchView("rules");

  // Step 2
  el("btn-apply-custom-dir").onclick = () => {
    applyDirLayout("custom", el<HTMLInputElement>("custom-dir-input").value);
  };
  el("naming-preset-select").onchange = () => {
    const presetKey = el<HTMLSelectElement>("naming-preset-select").value;
    const found = NAMING_PRESETS.find((p) => p.key === presetKey);
    if (found && presetKey !== "custom") {
      el<HTMLInputElement>("rule-pattern-input").value = found.pattern;
    }
    updateLiveSamplePreview();
  };
  for (const id of [
    "rule-prefix",
    "rule-suffix",
    "rule-date-format",
    "rule-separator",
    "rule-seq-scope",
    "rule-seq-digits",
    "rule-seq-start",
    "chk-rule-group",
    "chk-rule-tag",
    "chk-rule-orig",
    "rule-pattern-input",
  ]) {
    el(id).addEventListener("input", updateLiveSamplePreview);
    el(id).addEventListener("change", updateLiveSamplePreview);
  }
  el("btn-apply-naming-rule").onclick = applyNamingRuleFromForm;

  document.querySelectorAll("[data-grp-mode]").forEach((btn) => {
    (btn as HTMLButtonElement).onclick = () => {
      const mode = (btn as HTMLElement).dataset.grpMode!;
      document.querySelectorAll("[data-grp-mode]").forEach((b) => b.classList.toggle("active", b === btn));
      for (const m of ["time", "count", "preset", "manual"]) {
        el(`grp-pane-${m}`).classList.toggle("hidden", m !== mode);
      }
      if (mode === "manual") refreshManualGrid();
    };
  });

  el("btn-grp-by-day").onclick = () =>
    runAction(async () => {
      appState.detail = await call<TaskDetail>("group_by_time", { granularity: "day" });
      renderRulesStep();
      renderTopbar();
      showStatus("已按拍摄日期自动分组", "ok");
    });
  el("btn-grp-by-month").onclick = () =>
    runAction(async () => {
      appState.detail = await call<TaskDetail>("group_by_time", { granularity: "month" });
      renderRulesStep();
      renderTopbar();
      showStatus("已按拍摄月份自动分组", "ok");
    });
  el("btn-grp-by-gap").onclick = () =>
    runAction(async () => {
      const mins = Math.max(1, Number(el<HTMLInputElement>("input-gap-minutes").value) || 60);
      appState.detail = await call<TaskDetail>("group_by_time_gap", { gapMinutes: mins, prefix: "批次" });
      renderRulesStep();
      renderTopbar();
      showStatus(`已按 ${mins} 分钟间隔自动分批`, "ok");
    });
  el("btn-grp-fixed-size").onclick = () =>
    runAction(async () => {
      const size = Math.max(1, Number(el<HTMLInputElement>("input-fixed-size").value) || 10);
      const prefix = el<HTMLInputElement>("input-fixed-prefix").value.trim() || "分组";
      appState.detail = await call<TaskDetail>("group_by_fixed_size", { perGroup: size, prefix });
      renderRulesStep();
      renderTopbar();
      showStatus(`已按每组 ${size} 张生成分组`, "ok");
    });
  el("btn-grp-even").onclick = async () => {
    try {
      appState.detail = await call<TaskDetail>("group_evenly");
      renderRulesStep();
      renderTopbar();
      showStatus("已均匀分配到各分类", "ok");
    } catch (err) {
      showStatus(plainError(err), "error");
    }
  };
  el("btn-grp-paste-counts").onclick = () => {
    const d = appState.detail;
    if (!d || d.groups.length === 0) {
      showStatus("请先创建至少一个分类", "warn");
      return;
    }
    const ta = document.createElement("textarea");
    ta.rows = 3;
    ta.placeholder = `输入 ${d.groups.length} 个数量（空格或逗号分隔），如：5 10 8`;
    showModal({
      title: `指定 ${d.groups.length} 个分类的分配数量`,
      bodyNode: ta,
      buttons: [
        { label: "取消", onClick: (dlg) => dlg.close() },
        {
          label: "应用分配",
          primary: true,
          onClick: (dlg) =>
            runAction(async () => {
              const parsed = parseCountsPaste(ta.value, d.groups.length);
              if (!parsed.ok) {
                showStatus(parsed.error, "error");
                return;
              }
              appState.detail = await call<TaskDetail>("set_quantities", { counts: parsed.counts });
              dlg.close();
              renderRulesStep();
              renderTopbar();
            }),
        },
      ],
    });
  };
  el("btn-apply-preset-groups").onclick = () =>
    runAction(async () => {
      const presetId = el<HTMLSelectElement>("rules-preset-select").value;
      if (!presetId) return;
      const distributeEvenly = el<HTMLInputElement>("chk-preset-even").checked;
      const applyNaming = el<HTMLInputElement>("chk-preset-naming").checked;
      appState.detail = await call<TaskDetail>("apply_preset", { presetId, distributeEvenly, applyNaming });
      renderRulesStep();
      renderTopbar();
      showStatus("已套用预设分类方案", "ok");
    });
  el("btn-manual-assign-new").onclick = async () => {
    const name = el<HTMLInputElement>("input-manual-group-name").value.trim();
    const ids = Array.from(appState.manualSelectedIds);
    if (!name || ids.length === 0) {
      showStatus("请先输入分类名称并选中图片", "warn");
      return;
    }
    try {
      appState.detail = await call<TaskDetail>("create_group_for_photos", { groupName: name, photoIds: ids });
      appState.manualSelectedIds.clear();
      renderRulesStep();
      renderTopbar();
      showStatus(`已将 ${ids.length} 张图片归入「${name}」`, "ok");
    } catch (err) {
      showStatus(plainError(err), "error");
    }
  };
  el("btn-manual-assign-existing").onclick = () =>
    runAction(async () => {
      const groupId = el<HTMLSelectElement>("select-manual-target-group").value;
      const ids = Array.from(appState.manualSelectedIds);
      if (!groupId || ids.length === 0) {
        showStatus("请选择目标分类并选中图片", "warn");
        return;
      }
      appState.detail = await call<TaskDetail>("assign_photos_to_group", { photoIds: ids, groupId });
      appState.manualSelectedIds.clear();
      renderRulesStep();
      renderTopbar();
    });
  el("btn-add-group").onclick = async () => {
    const inp = el<HTMLInputElement>("input-add-group");
    try {
      appState.detail = await call<TaskDetail>("add_group", { name: inp.value });
      inp.value = "";
      renderRulesStep();
      renderTopbar();
    } catch (err) {
      showStatus(plainError(err), "error");
    }
  };
  el("btn-batch-edit-groups").onclick = () => {
    const ta = document.createElement("textarea");
    ta.rows = 6;
    ta.value = (appState.detail?.groups ?? []).map((g) => g.name).join("\n");
    ta.placeholder = "每行一个分类名称（或逗号分隔），如：\n沿途风景\n人物合影\n美食打卡";
    showModal({
      title: "批量录入或更新分类清单",
      bodyNode: ta,
      buttons: [
        { label: "取消", onClick: (d) => d.close() },
        {
          label: "应用分类清单",
          primary: true,
          onClick: (d) =>
            runAction(async () => {
              const parsed = parseGroupBatchText(ta.value);
              if (!parsed.ok) {
                showStatus(parsed.error, "error");
                return;
              }
              appState.detail = await call<TaskDetail>("set_groups_text", { text: ta.value });
              d.close();
              renderRulesStep();
              renderTopbar();
            }),
        },
      ],
    });
  };
  el("btn-sort-groups").onclick = () =>
    runAction(async () => {
      appState.detail = await call<TaskDetail>("sort_groups_by_name");
      renderRulesStep();
      renderTopbar();
    });
  el("btn-back-to-import").onclick = () => switchView("import");
  el("btn-goto-preview").onclick = () => switchView("preview");

  // Step 3
  document.querySelectorAll("[data-grp-filter]").forEach((btn) => {
    (btn as HTMLButtonElement).onclick = () => {
      appState.previewFilter = (btn as HTMLElement).dataset.grpFilter as typeof appState.previewFilter;
      document.querySelectorAll("[data-grp-filter]").forEach((b) => b.classList.toggle("active", b === btn));
      refreshPreviewStep();
    };
  });
  el("preview-search-input").addEventListener("input", () => refreshPreviewPhotos());
  el("btn-preview-confirm-all").onclick = () =>
    runAction(async () => {
      appState.detail = await call<TaskDetail>("confirm_all_ready");
      await refreshPreviewStep();
      renderTopbar();
      showStatus("已确认所有就绪图片", "ok");
    });
  el("btn-preview-sort-time").onclick = () =>
    runAction(async () => {
      appState.detail = await call<TaskDetail>("resort_by_mtime");
      await refreshPreviewStep();
    });
  el("btn-preview-sort-name").onclick = () =>
    runAction(async () => {
      appState.detail = await call<TaskDetail>("resort_by_filename");
      await refreshPreviewStep();
    });
  el("btn-preview-save-scheme").onclick = () => {
    const d = appState.detail;
    if (!d || d.groups.length === 0) {
      showStatus("当前任务尚无分类可保存", "warn");
      return;
    }
    const inp = document.createElement("input");
    inp.value = `${d.task_name || "自定义"}分类方案`;
    inp.style.width = "100%";
    showModal({
      title: "将当前分类与命名规则存为预设方案",
      bodyNode: inp,
      buttons: [
        { label: "取消", onClick: (dlg) => dlg.close() },
        {
          label: "保存方案",
          primary: true,
          onClick: (dlg) =>
            runAction(async () => {
              await call("save_preset", {
                id: null,
                name: inp.value,
                description: "从归档任务保存的通用分类方案",
                sceneType: "自定义场景",
                groupNames: d.groups.map((g) => g.name),
                defaultNaming: d.naming_rule,
              });
              appState.presets = await call<GroupPreset[]>("list_presets");
              dlg.close();
              showStatus("已保存到分类方案库", "ok");
            }),
        },
      ],
    });
  };
  el("btn-preview-select-all").onclick = () => {
    if (appState.previewSelectedIds.size === appState.previewPhotos.length) {
      appState.previewSelectedIds.clear();
    } else {
      for (const p of appState.previewPhotos) appState.previewSelectedIds.add(p.id);
    }
    refreshPreviewPhotos();
  };
  el("btn-preview-batch-move").onclick = () =>
    runAction(async () => {
      const ids = Array.from(appState.previewSelectedIds);
      const gid = el<HTMLSelectElement>("preview-batch-move-select").value;
      if (ids.length === 0 || !gid) return;
      appState.detail = await call<TaskDetail>("assign_photos_to_group", { photoIds: ids, groupId: gid });
      await refreshPreviewStep();
      renderTopbar();
    });
  el("btn-preview-batch-tag").onclick = () =>
    runAction(async () => {
      const ids = Array.from(appState.previewSelectedIds);
      const tid = el<HTMLSelectElement>("preview-batch-tag-select").value || null;
      if (ids.length === 0) return;
      appState.detail = await call<TaskDetail>("batch_assign_tag", { photoIds: ids, tagId: tid });
      await refreshPreviewStep();
      renderTopbar();
    });
  el("btn-preview-batch-exclude").onclick = () =>
    runAction(async () => {
      const ids = Array.from(appState.previewSelectedIds);
      if (ids.length === 0) return;
      appState.detail = await call<TaskDetail>("batch_exclude_photos", { photoIds: ids, excluded: true });
      appState.previewSelectedIds.clear();
      await refreshPreviewStep();
      renderTopbar();
    });
  el("btn-open-detail-mode").onclick = () => {
    appState.previewMode = "detail";
    el("preview-batch").classList.add("hidden");
    el("preview-detail").classList.remove("hidden");
    renderDetailStage();
  };
  el("btn-back-to-preview-batch").onclick = () => {
    appState.previewMode = "batch";
    refreshPreviewStep();
  };
  el("btn-detail-back-batch").onclick = () => {
    appState.previewMode = "batch";
    refreshPreviewStep();
  };
  el("btn-view-single").onclick = () => {
    appState.detailSubMode = "single";
    renderDetailStage();
  };
  el("btn-view-grid").onclick = () => {
    appState.detailSubMode = "grid";
    renderDetailStage();
  };
  el("detail-group-select").onchange = () => {
    const v = el<HTMLSelectElement>("detail-group-select").value;
    appState.activeGroupId = v || null;
    refreshPreviewPhotos();
  };
  el("btn-detail-prev").onclick = () => stepDetailPhoto(-1);
  el("btn-detail-next").onclick = () => stepDetailPhoto(1);
  el("btn-zoom-fit").onclick = () => {
    appState.viewerZoomActual = false;
    el("viewer-canvas").classList.remove("actual");
  };
  el("btn-zoom-actual").onclick = () => {
    appState.viewerZoomActual = true;
    el("viewer-canvas").classList.add("actual");
  };
  el("btn-rotate").onclick = () => {
    appState.viewerRotation = (appState.viewerRotation + 90) % 360;
    el<HTMLImageElement>("viewer-img").style.transform = `rotate(${appState.viewerRotation}deg)`;
  };
  el("btn-open-original").onclick = () => {
    if (appState.detailPhotoId) {
      runAction(() => call("open_source_file", { photoId: appState.detailPhotoId }).then(() => undefined));
    }
  };
  el("btn-exclude-cur").onclick = () => {
    const cur = appState.previewPhotos.find((p) => p.id === appState.detailPhotoId);
    if (cur) toggleExcludePhoto(cur.id, !cur.excluded);
  };
  el("btn-ack-review").onclick = () =>
    runAction(async () => {
      if (!appState.detailPhotoId) return;
      appState.detail = await call<TaskDetail>("confirm_photo", { photoId: appState.detailPhotoId });
      await refreshPreviewStep();
      renderTopbar();
    });
  el("detail-assign-group").onchange = () =>
    runAction(async () => {
      const gid = el<HTMLSelectElement>("detail-assign-group").value;
      if (gid && appState.detailPhotoId) {
        appState.detail = await call<TaskDetail>("assign_photos_to_group", {
          photoIds: [appState.detailPhotoId],
          groupId: gid,
        });
        await refreshPreviewStep();
        renderTopbar();
      }
    });
  el("btn-save-custom-name").onclick = async () => {
    if (!appState.detailPhotoId) return;
    try {
      appState.detail = await call<TaskDetail>("set_custom_name", {
        photoId: appState.detailPhotoId,
        customName: el<HTMLInputElement>("detail-custom-name").value || null,
      });
      await refreshPreviewStep();
      renderTopbar();
    } catch (err) {
      showStatus(plainError(err), "error");
    }
  };
  el("btn-clear-tag").onclick = () => {
    if (appState.detailPhotoId) assignTagToPhoto(appState.detailPhotoId, null);
  };
  el("palette-search").addEventListener("input", () => {
    const cur = appState.previewPhotos.find((p) => p.id === appState.detailPhotoId);
    if (cur) renderTagPalette(cur);
  });
  el("btn-multi-confirm").onclick = () =>
    runAction(async () => {
      const ids = Array.from(appState.detailSelectedIds);
      if (ids.length === 0) return;
      appState.detail = await call<TaskDetail>("batch_confirm_photos", { photoIds: ids });
      await refreshPreviewStep();
    });
  el("btn-multi-exclude").onclick = () =>
    runAction(async () => {
      const ids = Array.from(appState.detailSelectedIds);
      if (ids.length === 0) return;
      appState.detail = await call<TaskDetail>("batch_exclude_photos", { photoIds: ids, excluded: true });
      await refreshPreviewStep();
    });
  el("btn-multi-restore").onclick = () =>
    runAction(async () => {
      const ids = Array.from(appState.detailSelectedIds);
      if (ids.length === 0) return;
      appState.detail = await call<TaskDetail>("batch_exclude_photos", { photoIds: ids, excluded: false });
      await refreshPreviewStep();
    });

  el("btn-back-to-rules").onclick = () => switchView("rules");
  el("btn-goto-archive").onclick = () => switchView("archive");
  el("btn-detail-goto-archive").onclick = () => switchView("archive");

  // Step 4
  el("btn-back-to-preview").onclick = () => switchView("preview");
  el("btn-rebuild-plan").onclick = () => refreshArchivePlan();
  el("chk-partial-archive").onchange = () => refreshArchivePlan();
  el("chk-skip-existing").onchange = () => refreshArchivePlan();
  el("chk-strict-hash").onchange = () =>
    runAction(async () => {
      appState.detail = await call<TaskDetail>("set_strict", {
        strict: el<HTMLInputElement>("chk-strict-hash").checked,
      });
      refreshArchivePlan();
    });
  el("btn-pick-output").onclick = () =>
    runAction(async () => {
      const picked = await tauriDialog.open({ directory: true, multiple: false, title: "更改归档输出目录" });
      if (typeof picked === "string" && picked) {
        appState.detail = await call<TaskDetail>("set_output_root", { path: picked });
        refreshArchivePlan();
      }
    });
  el("btn-arch-inspect-quality").onclick = () =>
    runAction(async () => {
      await call("inspect_quality");
      appState.detail = await call<TaskDetail>("task_detail");
      refreshArchivePlan();
      showStatus("画质检测完成", "ok");
    });
  el("arch-preview-search").addEventListener("input", () => refreshArchivePlan());
  el("chk-arch-preview-abnormal").onchange = () => refreshArchivePlan();
  el("btn-arch-toggle-all-secs").onclick = () => {
    const groups = appState.plan ? buildArchivePreviewGroups(appState.plan.items) : [];
    if (appState.collapsedGroups.size >= groups.length) {
      appState.collapsedGroups.clear();
    } else {
      for (const g of groups) appState.collapsedGroups.add(g.groupName);
    }
    refreshArchivePlan();
  };
  el("btn-arch-export-plan").onclick = () =>
    runAction(async () => {
      if (!appState.plan) return;
      const dir = await call<string>("suggest_export_dir");
      const path = await tauriDialog.save({
        defaultPath: `${dir}\\预期归档清单.csv`,
        filters: [{ name: "CSV", extensions: ["csv"] }],
      });
      if (!path) return;
      await call("export_plan_csv_file", { confirmedSkips: [], path });
      showStatus(`已导出预期清单：${path}`, "ok");
    });
  el("btn-execute-archive").onclick = executeArchive;
  el("btn-cancel-copy").onclick = () => call("cancel_copy");

  // Manage views
  el("preset-search-input").addEventListener("input", renderPresetsView);
  el("btn-preset-new").onclick = () => {
    const box = document.createElement("div");
    box.className = "col-flex";
    box.innerHTML = `<input id="np-name" placeholder="方案名称" />
      <input id="np-scene" placeholder="场景分类（如 旅行/商品/办公）" />
      <textarea id="np-groups" rows="4" placeholder="预设分类列表（每行一个）"></textarea>`;
    showModal({
      title: "新建分类方案",
      bodyNode: box,
      buttons: [
        { label: "取消", onClick: (d) => d.close() },
        {
          label: "保存",
          primary: true,
          onClick: (d) =>
            runAction(async () => {
              const name = (box.querySelector("#np-name") as HTMLInputElement).value.trim();
              const scene = (box.querySelector("#np-scene") as HTMLInputElement).value.trim();
              const raw = (box.querySelector("#np-groups") as HTMLTextAreaElement).value;
              const parsed = parseGroupBatchText(raw);
              if (!parsed.ok) {
                showStatus(parsed.error, "error");
                return;
              }
              await call("save_preset", {
                id: null,
                name,
                description: "用户自定义分类方案",
                sceneType: scene || "自定义",
                groupNames: parsed.names,
                defaultNaming: null,
              });
              d.close();
              renderPresetsView();
            }),
        },
      ],
    });
  };
  el("btn-preset-import").onclick = () =>
    runAction(async () => {
      const p = await tauriDialog.open({ filters: [{ name: "JSON", extensions: ["json"] }] });
      if (typeof p === "string" && p) {
        const rep = await call<{ added: number; updated: number }>("import_presets_file", { path: p });
        showStatus(`导入方案完成：新增 ${rep.added}，更新 ${rep.updated}`, "ok");
        renderPresetsView();
      }
    });
  el("btn-preset-export").onclick = () =>
    runAction(async () => {
      const dir = await call<string>("suggest_export_dir");
      const p = await tauriDialog.save({ defaultPath: `${dir}\\图序归档_分类方案.json` });
      if (p) {
        await call("export_presets_file", { path: p });
        showStatus(`已导出方案库到 ${p}`, "ok");
      }
    });

  el("tag-search-input").addEventListener("input", renderTagsView);
  el("btn-tag-add").onclick = () => {
    const inp = document.createElement("input");
    inp.placeholder = "输入新标签名称（如 精选成片）";
    inp.style.width = "100%";
    showModal({
      title: "新增标签",
      bodyNode: inp,
      buttons: [
        { label: "取消", onClick: (d) => d.close() },
        {
          label: "创建",
          primary: true,
          onClick: (d) =>
            runAction(async () => {
              await call("tag_create", { name: inp.value, shortLabel: inp.value, aliases: [] });
              d.close();
              renderTagsView();
            }),
        },
      ],
    });
  };
  el("btn-tag-import").onclick = () =>
    runAction(async () => {
      const p = await tauriDialog.open({ filters: [{ name: "标签文件", extensions: ["json", "txt"] }] });
      if (typeof p === "string" && p) {
        const rep = await call<{ added: number; updated: number }>("import_tags_file", { path: p });
        showStatus(`导入标签完成：新增 ${rep.added}，更新 ${rep.updated}`, "ok");
        renderTagsView();
      }
    });
  el("btn-tag-export").onclick = () =>
    runAction(async () => {
      const dir = await call<string>("suggest_export_dir");
      const p = await tauriDialog.save({ defaultPath: `${dir}\\图序归档_标签库.json` });
      if (p) {
        await call("export_tags_file", { path: p });
        showStatus(`已导出标签库到 ${p}`, "ok");
      }
    });
  el("btn-tpl-save-current").onclick = () => el("btn-save-as-template").click();

  // Settings
  el("btn-setting-pick-out").onclick = () =>
    runAction(async () => {
      const p = await tauriDialog.open({ directory: true });
      if (typeof p === "string" && p) {
        await call("set_output_root", { path: p });
        renderSettingsView();
      }
    });
  el("btn-setting-clear-out").onclick = () =>
    runAction(async () => {
      await call("clear_remembered_output");
      renderSettingsView();
    });
  el("setting-open-after").onchange = () =>
    runAction(async () => {
      await call("set_open_folder_after", {
        enabled: el<HTMLInputElement>("setting-open-after").checked,
      });
      renderSettingsView();
    });
  el("setting-retention").onchange = () =>
    runAction(async () => {
      const days = Number(el<HTMLInputElement>("setting-retention").value) || 90;
      await call("set_retention_days", { days });
    });
  el("btn-setting-cleanup").onclick = () =>
    runAction(async () => {
      const rep = await call<{ note: string }>("cleanup_retention", { confirmed: true });
      showStatus(rep.note, "ok");
    });
  el("btn-clear-thumb-cache").onclick = () =>
    runAction(async () => {
      const rep = await call<{ note: string }>("clear_thumbnail_cache");
      appState.thumbCache.clear();
      showStatus(rep.note, "ok");
    });
  el("btn-run-diagnostics").onclick = () =>
    runAction(async () => {
      const diag = await call<Record<string, unknown>>("diagnostics");
      const box = el("diagnostics-box");
      box.classList.remove("hidden");
      box.textContent = JSON.stringify(diag, null, 2);
    });
  el("btn-clear-history").onclick = () =>
    runAction(async () => {
      if (await showConfirmModal("清空历史记录", `${CACHE_CLEAR_NOTE}\n清空后将无法在软件内撤销历史作业，确认继续？`, "确认清空", true)) {
        const rep = await call<{ note: string }>("clear_history");
        appState.drafts = [];
        showStatus(rep.note, "ok");
      }
    });

  // Keyboard shortcuts
  document.addEventListener("keydown", (e) => {
    if (el<HTMLDialogElement>("app-dialog").open) return;
    const target = e.target as HTMLElement | null;
    const isTyping =
      Boolean(target) &&
      (target!.tagName === "INPUT" ||
        target!.tagName === "TEXTAREA" ||
        target!.tagName === "SELECT" ||
        target!.isContentEditable);
    const act = shortcutAction({
      key: e.key,
      ctrlKey: e.ctrlKey,
      metaKey: e.metaKey,
      shiftKey: e.shiftKey,
      altKey: e.altKey,
      isTyping,
    });
    if (!act) return;
    if (act.type === "help") {
      e.preventDefault();
      openHelpDialog();
    } else if (act.type === "undo") {
      e.preventDefault();
      el("btn-undo").click();
    } else if (act.type === "redo") {
      e.preventDefault();
      el("btn-redo").click();
    } else if (appState.view === "preview" && appState.previewMode === "detail") {
      if (act.type === "prev_photo") {
        e.preventDefault();
        stepDetailPhoto(-1);
      } else if (act.type === "next_photo") {
        e.preventDefault();
        stepDetailPhoto(1);
      } else if (act.type === "exclude_photo") {
        e.preventDefault();
        el("btn-exclude-cur").click();
      } else if (act.type === "confirm_photo") {
        e.preventDefault();
        el("btn-ack-review").click();
      } else if (act.type === "tag_slot") {
        const enabled = appState.tags.filter((t) => t.enabled);
        const tag = enabled[act.index];
        if (tag && appState.detailPhotoId) assignTagToPhoto(appState.detailPhotoId, tag.id);
      }
    }
  });
}

function stepDetailPhoto(delta: number) {
  const photos = appState.previewPhotos;
  if (photos.length === 0) return;
  const idx = photos.findIndex((p) => p.id === appState.detailPhotoId);
  const next = Math.max(0, Math.min(photos.length - 1, (idx < 0 ? 0 : idx) + delta));
  appState.detailPhotoId = photos[next].id;
  appState.viewerRotation = 0;
  renderDetailStage();
}

async function init() {
  wireEvents();

  // Drag & drop folder via Tauri webview
  try {
    const webview = getCurrentWebview();
    await webview.onDragDropEvent((event) => {
      const dz = el("drop-zone");
      if (event.payload.type === "enter" || event.payload.type === "over") {
        dz.classList.add("dragover");
      } else if (event.payload.type === "leave") {
        dz.classList.remove("dragover");
      } else if (event.payload.type === "drop") {
        dz.classList.remove("dragover");
        const first = event.payload.paths?.[0];
        if (first && appState.view === "import" && !appState.scanning) {
          scanFolder(first, true);
        }
      }
    });
  } catch {
    // non-tauri preview
  }

  try {
    await tauriListen<{ discovered: number; ignored: number; current_file: string }>("scan-progress", (e) => {
      el("scan-progress-text").textContent = `已发现 ${e.payload.discovered} 张（${e.payload.current_file}）`;
    });
    await tauriListen<{ processed: number; total: number; current_file: string }>("quality-progress", (e) => {
      const pct = e.payload.total > 0 ? Math.round((e.payload.processed / e.payload.total) * 100) : 0;
      el("quality-progress-fill").style.width = `${pct}%`;
      el("quality-progress-text").textContent = `正在检测画质 ${e.payload.processed} / ${e.payload.total}`;
    });
    await tauriListen<{ file_name: string; succeeded: number; failed: number; skipped: number; remaining: number }>(
      "copy-progress",
      (e) => {
        const done = e.payload.succeeded + e.payload.failed + e.payload.skipped;
        const total = done + e.payload.remaining;
        const pct = total > 0 ? Math.round((done / total) * 100) : 0;
        el("copy-progress-fill").style.width = `${pct}%`;
        el("copy-progress-text").textContent = `正在安全复制 (${done}/${total})：${e.payload.file_name}`;
      },
    );
  } catch {
    // non-tauri preview
  }

  try {
    const boot = await call<{
      drafts: DraftSummary[];
      recovery: RecoveryView[];
      tags: TagItem[];
      presets: GroupPreset[];
    }>("bootstrap");
    appState.drafts = boot.drafts;
    appState.recovery = boot.recovery;
    appState.tags = boot.tags;
    appState.presets = boot.presets;
    appState.settings = await call<SettingsView>("settings");
    appState.templates = await call<TaskTemplate[]>("list_templates");
    await renderImportStep();
    renderTopbar();
  } catch (err) {
    showStatus(`初始化失败：${plainError(err)}`, "error");
  }
}

void formatArchivePlanCsv;
init();
