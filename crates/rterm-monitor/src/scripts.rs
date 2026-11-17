//! 远端采集脚本 — 单次 exec 输出多个带标记的段落。
//!
//! 标记格式 `@@RT_<SECTION>@@`，解析器按标记切段，避免逐项 exec 带来
//! 的往返开销；脚本本身只用 POSIX sh + 常见 coreutils / BSD 工具。

/// Linux：/proc + df + ps。
pub const LINUX_SCRIPT: &str = r#"
echo @@RT_STAT@@
head -n 1 /proc/stat
echo @@RT_MEM@@
cat /proc/meminfo
echo @@RT_DISK@@
df -P / 2>/dev/null | tail -n 1
echo @@RT_NET@@
cat /proc/net/dev
echo @@RT_LOAD@@
cat /proc/loadavg
echo @@RT_PS@@
ps -eo pid=,user=,%cpu=,%mem=,comm= --sort=-%cpu 2>/dev/null | head -n 20
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
