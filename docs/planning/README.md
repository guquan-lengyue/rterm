# planning/ — 开发规划索引

> **这个目录给 AI 开发者用。** 每次开工前从这里开始：看状态总览 → 点开当前 Sprint → 照单执行。

---

## 当前状态总览

| Sprint | 主题 | 文件 | 状态 | 前置 |
|--------|------|------|------|------|
| **S0** | 环境 & 骨架 | — | ✅ 已完成 | — |
| **S1** | 底层基础设施 | [sprint-01-infrastructure.md](./sprint-01-infrastructure.md) | ✅ 已完成 | S0 |
| **S2** | 会话数据层 | [sprint-02-session-data.md](./sprint-02-session-data.md) | ✅ 已完成 | S1 |
| **S3** | 会话树 + 编辑对话框 | [sprint-03-session-tree.md](./sprint-03-session-tree.md) | ✅ 已完成（2026-11-14） | S2 |
| **S4** | 设置 + 导入导出 | [sprint-04-settings-io.md](./sprint-04-settings-io.md) | ✅ 已完成（2026-11-14） | S3 |
| **S5** | SSH 连接 & 终端核心 | [sprint-05-ssh-terminal.md](./sprint-05-ssh-terminal.md) | ✅ 已完成（2026-11-14） | S2 + S3 + S4 |
| **S6** | 终端渲染 | [sprint-06-terminal-render.md](./sprint-06-terminal-render.md) | ✅ 已完成（2026-11-16） | S5 |
| **S7** | SFTP 文件管理 | [sprint-07-sftp.md](./sprint-07-sftp.md) | ✅ 已完成（S7 + S10 收口） | S5 + S6 ✅ |
| **S7.5** | 技术债清偿（S8 前插入） | [sprint-07.5-tech-debt.md](./sprint-07.5-tech-debt.md) | ✅ 已完成（2026-11-16） | S7 🚧 |
| **S8** | 路径同步 | [sprint-08-sync.md](./sprint-08-sync.md) | ✅ 已完成（2026-11-17，含真实 GUI E2E） | S6 ✅ + S7 🚧 + S7.5 ✅ |
| **S9** | 远程性能监控 | [sprint-09-monitor.md](./sprint-09-monitor.md) | ✅ 已完成（2026-11-17） | S5 + S6 ✅ + S7.5 ✅ |
| **S10** | 稳定性收尾 | [sprint-10-stability.md](./sprint-10-stability.md) | ✅ 代码任务完成（2026-11-17，待 9.12.1 手工 smoke → S11 11.1） | S6 ✅–S9 ✅ + S7.5 ✅ |
| **S11** | v1.0 打磨 & 发布 | [sprint-11-release.md](./sprint-11-release.md) | 🚧 就绪（先跑 S10 9.12.1 手工 smoke） | S10 ✅ |
| **S12** | 终端内嵌 SFTP 侧栏（v1.1） | [sprint-12-terminal-sftp-dock.md](./sprint-12-terminal-sftp-dock.md) | 📋 待开始（不阻塞 v1.0） | S8 ✅ + S10 ✅ + S11 |
| **S13** | 会话管理增强 + SSH 跳板代理/端口转发 + 顶部菜单栏（v1.2） | [sprint-13-session-management-proxy.md](./sprint-13-session-management-proxy.md) | 📋 待开始（可与 S12 并行） | S2 ✅ + S3 ✅ + S4 ✅ + S5 ✅ |

---

## 依赖图

```
S0 ✅
 │
 ▼
S1 ✅ 基础设施 ────┐
                   │
S2 ✅ 会话数据 ────┼──▶ S3 ✅ 会话树 + 编辑对话框 ══▶ S4 ✅ 设置 + 导入导出
                   │
                   └──▶ S5 ✅ SSH核心 ────┬──▶ S6 ✅ 终端渲染 ──▶ S7 ✅ SFTP（S10 收口）
                                         │                         │
                                         │                         ▼
                                         │                 S7.5 ✅ 技术债清偿
                                         │                         │
                                         │                         ▼
                                         │                 S8 ✅ 路径同步
                                         │
                                         └──▶ S9 ✅ 远程监控

                         S10 ✅ 稳定性收尾 ──▶ S11 🚧 v1.0 发布 ──▶ S12 📋 内嵌 SFTP 侧栏（v1.1）
                                                        │
                                                        └──▶ S13 📋 会话管理 + 跳板代理（v1.2）
```

**关键路径**：S0 → S1 → S2 → S3 → S4 → S5 → S6 → S7 → **S7.5** → S8 → S9 → **S10** → S11 → S12 / S13

**当前状态**：
- S10 的 21 项代码任务全部完成，workspace 174 passed / 1 ignored，clippy 0 warning
- S10 只差 9.12.1 GUI 手工 smoke（真实远端 / 大目录 / 长空闲 / 多类型 Tab），已并入 S11 11.1
- S11 依次执行：全量回归 → Release 构建 → 安装包 → 文档（补 S9 CHANGELOG）→ 代码清理 → 压力测试 → 发布验收
- S10 已知限制（命令面板仅 ASCII、vi 仅导航、回显抑制边界等）列在 S11 文档「S10 交接项」
- S12 已立项（v1.1）：终端 Tab 右侧停靠 SFTP 侧栏并跟随终端 cwd，复用 S8 的 `TerminalCwdHint`，不阻塞 v1.0
- S13 已立项（v1.2）：分组命名/下拉、会话图标、空白区右键、顶部菜单栏、三类右键菜单职责分离、
  导入导出完善、SSH 跳板代理与端口转发（-L/-R，-D 可选）；会话文件升级 schema v2（v1 可读，不支持降级）

---

## UI 原型（ASCII 线框图）

> 所有 View 的布局 / 尺寸 / 交互已在 `docs/architecture/ui-prototypes.md` 定义。写 UI 任务前先看一眼。

---

## 执行纪律（给 AI 自己看）

1. **严格按 Sprint 顺序**：不跳；每个 Sprint 末尾的 **验收任务** 必须通过才能进下一个
2. **严格按任务编号顺序**：5.0（perl 验证）必须在 5.1 之前；Sprint 内任务编号不是摆设
3. **每做完一个任务**：先 `cargo check -p <crate>` 再进下一个；Sprint 末尾 `cargo test --workspace` + `cargo build --workspace`
4. **发现阻塞**：暂停、记录、向用户报告，不要硬绕
5. **MVP 范围神圣不可侵犯**：不做 X11 / 多用户 / AI 助手 / 云同步（见 `product/overview.md` 1.2）
6. **EventBus 是唯一跨 View 通道**：View 之间永不直接调用，只 `cx.subscribe`
7. **不阻塞主线程**：ssh2 IO、SFTP 传输、监控采集必须放在独立后台线程/任务，通过 channel 回 UI

---

## 快速跳转到

| 我想…… | 去这里 |
|--------|--------|
| **开始写代码** | 🚧 **[sprint-11-release.md](./sprint-11-release.md)**（先执行 11.1：S10 9.12.1 手工 smoke） |
| 看 Sprint 完成总结 | [CHANGELOG.md](./CHANGELOG.md)（S1–S8、S10 已记录；S9 待 S11 11.6 补齐） |
| 看里程碑概览 + 风险缓解 | [roadmap.md](./roadmap.md) |
| 看 UI 原型线框图 | `docs/architecture/ui-prototypes.md` |
| 查依赖图 | 本文件「依赖图」章节 |
| 查当前进度 | 本文件「状态总览」表 |

---

## 变更记录

- **2026-11-19** v4.5：新增 **S13 会话管理增强 + SSH 跳板代理/端口转发（v1.2）**——分组 CRUD 与可搜索下拉、
  会话图标、空白区右键、顶部菜单栏、右键菜单三态职责分离、导入导出完善、跳板代理与端口转发（-L/-R，-D 可选）；
  会话文件升级 schema v2（向后兼容 v1，不支持降级）
- **2026-11-17** v4.4：新增 **S12 终端内嵌 SFTP 侧栏（v1.1）**——同一终端 Tab 右侧停靠 SFTP，
  路径跟随终端 cwd（受同步模式门控、无回环）；不阻塞 S11 的 v1.0 发布
- **2026-11-17** v4.3：S10 完成（21 项代码任务 + 回归；174 passed / 1 ignored，clippy 0 warning，fmt 通过）；S10 总结写入 CHANGELOG；S11 重排任务（11.1 先跑 9.12.1 手工 smoke，新增「S10 交接项」）；roadmap Phase 3/5/6 收口、Phase 7 进行中
- **2026-11-17** v4.2：S9 远程监控完成（MonitorView / 采集 / Top / Kill；/proc awk 优化 CPU%）；S10 转为下一个
- **2026-11-17** v4.1：S8 完成（双向路径同步 + 防回环 + Off 模式，真实 GUI E2E 通过；104 passed / 1 ignored，clippy 0 warning，fmt 通过）；S9 转为下一个
- **2026-11-17** v4.0：S8 核心落地（SyncBridge + 终端/SFTP 路由 + pwd 兜底；103 passed / 1 ignored）；真实 GUI 双向 E2E 待复验
- **2026-11-16** v3.9：S7.5 完成（P0–P3 + 主机密钥收口；92 passed / 1 ignored，clippy 0 warning，fmt 通过）；S8 转为下一个
- **2026-11-16** v3.8：S7.5 P2 状态/交互已落地（workspace 75 passed / 1 ignored）；剩余 P3 工程门禁 + 主机密钥收口
- **2026-11-16** v3.7：S7.5 P0 数据可靠性 + P1 线程/资源已落地（workspace 66 passed / 1 ignored）；P2/P3 待续
- **2026-11-16** v3.6：插入 **S7.5 技术债清偿**（里程碑走查产出；S8 前强制执行）；S8 状态改为待开始，依赖追加 S7.5；当前入口切到 S7.5
- **2026-11-16** v3.5：S7 核心完成（SftpService / TransferQueue / 双栏 View / 多类型 Tab / 会话右键打开 SFTP）；拖拽与暂停取消 E2E 转入 S10
- **2026-11-16** v3.4：S6 收尾——补 5.6 验收记录（vim/htop/多标签/回滚/复制粘贴），同步 roadmap / UI 原型 / S7–S11 交接说明（多类型 Tab 改造点、SyncBridge 挂载点、S6 遗留收口）
- **2026-11-16** v3.3：S6 完成（Canvas 逐 Cell 渲染 / 光标 / 滚动 / 选择复制 / resize / 粘贴 / 主题调色板 / 标签页 / 字号缩放 / 状态栏）；S7 转为「下一个」
- **2026-11-14** v3.2：S5 完成（SSH 连接 / 认证 / known_hosts / PTY / TerminalEngine / 重连）；S6 转为「下一个」；S5 落地说明与遗留写入 CHANGELOG
- **2026-11-14** v3.1：S4 完成（设置 + 导入导出）；S5 转为「下一个」；把 S4 交付资产（Settings 默认值 / 终端外观 / 同步模式 / 监控阈值 / 事件）写入 S5–S10 的接入点与任务
- **2026-11-13** v3.0：拆分 S3 为 S3（会话树+编辑对话框）和 S4（设置+导入导出），后续全顺延 → S11；新增 UI 原型线框图（`docs/architecture/ui-prototypes.md`）
- **2026-11-13** v2.2：S2 完成，状态表更新；新增 CHANGELOG.md 记录 Sprint 总结
- **2026-11-13** v2.1：S1 完成，状态表更新；新增 CHANGELOG.md 记录 Sprint 总结
- **2026-11-13** v2.0：从单一 35KB `sprints.md` 拆成 per-sprint 独立文件；本 README 成为规划目录主索引 + 状态总览
- 之前的单一 sprints.md 历史在 `docs/planning/sprints.md`（已删除）
