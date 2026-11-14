# Sprint 4 — 设置 + 导入导出

> **状态**：✅ 已完成（2026-11-14）
> **前置**：S3（SessionTree + 编辑对话框 + 启动退出已可用）
> **目标**：全局设置界面 SettingsView + 导入导出对话框 ImportExportDialog + Settings 持久化
> **定位**：S3 管"会话数据增删改"，S4 管"全局配置 + 数据迁移"
>
> **实现说明（与计划的偏差）**：
> - SettingsView 共 **7 个 Tab**：常规 / SSH 默认值 / 终端外观 / SFTP / 监控 / 高级 / 导入导出（4.2.3 的入口合并进设置）
> - 主题切换**即时生效**（整个应用换肤），取消时还原为已保存主题；新增 Dracula / One Dark 两套主题
> - 「文件夹 / 选中会话」用 chips + 勾选列表（gpui 0.3 无原生下拉框），选中列表最多展示 24 条
> - 导入预览最多展示 12 条，超出显示剩余数量
> - `settings.json` 使用 `#[serde(default)]`，缺字段/未知主题自动回退默认值
> - 快捷键 `Ctrl+,` 走 `App::on_action` 全局监听 + `app.defer`（action 派发期间窗口被临时移出 `App.windows`，不能同步 update）
> - 导入错误信息只显示顶层原因（避免长错误链把 footer 按钮挤出卡片）
>
> **S3 可直接复用的资产**：
> - 主题系统 `theme.rs`（Theme trait + CatppuccinMocha / WindowsLight，AppState.theme 可切换）
> - TextInput 组件（IME + Backspace/Delete/方向键/Home/End/Ctrl+A）
> - 自绘 Modal / 右键菜单 / 表单布局模式（SessionEditDialog 可作 SettingsView 模板）
> - 基础导入导出（固定路径、无口令、Skip 策略）：4.2 在此基础上加文件选择器 / 口令 / 冲突策略 / 预览
> - SessionStore 通过 `Arc<Mutex<>>` 共享，读写都走 `cx.global::<AppState>()`
>
> **📐 UI 原型**：详见 `docs/architecture/ui-prototypes.md` §4 SettingsView + §5 ImportExportDialog + §10 存储路径

---

### 4.1 全局设置界面

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 4.1.1 | 0.5h | `crates/rterm-app/src/views/settings.rs` | SettingsView：左侧 TabList（常规 / SSH 默认值 / 终端外观 / SFTP / 监控 / 高级）+ 右侧表单面板；`Ctrl+,` 打开 | 能切 Tab；每个 Tab 内容不同 |
| 4.1.2 | 0.5h | `crates/rterm-app/src/views/settings.rs` | **SSH 默认值 Tab**：DefaultPort, DefaultUsername, Keepalive, ConnectTimeout；**SFTP Tab**：SftpSyncMode 四选一 Radio；**监控 Tab**：MonitorInterval + CPU/Mem 阈值；**高级 Tab**：日志目录 + 数据目录自定义路径 | 改一个字段 → 广播 `AppSettingsChanged` |
| 4.1.3 | 0.5h | `crates/rterm-app/src/views/settings.rs` | **终端外观 Tab**：主题下拉（S3 已有 Catppuccin Mocha / Windows Light 两套，S4 再补 Dracula + One Dark；切换即替换 `AppState.theme`）+ 字体选择（系统枚举）+ 字号（10–24）+ 光标样式（方块/下划线/竖线）+ 滚动缓冲行数；右侧内嵌 gpui Canvas 预览面板（模拟终端渲染一行彩色文字） | 切主题 → 预览面板立即变色；改字号 → 预览字号变 |
| 4.1.4 | 0.25h | `crates/rterm-app/src/views/settings.rs` | **保存 / 取消 / 恢复默认**：保存 → 写 `settings.json` + `broadcast AppSettingsChanged`；取消 → 丢弃改动；恢复默认 → 所有字段重置为 `Settings::default()` | 重启 app → Settings 字段保留 |
| 4.1.5 | 0.25h | `crates/rterm-app/src/main.rs` | Settings 加载/保存：`settings.json` 路径 = `dirs::data_dir()/rterm/settings.json`；不存在 → `Settings::default()`；与 sessions.json 同目录 | 路径对齐 UI 原型 §10 |

### 4.2 导入导出对话框

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 4.2.1 | 0.5h | `crates/rterm-app/src/views/import_export.rs` | **导出 Modal**：路径选择（rfd FileDialog）+ ExportScope 三选一（全部 / 文件夹下拉 / Selection 列表）+ 加密方式（DPAPI 本机 / AES-GCM 口令 + 确认）→ 调 `rterm_session::export_json` + 写文件 | 能导出 JSON；口令加密文件换机器不可解；DPAPI 文件本机可解 |
| 4.2.2 | 0.5h | `crates/rterm-app/src/views/import_export.rs` | **导入 Modal**：路径选择 + 口令输入（可空）+ ConflictPolicy 三选一 + **预览 diff**（扫 JSON → 列出将要 added/updated/skipped 的会话名）→ 执行 → 显示 ImportReport（added/updated/skipped/renamed 数字） | 导出 → 清内存 store → 导入 → 数量一致 |
| 4.2.3 | 0.25h | `crates/rterm-app/src/views/import_export.rs` | 入口：SettingsView 里加 "导入/导出" Tab；SessionTree 右键菜单（S3 已有「导出会话 JSON / 导入会话 JSON」基础版，此处替换为完整 Modal 入口） | 两处入口都能弹出同一个 Modal |

### 4.3 验收

| # | 粒度 | 验收 |
|---|------|------|
| 4.3.1 | 0.25h | 设置界面 → 切终端主题 Dracula → 预览面板立刻变紫 → 保存 → 重启 → 再打开设置 → 主题还在 Dracula |
| 4.3.2 | 0.25h | 导出"全部"到 `test.json` → DPAPI 加密 → 改一个会话 → 导出 → 导入 Overwrite → 数量一致 |
| 4.3.3 | 0.25h | 导出 AES-GCM + 口令 "abc" → 清内存 → 导入错误口令 → Modal 显示 "解密失败"；正确口令 → 成功 |
| 4.3.4 | 0.25h | `cargo check --workspace` 0 error；`cargo test --workspace` 之前的 20 个测试全过 |

---

### 文件清单

```
修改：
  crates/rterm-app/src/main.rs              ← Settings load/save
  crates/rterm-app/src/views/settings.rs    ← 新增
  crates/rterm-app/src/views/import_export.rs  ← 新增
  docs/architecture/ui-prototypes.md  §4 §5  ← 已在上一步完成，无需再改
```
