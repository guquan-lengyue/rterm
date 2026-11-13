# architecture/ — 技术设计索引

> 定义 rterm **怎么实现**。开发中遇到技术选型、数据流、模块边界问题时翻这里。

---

## 文件

| 文件 | 大小 | 内容 | 何时读 |
|------|------|------|--------|
| [architecture.md](./architecture.md) | 23.7 KB | 分层架构 + crate 职责 + 5 个子系统数据流 + 核心数据模型 Rust 草图 + 完整 AppEvent 枚举 | 写代码前 / 数据流变更时 |
| [decisions.md](./decisions.md) | 8.9 KB | 10 条 ADR 技术决策（UI框架、SSH、终端仿真、加密、异步模型等） | 排错 / 新技术选型时 |

---

## 快速查找

| 我想查…… | 去这里 |
|----------|--------|
| crate 职责分工 | [architecture.md#2-crate-划分](./architecture.md#2-crate-划分) |
| AppEvent 完整枚举 | [architecture.md#5-事件总线](./architecture.md#5-事件总线) |
| Session / Settings 数据模型 | [architecture.md#4-核心数据模型](./architecture.md#4-核心数据模型) |
| 终端数据流图 | [architecture.md#32-终端数据流](./architecture.md#32-终端数据流) |
| 路径同步 SyncBridge 设计 | [architecture.md#33-路径同步syncbridge-详细设计](./architecture.md#33-路径同步syncbridge-详细设计) |
| 依赖概览 | [architecture.md#7-配置-cargotoml-依赖概览workspace](./architecture.md#7-配置-cargotoml-依赖概览workspace) |
| 存储路径 | [architecture.md#6-存储路径](./architecture.md#6-存储路径) |
| 为什么选 ssh2 | [decisions.md#adr-002ssh-协议栈--选用-ssh2-libssh2-bindings--vendored-openssl](./decisions.md#adr-002ssh-协议栈--选用-ssh2-libssh2-bindings--vendored-openssl) |
| 为什么选 alacritty_terminal | [decisions.md#adr-003终端仿真--选用-alacritty_terminal-作为后端](./decisions.md#adr-003终端仿真--选用-alacritty_terminal-作为后端) |
| 异步线程模型 | [decisions.md#adr-007异步线程模型--ssh2-blocking--tokio-mpsc-桥接-gpui-主线程](./decisions.md#adr-007异步线程模型--ssh2-blocking--tokio-mpsc-桥接-gpui-主线程) |
| 加密策略 | [decisions.md#adr-005敏感字段加密--windows-dpapi--导出文件-aes-gcm](./decisions.md#adr-005敏感字段加密--windows-dpapi--导出文件-aes-gcm) |

---

## 变更规则

- **架构调整**：改 `architecture.md` 对应章节 + 追加一条 ADR 到 `decisions.md`
- **新技术选型**：只追加 ADR，不删旧 ADR（历史保留）
- **crate 增删**：同步更新 architecture.md 第 2 节的 crate 树 + 本索引的快速查找表
