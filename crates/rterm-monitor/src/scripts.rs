//! 远端采集脚本 — 单次 exec 输出多个带标记的段落。
//!
//! 标记格式 `@@RT_<SECTION>@@`，解析器按标记切段，避免逐项 exec 带来
//! 的往返开销；脚本本身只用 POSIX sh + 常见 coreutils / BSD 工具。

/// Linux：/proc + df。
///
/// 进程列表不用 `ps`：ps 内部同样遍历 `/proc`，但会额外做 uid→name 解析、
/// 且 `%cpu` 是"进程生命周期平均"而非瞬时值。这里用单个 awk 进程直读
/// `/proc/<pid>/stat` 与 `/proc/<pid>/status`：
/// - 只有一次 fork（awk 自身），不会为每个进程 fork 一个 `cat`
/// - utime/stime 与 `/proc/stat` 同源同刻度，可做精确的 tick 增量
/// - uid→用户名在 awk 里用 `/etc/passwd` 预加载映射，不额外调用
///
/// 输出列（TAB 分隔）：`pid user utime stime rss_kb comm`
///
/// 末尾的 `/dev/null` 是兜底：若 glob 一个都没匹配上，awk 会退化为读 stdin
/// 并阻塞到通道关闭；给一个恒存在的空文件可避免这种挂起。
pub const LINUX_SCRIPT: &str = r#"
echo @@RT_STAT@@
grep '^cpu' /proc/stat
echo @@RT_MEM@@
cat /proc/meminfo
echo @@RT_DISK@@
df -P / 2>/dev/null | tail -n 1
echo @@RT_NET@@
cat /proc/net/dev
echo @@RT_LOAD@@
cat /proc/loadavg
echo @@RT_PS@@
awk '
BEGIN {
  while ((getline line < "/etc/passwd") > 0) {
    n = split(line, f, ":")
    if (n >= 3) users[f[3]] = f[1]
  }
  close("/etc/passwd")
}
{
  pid = $1
  s = $0
  open_paren = index(s, "(")
  if (open_paren == 0) next
  pos = open_paren
  while ((i = index(substr(s, pos + 1), ")")) > 0) pos += i
  comm = substr(s, open_paren + 1, pos - open_paren - 1)
  split(substr(s, pos + 1), a, " ")
  if (a[1] == "Z") next
  uid = "-"; rss = 0
  while ((getline uline < ("/proc/" pid "/status")) > 0) {
    if (uline ~ /^Uid:/) { split(uline, uf); uid = uf[2] }
    else if (uline ~ /^VmRSS:/) { split(uline, vf); rss = vf[2] }
    if (uid != "-" && rss != 0) break
  }
  close("/proc/" pid "/status")
  user = (uid in users) ? users[uid] : uid
  printf "%s\t%s\t%s\t%s\t%s\t%s\n", pid, user, a[12], a[13], rss, comm
}
' /proc/[0-9]*/stat /dev/null 2>/dev/null
"#;

/// macOS：sysctl + vm_stat + netstat + ps。
pub const MACOS_SCRIPT: &str = r#"
echo @@RT_CPU@@
sysctl -n hw.ncpu 2>/dev/null
ps -A -o %cpu= 2>/dev/null | awk '{s+=$1} END {printf "%.2f\n", s+0}'
echo @@RT_MEM@@
sysctl -n hw.memsize 2>/dev/null
vm_stat 2>/dev/null
sysctl -n vm.swapusage 2>/dev/null
echo @@RT_DISK@@
df -P / 2>/dev/null | tail -n 1
echo @@RT_NET@@
netstat -ibn 2>/dev/null | awk '/^en[0-9]/ && $7 ~ /^[0-9]+$/ {rx+=$7; tx+=$10} END {print rx+0, tx+0}'
echo @@RT_LOAD@@
sysctl -n vm.loadavg 2>/dev/null
echo @@RT_PS@@
ps -Ao pid=,user=,%cpu=,%mem=,comm= -r 2>/dev/null | head -n 20
"#;

/// 启动期探测远端系统类型。
pub const PROBE_SCRIPT: &str = "uname -s 2>/dev/null || echo Linux";

/// 采集脚本标记前缀。
pub const MARKER_PREFIX: &str = "@@RT_";
