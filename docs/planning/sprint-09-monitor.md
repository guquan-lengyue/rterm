# Sprint 9 — 远程性能监控

> **状态**：待开始（S6 已完成，Tab 入口已就绪；可与 S7 并行）
> **前置**：[Sprint 5](./sprint-05-ssh-terminal.md)（SSH exec 能力；Perl ✅ 已完成）+ [Sprint 4](./sprint-04-settings-io.md)（监控间隔与阈值已持久化）
> **目标**：MonitorCollector + tokio interval 采集 + gpui Canvas 折线图 + Top 进程 + Kill
>
> **S5 已就绪的接入点**：
> - `SshSession::session_clone()` 可在采集线程建立独立 channel 执行 `exec`
> - `MonitorSample` / `MonitorAlert` 事件已定义
> - **Tab 入口已就绪（S6 5.4）**：`views/tab_bar.rs` + `MainWindow.tabs`（`TabKind::Monitor` 已预留）
>
> **S4 已就绪的接入点**：
> - `Settings.monitor_interval_secs` / `monitor_cpu_threshold` / `monitor_mem_threshold` 已在设置 → 监控 Tab 持久化 → 采集器直接读取，不再新增设置入口
> - `MonitorSample` / `MonitorAlert` 事件已在 `event.rs` 定义
> - 告警 Toast 可复用 S4 自绘 Modal/卡片样式

---

### 8.1 MonitorCollector

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 8.1.1 | 0.5h | `crates/rterm-monitor/src/collector.rs` | `pub struct MonitorCollector { samples: HashMap<String, VecDeque<ResourceSample>>, session_id }` + `pub struct ResourceSample { cpu_usage, mem_pct, swap_pct, disk_pct, net_rx_kbps, net_tx_kbps, load_1, load_5, timestamp }` | 能构造 |
| 8.1.2 | 0.5h | `crates/rterm-monitor/src/collector.rs` | 单行采集脚本（Linux）：`cat /proc/stat; cat /proc/meminfo; df -P / \| tail -1; cat /proc/net/dev; uptime` → 解析 | 对真实服务器能返回 ResourceSample |
| 8.1.3 | 0.25h | `crates/rterm-monitor/src/collector.rs` | macOS 兼容脚本：sysctl + vm_stat + netstat -ib（cfg target_os） | macOS 也能出数据 |
| 8.1.4 | 0.5h | `crates/rterm-monitor/src/collector.rs` | `pub fn start(session_id, interval, ssh_session, thresholds) -> Task<()>` — interval / 阈值由调用方从 `Settings.monitor_interval_secs` / `monitor_cpu_threshold` / `monitor_mem_threshold` 传入（默认 2s / 90% / 85%）→ tokio interval → exec → parse → push 60 点 VecDeque → broadcast MonitorSample | 默认 2 秒一次；设置改间隔后新采集器生效 |
| 8.1.5 | 0.25h | `crates/rterm-monitor/src/collector.rs` | Top 进程采集：`ps -eo pid,user,%cpu,%mem,comm --sort=-%cpu \| head -20` → Vec<TopProcess> | 能出 Top 进程 |
| 8.1.6 | 0.25h | `crates/rterm-monitor/src/collector.rs` | 阈值告警：rolling_avg(last 5) vs `Settings` 阈值 → broadcast MonitorAlert + 系统通知 | 把 CPU 阈值调到 10% → 触发 Toast |

### 8.2 Monitor View

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 8.2.1 | 0.5h | `crates/rterm-app/src/views/monitor.rs` | 6 个卡片（CPU/MEM/SWAP/DISK/NET/LOAD）+ Canvas 折线图（60 点 + 2px 线 + 网格） | 折线实时跳动 |
| 8.2.2 | 0.25h | `crates/rterm-app/src/views/monitor.rs` | 暂停 / 恢复 / 重置按钮 | 能暂停采集 |
| 8.2.3 | 0.5h | `crates/rterm-app/src/views/monitor.rs` | Top 进程表 + 列头排序 + 右键 Kill / Force Kill（二次确认弹窗） | Kill PID 生效 |

### 8.3 验收

| # | 粒度 | 验收 |
|---|------|------|
| 8.3.1 | 0.25h | Monitor Tab 打开 → 2 秒内 6 张折线图 → `md5sum /dev/zero &` → CPU 飙升 → 红线闪烁 + Toast → Kill → 回落 |
