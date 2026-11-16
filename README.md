# rterm

一个用 **Rust + bezel-gpui** 构建的跨平台远程终端管理工具，对标 sshxterm / Xshell。

核心特性：SSH 会话管理、远程终端（xterm-256color）、SFTP 文件管理、远程性能监控、**SFTP ↔ 终端路径双向同步**、会话加密导入导出。

> **MVP 开发中。v1.0 目标：Windows 完整验证，macOS/Linux 源码可构建。**

---

## 快速开始

### 依赖

- Rust 1.85+（MSVC 工具链）
- Visual Studio 2022 Build Tools（C++ workload + MSVC + Windows SDK）
- CMake 3+
- Strawberry Perl（vendored OpenSSL 编译）

### 构建

```powershell
# 1. 加载 MSVC + CMake 环境（每次新 shell 都要先跑）
. .\scripts\msvc-env.ps1

# 2. 编译
cargo build -p rterm-app

# 3. 运行
.\target\debug\rterm-app.exe
```

代理配置写在 `~/.cargo/config.toml`。

---

## 文档入口

所有设计文档已按职责分目录管理，见 **[docs/README.md](docs/README.md)** 总索引。

```
docs/
├── README.md                   ← 总索引（你应该从这里开始）
├── product/                   ← WHAT — 产品做什么
│   └── overview.md             ← 项目定位 + M1–M6 详细功能列表
├── architecture/              ← HOW — 技术怎么实现
│   ├── architecture.md         ← 分层架构 + crate 职责 + 数据流 + 数据模型
│   └── decisions.md            ← 10 条 ADR 技术决策记录
└── planning/                   ← WHEN — 交付节奏
    ├── README.md               ← 当前 Sprint 状态总览 + 依赖图（开工先看这里）
    ├── roadmap.md              ← Phase 0–7 里程碑 + 风险缓解
    ├── sprint-01…11-*.md       ← 每个 Sprint 的详细任务 + 验收
    └── CHANGELOG.md            ← Sprint 完成总结
```

---

## Crate 结构

```
crates/
├── rterm-app      # 入口 + gpui 主窗口 + AppState + EventBus
├── rterm-session  # Session 模型 + CRUD + 持久化 + 导入导出
├── rterm-ssh      # ssh2 crate 封装 + 重连 + keepalive
├── rterm-terminal # PTY + alacritty_terminal glue + 输入输出桥接
├── rterm-sftp     # SFTP 浏览 + 传输队列
├── rterm-monitor  # 远程采集 + 时序聚合 + 告警
├── rterm-sync     # 终端 ↔ SFTP 路径同步核心（SyncBridge）
└── rterm-crypto   # DPAPI + AES-GCM 加解密
```

---

## 参考

- [bezel-gpui on crates.io](https://crates.io/crates/bezel-gpui) — Zed gpui 的活跃 fork，Windows/macOS/Linux 三平台构建
- [alacritty_terminal](https://docs.rs/alacritty_terminal/latest/alacritty_terminal/) — 终端仿真核心库
- [ssh2 crate](https://docs.rs/ssh2/latest/ssh2/) — libssh2 bindings

---

## 许可证

MIT（或按项目最终决定）
