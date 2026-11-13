# planning/ — 开发规划索引

> **这个目录给 AI 开发者用。** 每次开工前从这里开始：看状态总览 → 点开当前 Sprint → 照单执行。

---

## 当前状态总览

| Sprint | 主题 | 文件 | 状态 | 前置 |
|--------|------|------|------|------|
| **S0** | 环境 & 骨架 | — | ✅ 已完成 | — |
| **S1** | 底层基础设施 | [sprint-01-infrastructure.md](./sprint-01-infrastructure.md) | ⏳ **下一个** | S0 |
| **S2** | 会话数据层 | [sprint-02-session-data.md](./sprint-02-session-data.md) | — | S1 |
| **S3** | 会话 UI 层 | [sprint-03-session-ui.md](./sprint-03-session-ui.md) | — | S2 |
| **S4** | SSH + 终端核心 | [sprint-04-ssh-terminal.md](./sprint-04-ssh-terminal.md) | — | S2 |
| **S5** | 终端渲染 | [sprint-05-terminal-render.md](./sprint-05-terminal-render.md) | — | S4 |
| **S6** | SFTP 文件管理 | [sprint-06-sftp.md](./sprint-06-sftp.md) | — | S4 |
| **S7** | 路径同步 | [sprint-07-sync.md](./sprint-07-sync.md) | — | S5 + S6 |
| **S8** | 远程性能监控 | [sprint-08-monitor.md](./sprint-08-monitor.md) | — | S4 |
| **S9** | 稳定性收尾 | [sprint-09-stability.md](./sprint-09-stability.md) | — | S5–S8 |
| **S10** | v1.0 打磨 & 发布 | [sprint-10-release.md](./sprint-10-release.md) | — | S9 |

---

## 依赖图

```
S0 ✅
 │
 ▼
S1 基础设施 ─────┐
                 │
S2 会话数据 ─────┤──▶ S3 会话 UI
                 │
                 ├──▶ S4 SSH+PTY+TerminalEngine ──┬──▶ S5 终端渲染
                 │                                 ├──▶ S6 SFTP 文件管理
                 │                                 └──▶ S8 远程监控
                 │
                 └──▶ S7 路径同步（依赖 S5 + S6）
                       │
                       ▼
                 S9 稳定性收尾 ──▶ S10 v1.0 发布
```

**关键路径**：S0 → S1 → S2 → S4 → S5 → S9 → S10

**可并行**：S3 与 S4 可并行（都只依赖 S2）；S5 / S6 / S8 三者可并行（都只依赖 S4）

---

## 执行纪律（给 AI 自己看）

1. **严格按 Sprint 顺序**：不跳；每个 Sprint 末尾的 **验收任务** 必须通过才能进下一个
2. **严格按任务编号顺序**：S4.0（perl 验证）必须在 S4.1 之前；Sprint 内任务编号不是摆设
3. **每做完一个任务**：先 `cargo check -p <crate>` 再进下一个；Sprint 末尾 `cargo test --all` + `cargo build --all`
4. **发现阻塞**：暂停、记录、向用户报告，不要硬绕
5. **MVP 范围神圣不可侵犯**：不做 X11 / 多用户 / AI 助手 / 云同步（见 `product/overview.md` 1.2）
6. **EventBus 是唯一跨 View 通道**：View 之间永不直接调用，只 `cx.subscribe`
7. **不阻塞主线程**：ssh2 IO、SFTP 传输、监控采集 必须 `spawn_blocking`

---

## 快速跳转到

| 我想…… | 去这里 |
|--------|--------|
| **开始写代码** | ⏳ **[sprint-01-infrastructure.md](./sprint-01-infrastructure.md)** |
| 看里程碑概览 + 风险缓解 | [roadmap.md](./roadmap.md) |
| 看依赖图 | 本文件「依赖图」章节 |
| 查当前进度 | 本文件「状态总览」表 |

---

## 变更记录

- **2026-11-13** v2.0：从单一 35KB `sprints.md` 拆成 10 个 per-sprint 独立文件；本 README 成为规划目录主索引 + 状态总览
- 之前的单一 sprints.md 历史在 `docs/planning/sprints.md`（已删除）
