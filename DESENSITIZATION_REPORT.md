# 图序归档（TuXuArchive）开发交付与脱敏检查报告

## 1. 项目概览与工程隔离

新产品 **「图序归档」（TuXuArchive v1.0.0）** 在独立目录 `通用图片归档工具/` 内完成全部重构、构建与验证，未修改、删除或覆盖原巡检软件的任何源码、配置或既有交付产物。

### 运行时与配置隔离对照

| 隔离项 | 图序归档（新产品） | 说明 |
| --- | --- | --- |
| 产品名称 / 窗口标题 | `TuXuArchive` / `图序归档 — 通用图片批量分类与重命名工具` | 完全重命名并独立打包 |
| 应用标识（Identifier） | `local.tuxu.archive` | 与旧版标识完全隔离 |
| 单实例互斥锁（Mutex） | `Local\TuXuArchive` | 支持与旧版软件同时启动运行 |
| 应用数据目录 | `%LOCALAPPDATA%\TuXuArchive\data`（支持 `TUXU_DATA_DIR` 覆盖） | 独立 SQLite 数据库与缩略图缓存 |
| 自解压运行时目录 | `%LOCALAPPDATA%\TuXuArchive\runtime` | 独立解锁提取、互不冲突 |
| 环境变量前缀 | `TUXU_DATA_DIR` / `TUXU_NO_UI` / `TUXU_ZIP_PATH` | 独立开发/测试/打包变量 |
| Vite 开发端口 | `http://localhost:1422` | 避免本地端口冲突 |
| 包名 / Crate 名 | `tuxu-archive` / `tuxu-core` / `tuxu-archive-launcher` | 独立 Cargo 与 npm 工作区 |
| 软件图标 | `src-tauri/icons/{32x32,64x64,128x128,128x128@2x,icon}.png`、`icon.ico` | 全新设计白底蓝图归档主题图标 |

---

## 2. 产品通用化与界面重构要点

1. **业务模型泛化（`crates/tuxu-core`）**
   - 将行业专属模型抽象为通用的 **「归档任务（Task）→ 分类/分组（Group）→ 命名规则（NamingRule）」**。
   - 支持**同一分类/分组包含多张图片**，通过连续序号 `{seq}`、自定义前后缀 `{prefix}` / `{suffix}`、日期时间 `{date}` / `{datetime}` / `{year}` / `{month}` / `{day}`、分类 `{group}`、标签 `{tag}` 与原文件名 `{orig}` 自由组合。
   - 支持 6 种目录层级模板（`task_and_group`、`group_only`、`date_and_group`、`task_only`、`flat`、`custom`）。
   - 提供 4 种批量分组方式：**按时间（按天/按月/按拍摄间隔）**、**按数量（固定张数/均分多组/自定义数量）**、**按预设方案**、**手动框选/多选指派**。
   - 移除行业词库、业务模板与演示数据，改用清晰标注的通用示例方案（`EXAMPLE_PRESETS`：摄影相册整理、电商/商品素材、文档与票据扫描、设计项目归档）与通用示例标签库（`EXAMPLE_TAGS`）。

2. **安全保护与可靠性延续**
   - 保留原图只读保护、共享只读打开、目标零覆盖拦截（同名冲突报错码 `E002`）、路径重叠与非法字符检查、SQLite 提交意图日志、中断恢复重试、CSV/JSON 归档清单导出，以及基于文件身份（FileId）与 SHA-256 的安全撤销机制。

3. **界面信息架构重构（`index.html`、`src/workflow.ts`、`src/styles.css`）**
   - 改为面向普通用户的浅色桌面四步流程：**① 导入与位置 → ② 分类与命名 → ③ 预览与调整 → ④ 核对与归档**。
   - 保留虚拟滚动预览表格、实时路径预览、大图精修弹窗（原生 `<dialog class="app-dialog">`）、快捷键操作（`←/→`、`1~9`、`Enter`、`Delete`、`Ctrl+Z`/`Ctrl+Y`）与示例模板管理。

---

## 3. 脱敏检查结果

- **检查工具**：[`scripts/sanitization_check.ps1`](scripts/sanitization_check.ps1)
- **检查范围**：新项目内所有 `.rs`、`.ts`、`.js`、`.html`、`.css`、`.json`、`.toml`、`.ps1`、`.py`、`.md`、`.txt` 文件（排除构建缓存 `target/`、`node_modules/`、`dist/`、`dist-release/` 及检查脚本/本报告自身）。
- **检查规则**：覆盖原行业机构名称、电力/电网/无人机/巡检/杆塔/塔型/节号等业务词汇、个人标识、硬编码本机路径（`C:\Users\` 等）以及旧产品标识/环境变量前缀。
- **扫描结果**：
  - 扫描文件数：`47`
  - 检出敏感或残留项：`0`
  - 结论：`SANITIZATION_CHECK_PASSED`

---

## 4. 验证与分发产物

### 已完成构建产物（`dist-release/20261116-190612/`）

- `TuXuArchive_v1.0_x64.exe`（`5,800,448` 字节，SHA-256：`3cafeeeebb18b9492d531c5d6af12904578907d4aa66ccef84bf153ef6b33804`）
  - 免安装自解压单文件启动器，静态链接 MSVC CRT（`+crt-static`），`dumpbin.exe` 依赖扫描通过，仅依赖系统基础 DLL（`advapi32.dll`、`api-ms-win-core-synch-l1-2-0.dll`、`bcryptprimitives.dll`、`kernel32.dll`、`ntdll.dll`、`user32.dll`）。
- `TuXuArchive_v1.0_Portable_x64.zip`（`5,118,657` 字节，SHA-256：`9ffb07fd376958cb8357932659f49258905dcd8534ae3eaf361c017b00b2496b`）
  - 内含 `TuXuArchive\TuXuArchive.exe`、`README.txt` 与 `manifest.sha256`。
- `dependency-report.json`（`passed: true`，覆盖 2 个发布二进制文件，无异常或第三方运行库依赖）、`build-info.json`、`SHA256SUMS.txt`、`source-manifest.json`。

### 自动化测试与验证摘要

- **脱敏扫描**：`powershell -ExecutionPolicy Bypass -File ".\scripts\sanitization_check.ps1"` -> 扫描 `47` 个文件，发现 `0` 项，通过。
- **前端类型检查与打包**：`npm run build`（`tsc --noEmit && vite build`） -> 通过（产出 `dist/index.html`、`dist/app.css`、`dist/app.js`）。
- **前端单元测试**：`npm test` -> `9` 项通过，`0` 项失败。
- **Rust Clippy 静态检查**：`cargo clippy --workspace --all-targets` -> 通过（`tuxu-core` 与 `src-tauri` 零警告）。
- **Rust 全工作区测试**：`cargo test --workspace` ->
  - `tuxu-core` 单元测试：`14` passed，`0` failed，`1` ignored（`durable_handle_commit_timing`，需显式运行的手工耗时基准）；
  - `tuxu-core` 集成测试（`integration_tests.rs`）：`5` passed，`0` failed（覆盖：单类多图归档与安全撤销+防篡改保护、目标防覆盖 `E002`、手动多选与中断恢复、路径重叠拦截与 1000 张图片批量性能、`flat`/`task_only` 无分组自动确认）；
  - `tuxu-archive`（`tuxu_archive_lib`）：`1` passed，`0` failed（并发任务互斥锁测试）；
  - `tuxu-archive-launcher`（`TuXuArchive_v1_0_x64`）：`7` passed，`0` failed（提取目录回退、路径穿越拦截、崩溃安全与死锁接管、哈希校验回滚）。
- **CLI 端到端烟雾测试**：使用 `cargo run -p tuxu-core --bin archive-once` 对临时目录 4 张合成 PNG 图片按每组 2 张归档为 `示例相册\分组01\分组01_001.png` 等 4 个文件，`succeeded: 4, failed: 0`（在代码审查修复前执行）。

### 未验证或受环境限制事项

- **实时 Windows 桌面 GUI 交互**：当前环境未人工点击运行中的 WebView2 图形窗口，因此窗口多分辨率缩放、系统文件选择对话框交互与拖拽事件流未做人工真机目视点击验证。
- **特殊影像编码实机解码**：本机未实测真实手机/相机拍摄的 HEIC/RAW(DNG) 样张在目标机器 WIC（Windows Imaging Component）中的实际解码表现（依赖目标系统安装的 HEIF/Raw 扩展）。
- **“干净无开发环境系统”实机运行**：`dumpbin.exe` 与 `+crt-static` 已验证 EXE 无 VC++ Redistributable 静态依赖，但尚未在未安装 Rust/Node/VS 的纯净 Windows 10/11 虚拟机上实际双击启动验证 WebView2 运行时行为。
