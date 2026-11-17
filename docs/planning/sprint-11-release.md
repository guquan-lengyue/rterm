# Sprint 11 — v1.0 打磨 & 发布

> **状态**：就绪（S10 代码任务已完成；第一项先把 S10 9.12.1 GUI 手工 smoke 跑完）
> **前置**：[Sprint 10](./sprint-10-stability.md) ✅（21 项代码任务全绿，手工 smoke 待补）
> **目标**：全量回归（含 S10 手工 smoke）+ Release 构建 + 安装包 + README + CHANGELOG 定稿 + 压力测试
> **后续**：[Sprint 12 — 终端内嵌 SFTP 侧栏](./sprint-12-terminal-sftp-dock.md)（v1.1，不阻塞本 Sprint 发布）
>
> **S10 基线**：`cargo test --workspace` **174 passed / 1 ignored**；`cargo clippy --workspace --all-targets` 0 warning；`cargo fmt --all` 通过；本地门禁脚本 [check.ps1](../../scripts/check.ps1)。
>
> **进度（2026-11-17）**：11.3 Release 构建执行中；11.4 安装包脚本已落地
> [installer/rterm.iss](../../installer/rterm.iss)（本机未装 Inno Setup 6，需安装后 `iscc` 生成）；
> 11.5 README 用户版 ✅；11.6 CHANGELOG 补齐 Sprint 9 ✅；11.7 代码清理核对 ✅（无 TODO/FIXME，clippy 0 warning）；
> 11.1 / 11.2 / 11.8 ⏳ 需真实远端，待手工执行。

---

### 11.1 全量回归（含 S10 9.12.1 手工 smoke）

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 11.1 | 1h | — | 真实远端执行 S10 9.12.1：终端 / SFTP / 监控多 Tab 共存、主题切换、高 DPI 鼠标、拖拽传输、暂停/继续/取消、1000 项目录滚动、空闲 1 分钟首键、双向同步无 `cd` 回显 | S10 遗留 smoke 全部通过 |
| 11.2 | 1h | — | 连接 / 重连 / 终端 / SFTP / 路径同步 / 监控 全场景逐个跑 | 全部过 |

### 11.2 Release 构建验证

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 11.3 | 0.5h | `Cargo.toml` | Release profile 已配 lto=thin / codegen-units=1 / strip=true；核对 `cargo build --release` 产物与日志 | 无 warning，产物 < 20MB |

### 11.3 Windows 安装包

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 11.4 | 1h | `installer/rterm.iss` | Inno Setup 脚本：rterm.exe + 图标 + 文件关联 + 开始菜单 | 生成安装包 → 安装 → 能启动 |

### 11.4 文档收尾

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 11.5 | 0.5h | `README.md` 用户版 | 安装 / 使用 / FAQ + 截图占位；补 S10 新增能力（命令面板、vi 模式、诊断包、会话日志、自动备份） | 清晰完整 |
| 11.6 | 0.5h | `CHANGELOG.md` | 补齐 Sprint 9 条目，并把 S1–S10 汇总为 v0.1 → v1.0 发布说明 | 格式规范、时间线完整 |

### 11.5 代码清理

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 11.7 | 0.5h | 全仓 | 未用 crate / 死代码已在 S7.5 清理；最终核对：TODO 整理 / public API 命名 / doc comment 补全 | 保持 `cargo clippy` 0 warning |

### 11.6 压力测试

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 11.8 | 0.5h | — | 10 并发 Tab + 20 并发传输 + 监控 15min；反复开关 SFTP / Terminal Tab，检查线程数回落 | 无内存/线程泄漏 / 无 panic |

### 11.7 Release 验收

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 11.9 | 0.25h | — | 干净源码树 + README + CHANGELOG + 安装包 + Demo 通过 | **✅ v1.0.0 RELEASE** |

---

### S10 交接项（不阻塞 v1.0，按需取用）

| 项 | 来源 | 处理建议 |
|----|------|----------|
| GUI 手工 smoke（真实远端 / 大目录 / 长空闲） | 9.12.1 | **必做**，已列为 11.1 |
| 命令面板搜索仅 ASCII（无 IME 组合输入） | 9.3 | 记录为已知限制；若要做，需给命令面板换成自绘 IME 输入（估 1h+） |
| OSC 52 只支持远端 → 本地 copy | 9.6 | 维持安全默认，不扩 |
| vi 模式仅导航 + 选词 | 9.7 | v1.1 候选（yank / 搜索），v1.0 记录限制 |
| 回显抑制依赖远端原样回显 | 9.21 | 记录限制；如需彻底解决需 shell 集成（OSC 133），转 v1.1 |
| teardown `window not found` 噪音 | 9.22 | 已过滤；可给 bezel-gpui 上游提 issue（不影响发布） |
| SFTP 本地 mkdir/rename/delete 仍在 UI 线程 | 9.18 | 评估后保留（本地操作 <1ms）；拖拽目录 / 拖影转 v1.1 |
