# 图序归档 (TuXu Archive) v1.0.0

一款轻量、离线、安全可靠的 Windows 通用图片批量分类、重命名与归档工具（基于 Tauri 2 + Rust + TypeScript 构建）。

## 核心特性

1. **四步新手向导，默认批量快排**
   - **① 导入与位置**：支持拖入或浏览选择源图片文件夹（可选包含子文件夹），自动提取 EXIF 拍摄时间排序；设置独立输出根目录（自动记忆）与任务名称。
   - **② 分类与命名**：自由选择目录层级（`任务/分类/文件`、`分类/文件`、`月份/分类/文件`、`任务/文件`、平铺或自定义模板）与组合命名规则（支持日期、分类名、标签、原文件名、自定义前后缀及连续编号）；支持**按时间（天/月/拍摄间隔）**、**按数量（固定张数/均分/指定张数）**、**按预设方案**与**手动缩略图多选**四种批量分组方式。
   - **③ 预览与调整**：实时预览每张图片的预期目标路径；同一分类可包含多张图片并通过连续编号自动避免重名；支持批量改类、打标签、排序、排除废片，以及“大图精修”与键盘快捷键（`←/→`、`1~9`、`Enter`、`Delete`、`Ctrl+Z`/`Ctrl+Y`）。
   - **④ 核对与安全归档**：执行前进行零覆盖、路径重叠、非法字符、磁盘空间检查；归档全程只读原图、写入临时文件校验后再原子提交。

2. **安全保障与恢复能力**
   - 原图只读保护与共享锁读取，绝不改动或删除源目录照片；
   - 目标防覆盖保护（发现目标已存在或规则导致重名冲突立即拦截）；
   - SQLite 提交意图持久化与中断恢复重试；
   - 支持导出 CSV / JSON 归档清单，支持基于文件身份（FileId）与 SHA-256 核对的安全撤销（外部修改过的副本将被保留并提示）。

3. **独立工程与运行隔离**
   - 独立应用标识 `local.tuxu.archive`、互斥锁 `Local\TuXuArchive`、数据目录 `%LOCALAPPDATA%\TuXuArchive\data` 与运行时目录 `%LOCALAPPDATA%\TuXuArchive\runtime`；
   - 内置通用示例分类方案与示例标签库（摄影相册、商品素材、文档票据、设计归档），可自由编辑或导入导出。

## 开发、测试与构建

```powershell
# 进入项目目录
cd 通用图片归档工具

# 前端测试与构建
npm test
npm run build

# Rust 全工作区测试（核心引擎 + 集成测试 + Tauri + 启动器）
cargo test --workspace

# 构建正式分发包（生成 Portable ZIP 与免安装自解压 EXE）
powershell -ExecutionPolicy Bypass -File .\build-release.ps1
```

## 分发产物

构建输出位于 `dist-release/<timestamp>/`：
- `TuXuArchive_v1.0_x64.exe`：免安装自解压单文件版（静态链接 MSVC CRT，无需额外安装 VC++ 运行库、Python、Rust 或 Node，仅需 Windows 10/11 系统自带 WebView2 Runtime）
- `TuXuArchive_v1.0_Portable_x64.zip`：绿色免安装压缩包版
- `SHA256SUMS.txt`、`dependency-report.json`、`build-info.json`
