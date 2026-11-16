/** Pure workflow rules, naming preview, and validation helpers for 图序归档 (TuXu Archive). No DOM and no Tauri. */

export type StepId = "import" | "rules" | "preview" | "archive";

export const STEP_LABELS: Record<StepId, string> = {
  import: "导入与位置",
  rules: "分类与命名",
  preview: "预览与调整",
  archive: "核对与归档",
};

export const ARCHIVE_BUCKETS = ["已完成", "未完成", "冲突", "时间异常"] as const;
export type ArchiveBucketName = (typeof ARCHIVE_BUCKETS)[number];

export const CACHE_CLEAR_NOTE = "清除缓存或历史不会删除原图，也不会删除已经归档的文件。";

export type NamingRuleDto = {
  preset: string;
  pattern: string;
  prefix: string;
  suffix: string;
  date_format: string;
  include_group: boolean;
  include_tag: boolean;
  include_original: boolean;
  seq_scope: string;
  seq_start: number;
  seq_digits: number;
  separator: string;
};

export type DirLayoutKey =
  | "task_and_group"
  | "group_only"
  | "task_only"
  | "date_and_group"
  | "flat"
  | "custom";

export const DIR_LAYOUT_OPTIONS: Array<{
  key: DirLayoutKey;
  title: string;
  pattern: string;
  desc: string;
}> = [
  { key: "task_and_group", title: "任务 / 分类 / 文件", pattern: "{task}/{group}", desc: "最常用两级目录结构" },
  { key: "group_only", title: "分类 / 文件", pattern: "{group}", desc: "直接按分类建子文件夹" },
  { key: "date_and_group", title: "月份 / 分类 / 文件", pattern: "{month}/{group}", desc: "按年月自动归类" },
  { key: "task_only", title: "任务 / 文件", pattern: "{task}", desc: "所有图片放入同一任务目录" },
  { key: "flat", title: "平铺输出", pattern: "", desc: "直接输出到目标根目录" },
  { key: "custom", title: "自定义层级", pattern: "{task}/{group}", desc: "自由组合路径占位符" },
];

export const NAMING_PRESETS: Array<{
  key: string;
  label: string;
  pattern: string;
  desc: string;
}> = [
  { key: "date_group_seq", label: "日期 + 分类 + 连续编号", pattern: "{date}_{group}_{seq}", desc: "适合相册与日常整理" },
  { key: "group_seq", label: "分类 + 连续编号", pattern: "{group}_{seq}", desc: "简洁清晰、支持同分类多图" },
  { key: "prefix_seq", label: "自定义前缀 + 连续编号", pattern: "{prefix}{seq}", desc: "适合统一流水号输出" },
  { key: "orig_group", label: "保留原文件名 + 分类", pattern: "{orig}_{group}", desc: "保留原图编号便于追溯" },
  { key: "tag_seq", label: "标签名 + 连续编号", pattern: "{tag}_{seq}", desc: "按单张标签精细命名" },
  { key: "custom", label: "高级自定义模板", pattern: "{date}_{group}_{seq}", desc: "自由编辑占位符表达式" },
];

export type VirtualWindow = {
  start: number;
  end: number;
  topSpacer: number;
  bottomSpacer: number;
  totalHeight: number;
};

export type VirtualGridWindow = VirtualWindow & {
  cols: number;
};

export function computeVirtualWindow(params: {
  total: number;
  itemHeight: number;
  viewportHeight: number;
  scrollTop: number;
  overscan?: number;
}): VirtualWindow {
  const total = Math.max(0, Math.floor(params.total));
  const stride = Math.max(1, params.itemHeight);
  const totalHeight = total * stride;
  if (total === 0) {
    return { start: 0, end: 0, topSpacer: 0, bottomSpacer: 0, totalHeight: 0 };
  }
  const viewport = Math.max(stride, params.viewportHeight || stride * 6);
  const overscan = Math.max(0, Math.floor(params.overscan ?? 4));
  const scrollTop = Math.max(0, params.scrollTop);
  const rawStart = Math.floor(scrollTop / stride) - overscan;
  const rawEnd = Math.ceil((scrollTop + viewport) / stride) + overscan;
  const start = Math.max(0, Math.min(total - 1, rawStart));
  const end = Math.max(start + 1, Math.min(total, rawEnd));
  const topSpacer = start * stride;
  const bottomSpacer = Math.max(0, (total - end) * stride);
  return { start, end, topSpacer, bottomSpacer, totalHeight };
}

export function computeVirtualGridWindow(params: {
  total: number;
  containerWidth: number;
  minColWidth: number;
  rowHeight: number;
  gap?: number;
  viewportHeight: number;
  scrollTop: number;
  overscanRows?: number;
}): VirtualGridWindow {
  const total = Math.max(0, Math.floor(params.total));
  const gap = Math.max(0, params.gap ?? 8);
  const minCol = Math.max(40, params.minColWidth);
  const availWidth = Math.max(minCol, params.containerWidth || minCol);
  const cols = Math.max(1, Math.floor((availWidth + gap) / (minCol + gap)));
  if (total === 0) {
    return { cols, start: 0, end: 0, topSpacer: 0, bottomSpacer: 0, totalHeight: 0 };
  }
  const totalRows = Math.ceil(total / cols);
  const rowWin = computeVirtualWindow({
    total: totalRows,
    itemHeight: Math.max(1, params.rowHeight),
    viewportHeight: params.viewportHeight,
    scrollTop: params.scrollTop,
    overscan: params.overscanRows ?? 2,
  });
  const start = Math.min(total - 1, rowWin.start * cols);
  const end = Math.min(total, rowWin.end * cols);
  return {
    cols,
    start,
    end,
    topSpacer: rowWin.topSpacer,
    bottomSpacer: rowWin.bottomSpacer,
    totalHeight: rowWin.totalHeight,
  };
}

export function escapeHtml(raw: string): string {
  return raw
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

export function plainError(err: unknown): string {
  const raw = typeof err === "string" ? err : err instanceof Error ? err.message : String(err ?? "");
  return raw.replace(/^E\d{3}\s+/, "").replace(/^W\d{3}\s+/, "").trim() || "操作未完成，请重试。";
}

export type TaskProgress = {
  scanComplete: boolean;
  draftReopened: boolean;
  groupCount: number;
  dirNeedsGroup: boolean;
  dirNeedsTask: boolean;
  hasTaskName: boolean;
  grouped: number;
  readyCount: number;
  totalActive: number;
};

export function importStepDone(progress: TaskProgress): boolean {
  return progress.scanComplete || progress.draftReopened;
}

export function canOpenStep(step: StepId, progress: TaskProgress): boolean {
  if (step === "import") return true;
  if (!importStepDone(progress)) return false;
  if (step === "rules") return true;
  if (step === "preview") return progress.totalActive > 0;
  return progress.totalActive > 0;
}

export function normalizeWinPath(path: string): string {
  const norm = path.trim().replaceAll("/", "\\").replace(/\\+$/, "");
  return norm.toLowerCase();
}

export function pathsOverlap(a: string, b: string): boolean {
  const na = normalizeWinPath(a);
  const nb = normalizeWinPath(b);
  if (!na || !nb) return false;
  if (na === nb) return true;
  return nb.startsWith(`${na}\\`) || na.startsWith(`${nb}\\`);
}

export type PathValidationResult = {
  ok: boolean;
  reason: string;
};

export function validateImportPaths(params: {
  sourceRoot: string;
  outputRoot: string;
  backendValid?: boolean;
  backendReason?: string;
}): PathValidationResult {
  const src = params.sourceRoot.trim();
  const out = params.outputRoot.trim();
  if (!src) return { ok: false, reason: "请选择需要整理的源图片文件夹。" };
  if (!out) return { ok: false, reason: "请选择归档输出根目录。" };
  if (pathsOverlap(src, out)) {
    return {
      ok: false,
      reason: "源文件夹与输出目录不能相同或互相包含，请把输出目录选在源文件夹之外。",
    };
  }
  if (params.backendValid === false && params.backendReason) {
    return { ok: false, reason: params.backendReason };
  }
  return { ok: true, reason: "" };
}

const WIN_RESERVED = new Set([
  "CON", "PRN", "AUX", "NUL",
  "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9",
  "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
]);

export function validateComponentName(name: string, label: string): string | null {
  if (!name) return `${label}不能为空`;
  if (name !== name.trim() || name.endsWith(".") || name.endsWith(" ")) {
    return `${label}「${name}」不能包含首尾空白或结尾句点`;
  }
  if (name === "." || name === "..") return `${label}不能为 . 或 ..`;
  if (name.length > 200) return `${label}过长（最多 200 个字符）`;
  if (/[\x00-\x1f<>:"/\\|?*]/.test(name)) {
    return `${label}「${name}」包含 Windows 不允许的字符`;
  }
  const stem = name.split(".")[0] ?? name;
  if (WIN_RESERVED.has(stem.toUpperCase())) {
    return `${label}「${name}」与 Windows 保留设备名冲突`;
  }
  return null;
}

export function parseGroupBatchText(text: string): { ok: true; names: string[] } | { ok: false; error: string } {
  const parts = text
    .split(/[,，;；\r\n\t]+/)
    .map((s) => s.trim())
    .filter(Boolean);
  if (parts.length === 0) {
    return { ok: false, error: "请输入至少一个分类名称（可用逗号或换行分隔）" };
  }
  const seen = new Set<string>();
  for (const name of parts) {
    const err = validateComponentName(name, "分类名称");
    if (err) return { ok: false, error: err };
    const key = name.toLowerCase();
    if (seen.has(key)) {
      return { ok: false, error: `分类名称「${name}」存在重复（Windows 不区分大小写）` };
    }
    seen.add(key);
  }
  return { ok: true, names: parts };
}

export function parseCountsPaste(text: string, groupCount: number): { ok: true; counts: number[] } | { ok: false; error: string } {
  const nums = text
    .trim()
    .split(/[\s,，;；]+/)
    .filter(Boolean)
    .map((s) => Number(s));
  if (nums.length === 0 || nums.some((n) => !Number.isInteger(n) || n < 0)) {
    return { ok: false, error: "请粘贴非负整数列表（空格、逗号或换行分隔）" };
  }
  if (nums.length !== groupCount) {
    return { ok: false, error: `粘贴了 ${nums.length} 个数字，但当前有 ${groupCount} 个分类` };
  }
  return { ok: true, counts: nums };
}

export function previewEffectivePattern(rule: NamingRuleDto): string {
  const sep = rule.separator || "_";
  switch (rule.preset) {
    case "custom":
      return rule.pattern;
    case "date_group_seq": {
      const parts: string[] = [];
      if (rule.prefix.trim()) parts.push("{prefix}");
      if (rule.date_format !== "none") parts.push("{date}");
      if (rule.include_group) parts.push("{group}");
      if (rule.include_tag) parts.push("{tag}");
      if (rule.include_original) parts.push("{orig}");
      if (rule.seq_scope !== "none") parts.push("{seq}");
      if (rule.suffix.trim()) parts.push("{suffix}");
      return parts.length > 0 ? parts.join(sep) : "{orig}";
    }
    case "group_seq":
      return `{prefix}{group}${sep}{seq}{suffix}`;
    case "prefix_seq":
      return `{prefix}{seq}{suffix}`;
    case "orig_group":
      return `{prefix}{orig}${sep}{group}{suffix}`;
    case "tag_seq":
      return `{prefix}{tag}${sep}{seq}{suffix}`;
    default:
      return rule.pattern;
  }
}

export function previewSamplePath(params: {
  taskName: string;
  dirLayout: string;
  dirPattern: string;
  groupName: string;
  tagName?: string;
  origName?: string;
  rule: NamingRuleDto;
  seq?: number;
}): string {
  const task = params.taskName.trim() || "示例归档任务";
  const group = params.groupName.trim() || "风景";
  const tag = params.tagName?.trim() || "精选";
  const orig = (params.origName || "IMG_0001.jpg").replace(/\.[^.]+$/, "");
  const layoutOpt = DIR_LAYOUT_OPTIONS.find((o) => o.key === params.dirLayout);
  const rawDir = params.dirLayout === "custom" ? params.dirPattern : layoutOpt?.pattern ?? "{task}/{group}";
  const dirs = rawDir
    .split(/[\\/]+/)
    .map((s) =>
      s
        .trim()
        .replaceAll("{task}", task)
        .replaceAll("{group}", group)
        .replaceAll("{date}", "2026-05-01")
        .replaceAll("{year}", "2026")
        .replaceAll("{month}", "2026-05")
        .replaceAll("{day}", "01"),
    )
    .filter(Boolean);

  const r = params.rule;
  const sep = r.separator ?? "_";
  const seqStr =
    r.seq_scope === "none"
      ? ""
      : String(Math.max(1, params.seq ?? r.seq_start ?? 1)).padStart(Math.max(1, Math.min(6, r.seq_digits || 3)), "0");
  const dateStr =
    r.date_format === "none"
      ? ""
      : r.date_format === "YYYY-MM-DD"
        ? "2026-05-01"
        : r.date_format === "YYYYMMDD_HHMMSS"
          ? "20260501_102030"
          : "20260501";

  let stem = previewEffectivePattern(r)
    .replaceAll("{prefix}", r.prefix.trim())
    .replaceAll("{suffix}", r.suffix.trim())
    .replaceAll("{task}", task)
    .replaceAll("{group}", group)
    .replaceAll("{tag}", tag)
    .replaceAll("{date}", dateStr)
    .replaceAll("{datetime}", "20260501_102030")
    .replaceAll("{year}", "2026")
    .replaceAll("{month}", "2026-05")
    .replaceAll("{day}", "01")
    .replaceAll("{seq}", seqStr)
    .replaceAll("{orig}", orig);

  if (sep) {
    while (stem.includes(sep + sep)) stem = stem.replaceAll(sep + sep, sep);
  }
  while (stem.includes("__")) stem = stem.replaceAll("__", "_");
  while (stem.includes("--")) stem = stem.replaceAll("--", "-");
  stem = stem.replace(/^[_\-\s]+|[_\-\s]+$/g, "");
  if (!stem) stem = orig;
  return [...dirs, `${stem}.jpg`].join("\\");
}

export type IssueRow = {
  code: string;
  severity: string;
  blocks: boolean;
  message: string;
  photo_id?: string | null;
};

export function archiveBoard(
  issues: IssueRow[],
  readyCount: number,
): Record<ArchiveBucketName, IssueRow[]> {
  const buckets: Record<ArchiveBucketName, IssueRow[]> = {
    已完成: [],
    未完成: [],
    冲突: [],
    时间异常: [],
  };
  for (const iss of issues) {
    if (iss.code === "W001" || iss.code === "W002") {
      buckets["时间异常"].push(iss);
    } else if (iss.code === "W003" || iss.code === "W004") {
      buckets["未完成"].push(iss);
    } else {
      buckets["冲突"].push(iss);
    }
  }
  if (readyCount > 0) {
    buckets["已完成"].push({
      code: "OK",
      severity: "info",
      blocks: false,
      message: `${readyCount} 张图片已完成分类与命名核对，可安全复制归档。`,
    });
  }
  return buckets;
}

export type PlanItemDto = {
  photo_id: string;
  source_path: string;
  source_name: string;
  target_path: string;
  relative_path: string;
  group_name: string;
  target_name: string;
  extension: string;
  size: number;
  blocked: boolean;
};

export type ArchivePreviewGroup = {
  groupName: string;
  items: PlanItemDto[];
};

export function buildArchivePreviewGroups(items: PlanItemDto[]): ArchivePreviewGroup[] {
  const map = new Map<string, PlanItemDto[]>();
  for (const item of items) {
    const key = item.group_name || "默认目录";
    const list = map.get(key) ?? [];
    list.push(item);
    map.set(key, list);
  }
  return Array.from(map.entries()).map(([groupName, groupItems]) => ({
    groupName,
    items: groupItems,
  }));
}

function csvCell(value: string): string {
  const guarded = /^[=+\-@\t]/.test(value) ? `'${value}` : value;
  return /[,"\r\n]/.test(guarded) ? `"${guarded.replaceAll('"', '""')}"` : guarded;
}

export function formatArchivePlanCsv(items: PlanItemDto[], skipIds: Set<string>): string {
  const lines = ["分类,原图片名,目标名称,归档相对路径,目标完整路径,计划动作"];
  for (const item of items) {
    const action = skipIds.has(item.photo_id) ? "跳过" : "复制归档";
    lines.push(
      [
        item.group_name,
        item.source_name,
        item.target_name,
        item.relative_path,
        item.target_path,
        action,
      ]
        .map(csvCell)
        .join(","),
    );
  }
  return `\uFEFF${lines.join("\n")}\n`;
}

export function computeSelectionRange<T extends { id: string }>(
  items: readonly T[],
  anchorId: string | null,
  targetId: string,
): string[] {
  const targetIdx = items.findIndex((x) => x.id === targetId);
  if (targetIdx < 0) return [];
  if (!anchorId) return [targetId];
  const anchorIdx = items.findIndex((x) => x.id === anchorId);
  if (anchorIdx < 0) return [targetId];
  const lo = Math.min(anchorIdx, targetIdx);
  const hi = Math.max(anchorIdx, targetIdx);
  return items.slice(lo, hi + 1).map((x) => x.id);
}

export type ShortcutAction =
  | { type: "help" }
  | { type: "undo" }
  | { type: "redo" }
  | { type: "prev_photo" }
  | { type: "next_photo" }
  | { type: "exclude_photo" }
  | { type: "confirm_photo" }
  | { type: "tag_slot"; index: number }
  | null;

export function shortcutAction(e: {
  key: string;
  ctrlKey: boolean;
  metaKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
  isTyping: boolean;
}): ShortcutAction {
  if (e.key === "F1") return { type: "help" };
  if (e.isTyping) return null;
  if ((e.ctrlKey || e.metaKey) && !e.altKey) {
    const k = e.key.toLowerCase();
    if (k === "z" && !e.shiftKey) return { type: "undo" };
    if (k === "y" || (k === "z" && e.shiftKey)) return { type: "redo" };
    return null;
  }
  if (e.altKey) return null;
  if (e.key === "ArrowLeft") return { type: "prev_photo" };
  if (e.key === "ArrowRight") return { type: "next_photo" };
  if (e.key === "Delete" || e.key === "Backspace") return { type: "exclude_photo" };
  if (e.key === "Enter") return { type: "confirm_photo" };
  if (/^[1-9]$/.test(e.key)) return { type: "tag_slot", index: Number(e.key) - 1 };
  return null;
}

export function contextualHelpContent(step: StepId | "manage"): {
  title: string;
  bullets: string[];
} {
  switch (step) {
    case "import":
      return {
        title: "第 ① 步：导入图片与设置输出位置",
        bullets: [
          "把待整理的文件夹拖入窗口，或点击“选择源图片文件夹”扫描图片；原文件夹全程只读，绝不改动或删除原图。",
          "选择独立的输出根目录（不能与源文件夹相同或互相包含），首次选择后下次会自动记住。",
          "可根据需要填写“任务名称”（默认取自源文件夹名），也可在右侧直接恢复未完成的草稿。",
        ],
      };
    case "rules":
      return {
        title: "第 ② 步：设置分类与命名规则",
        bullets: [
          "左侧选择输出目录层级（支持 任务/分类/文件、仅分类、按月/分类、平铺或自定义）并组合命名规则。",
          "同一分类可包含任意多张图片，开启连续编号（按分类从1或全局连续）即可自动避免重名冲突。",
          "右侧支持四种批量分组方式：按时间（天/月/拍摄间隔）、按数量（每组N张/均分/指定数量）、按预设方案、以及缩略图多选手动归类。",
        ],
      };
    case "preview":
      return {
        title: "第 ③ 步：预览目标路径与精细调整",
        bullets: [
          "每张图片都会实时显示“原文件名 → 预期目标路径”；可按分类筛选、批量移动分类、打标签、按时间或文件名排序。",
          "点击“大图精修”可查看高分辨率预览、旋转画面、打开系统原图、设置单张自定义名称或排除废片。",
          "支持键盘快捷键：← / → 切换图片，1~9 快速打标签，Enter 确认，Delete 排除，Ctrl+Z / Ctrl+Y 撤销重做。",
        ],
      };
    case "archive":
      return {
        title: "第 ④ 步：核对与安全归档",
        bullets: [
          "左侧汇总“已完成 / 未完成 / 冲突 / 时间异常”，所有阻断性冲突未解决前禁止执行复制。",
          "复制采用临时文件校验 + 原子不覆盖提交，绝不会静默覆盖目标目录里已有文件。",
          "归档完成后可打开输出目录、导出 CSV/JSON 归档清单，或在历史记录中安全撤销（仅删除校验通过的本次副本）。",
        ],
      };
    default:
      return {
        title: "方案、标签与系统说明",
        bullets: [
          "内置的示例方案与示例标签仅作通用演示参考，您可以自由编辑、删除或导入导出自己的方案库。",
          "清除缓存或清空历史记录不会影响原图和已归档输出的文件。",
          "本软件数据目录与进程标识完全独立，可与其他整理工具同时运行而不冲突。",
        ],
      };
  }
}
