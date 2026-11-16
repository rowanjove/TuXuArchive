import { describe, it } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  archiveBoard,
  buildArchivePreviewGroups,
  canOpenStep,
  computeSelectionRange,
  computeVirtualGridWindow,
  computeVirtualWindow,
  contextualHelpContent,
  formatArchivePlanCsv,
  parseCountsPaste,
  parseGroupBatchText,
  pathsOverlap,
  previewSamplePath,
  shortcutAction,
  validateComponentName,
  validateImportPaths,
  type NamingRuleDto,
} from "../src/workflow.ts";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const projectRoot = path.resolve(__dirname, "..");

const defaultRule: NamingRuleDto = {
  preset: "date_group_seq",
  pattern: "{date}_{group}_{seq}",
  prefix: "",
  suffix: "",
  date_format: "YYYYMMDD",
  include_group: true,
  include_tag: false,
  include_original: false,
  seq_scope: "per_group",
  seq_start: 1,
  seq_digits: 3,
  separator: "_",
};

describe("naming and directory hierarchy preview", () => {
  it("builds expected relative paths across directory modes and naming rules", () => {
    const p1 = previewSamplePath({
      taskName: "旅行相册",
      dirLayout: "task_and_group",
      dirPattern: "{task}/{group}",
      groupName: "海边风景",
      rule: defaultRule,
      seq: 2,
    });
    assert.equal(p1, "旅行相册\\海边风景\\20260501_海边风景_002.jpg");

    const p2 = previewSamplePath({
      taskName: "旅行相册",
      dirLayout: "group_only",
      dirPattern: "{group}",
      groupName: "海边风景",
      rule: { ...defaultRule, preset: "prefix_seq", prefix: "IMG_", suffix: "_成片", seq_digits: 4 },
      seq: 12,
    });
    assert.equal(p2, "海边风景\\IMG_0012_成片.jpg");

    const p3 = previewSamplePath({
      taskName: "旅行相册",
      dirLayout: "flat",
      dirPattern: "",
      groupName: "海边风景",
      origName: "DSC_9001.jpg",
      rule: { ...defaultRule, preset: "orig_group" },
    });
    assert.equal(p3, "DSC_9001_海边风景.jpg");

    const p4 = previewSamplePath({
      taskName: "旅行相册",
      dirLayout: "custom",
      dirPattern: "{task}/{year}/{group}",
      groupName: "美食",
      rule: { ...defaultRule, preset: "custom", pattern: "{group}-{seq}" },
      seq: 5,
    });
    assert.equal(p4, "旅行相册\\2026\\美食\\美食-005.jpg");
  });
});

describe("path and component validation", () => {
  it("detects overlapping Windows paths case-insensitively", () => {
    assert.equal(pathsOverlap("D:\\Photos\\Trip", "d:/photos/trip/Archive"), true);
    assert.equal(pathsOverlap("D:\\Photos\\Trip\\Sub", "D:\\Photos\\Trip"), true);
    assert.equal(pathsOverlap("D:\\Photos\\Trip", "D:\\Photos\\TripOut"), false);

    const bad = validateImportPaths({
      sourceRoot: "D:\\Photos",
      outputRoot: "D:\\Photos\\Out",
    });
    assert.equal(bad.ok, false);
    assert.match(bad.reason, /不能相同或互相包含/);

    const ok = validateImportPaths({
      sourceRoot: "D:\\Photos",
      outputRoot: "D:\\ArchiveOut",
    });
    assert.equal(ok.ok, true);
  });

  it("rejects Windows reserved names and invalid characters", () => {
    assert.ok(validateComponentName("CON", "分类名称"));
    assert.ok(validateComponentName("abc:def", "文件名"));
    assert.ok(validateComponentName("trailing.", "分类名称"));
    assert.equal(validateComponentName("2026-05_日常相册", "分类名称"), null);
  });
});

describe("batch parsing and workflow guards", () => {
  it("parses batch group names and rejects case-insensitive duplicates", () => {
    const good = parseGroupBatchText("风景, 美食；人物\n夜景");
    assert.equal(good.ok, true);
    if (good.ok) assert.deepEqual(good.names, ["风景", "美食", "人物", "夜景"]);

    const dup = parseGroupBatchText("Album, album");
    assert.equal(dup.ok, false);
  });

  it("parses pasted counts matching group count", () => {
    const ok = parseCountsPaste("4 6 10", 3);
    assert.equal(ok.ok, true);
    if (ok.ok) assert.deepEqual(ok.counts, [4, 6, 10]);

    const mismatch = parseCountsPaste("4 6", 3);
    assert.equal(mismatch.ok, false);
  });

  it("controls step availability based on scan progress", () => {
    const empty = {
      scanComplete: false,
      draftReopened: false,
      groupCount: 0,
      dirNeedsGroup: true,
      dirNeedsTask: true,
      hasTaskName: false,
      grouped: 0,
      readyCount: 0,
      totalActive: 0,
    };
    assert.equal(canOpenStep("import", empty), true);
    assert.equal(canOpenStep("rules", empty), false);

    const scanned = { ...empty, scanComplete: true, totalActive: 10 };
    assert.equal(canOpenStep("rules", scanned), true);
    assert.equal(canOpenStep("preview", scanned), true);
    assert.equal(canOpenStep("archive", scanned), true);
  });
});

describe("archive board, CSV escaping, virtual windows, and shortcuts", () => {
  it("categorizes issues and guards CSV formula prefixes", () => {
    const board = archiveBoard(
      [
        { code: "W001", severity: "warning", blocks: false, message: "无EXIF时间" },
        { code: "W003", severity: "error", blocks: true, message: "未分类" },
        { code: "E002", severity: "error", blocks: true, message: "目标已存在" },
      ],
      5,
    );
    assert.equal(board["已完成"].length, 1);
    assert.equal(board["未完成"].length, 1);
    assert.equal(board["冲突"].length, 1);
    assert.equal(board["时间异常"].length, 1);

    const groups = buildArchivePreviewGroups([
      {
        photo_id: "p1",
        source_path: "D:\\src\\=cmd.jpg",
        source_name: "=cmd.jpg",
        target_path: "D:\\out\\风景\\风景_001.jpg",
        relative_path: "风景\\风景_001.jpg",
        group_name: "风景",
        target_name: "风景_001",
        extension: ".jpg",
        size: 100,
        blocked: false,
      },
    ]);
    assert.equal(groups.length, 1);
    const csv = formatArchivePlanCsv(groups[0].items, new Set());
    assert.match(csv, /'=cmd\.jpg/);
  });

  it("computes selection ranges, virtual windows, and keyboard shortcuts", () => {
    const items = [{ id: "a" }, { id: "b" }, { id: "c" }, { id: "d" }];
    assert.deepEqual(computeSelectionRange(items, "b", "d"), ["b", "c", "d"]);

    const win = computeVirtualWindow({ total: 1000, itemHeight: 60, viewportHeight: 300, scrollTop: 600, overscan: 2 });
    assert.equal(win.start, 8);
    assert.equal(win.end, 17);

    const grid = computeVirtualGridWindow({
      total: 100,
      containerWidth: 400,
      minColWidth: 100,
      rowHeight: 120,
      gap: 8,
      viewportHeight: 240,
      scrollTop: 0,
    });
    assert.equal(grid.cols, 3);
    assert.equal(grid.start, 0);

    assert.deepEqual(
      shortcutAction({ key: "z", ctrlKey: true, metaKey: false, shiftKey: false, altKey: false, isTyping: false }),
      { type: "undo" },
    );
    assert.deepEqual(
      shortcutAction({ key: "3", ctrlKey: false, metaKey: false, shiftKey: false, altKey: false, isTyping: false }),
      { type: "tag_slot", index: 2 },
    );
    assert.equal(
      shortcutAction({ key: "Delete", ctrlKey: false, metaKey: false, shiftKey: false, altKey: false, isTyping: true }),
      null,
    );
    assert.ok(contextualHelpContent("rules").bullets.length >= 3);
  });
});

describe("UI contract and sanitization check", () => {
  it("uses native dialog, brand title, and zero domain leftovers", () => {
    const html = fs.readFileSync(path.join(projectRoot, "index.html"), "utf8");
    const css = fs.readFileSync(path.join(projectRoot, "src/styles.css"), "utf8");
    assert.ok(html.includes("<dialog id=\"app-dialog\""));
    assert.ok(html.includes("图序归档"));
    assert.ok(css.includes("dialog.app-dialog:modal"));
    assert.ok(css.includes("scrollbar-gutter: stable"));

    for (const forbidden of ["无人机", "巡检", "节号", "塔型", "飞手", "线路"]) {
      assert.equal(html.includes(forbidden), false, `index.html contains domain term: ${forbidden}`);
    }
  });
});
