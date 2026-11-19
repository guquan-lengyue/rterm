//! 远端采集脚本 — 单次 exec 输出多个带标记的段落。
//!
//! 标记格式 `@@RT_<SECTION>@@`，解析器按标记切段，避免逐项 exec 带来
//! 的往返开销；脚本本身只用 POSIX sh + 常见 coreutils / BSD 工具。

/// Linux：/proc + df。
///
/// `df -Pk` 强制以 1 KiB 块输出，Linux 与 macOS 列格式一致，
/// 便于解析器统一换算成字节。
pub const LINUX_SCRIPT: &str = r#"
echo @@RT_STAT@@
grep '^cpu' /proc/stat
echo @@RT_MEM@@
cat /proc/meminfo
echo @@RT_DISK@@
df -Pk / 2>/dev/null | tail -n 1
echo @@RT_NET@@
cat /proc/net/dev
echo @@RT_LOAD@@
cat /proc/loadavg
"#;

/// macOS：sysctl + vm_stat + netstat + df。
pub const MACOS_SCRIPT: &str = r#"
echo @@RT_CPU@@
sysctl -n hw.ncpu 2>/dev/null
ps -A -o %cpu= 2>/dev/null | awk '{s+=$1} END {printf "%.2f\n", s+0}'
echo @@RT_MEM@@
sysctl -n hw.memsize 2>/dev/null
vm_stat 2>/dev/null
sysctl -n vm.swapusage 2>/dev/null
echo @@RT_DISK@@
df -Pk / 2>/dev/null | tail -n 1
echo @@RT_NET@@
netstat -ibn 2>/dev/null | awk '/^en[0-9]/ && $7 ~ /^[0-9]+$/ {rx+=$7; tx+=$10} END {print rx+0, tx+0}'
echo @@RT_LOAD@@
sysctl -n vm.loadavg 2>/dev/null
"#;

/// 启动期探测远端系统类型。
pub const PROBE_SCRIPT: &str = "uname -s 2>/dev/null || echo Linux";

/// 采集脚本标记前缀。
pub const MARKER_PREFIX: &str = "@@RT_";

/// 目录占用扫描脚本：列出 `path` 的直接子项（含文件）及其递归占用。
///
/// 每行输出 `d|f \t size_kb \t path`，`-x` 保证不跨文件系统
/// （避免把挂载点 / snap 等外部容量算进来）。
/// `du` 与 `-d 1` 在 GNU / BSD 上语义一致。
pub fn disk_scan_script(path: &str) -> String {
    format!(
        r#"echo @@RT_DU@@
du -a -x -k -d 1 {quoted} 2>/dev/null | while IFS="$(printf '\t')" read -r size p; do
  if [ -d "$p" ]; then kind=d; else kind=f; fi
  printf '%s\t%s\t%s\n' "$kind" "$size" "$p"
done
"#,
        quoted = shell_quote(path)
    )
}

/// POSIX shell 单引号转义，防止路径中的特殊字符注入命令。
fn shell_quote(raw: &str) -> String {
    format!("'{}'", raw.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_wraps_and_escapes() {
        assert_eq!(shell_quote("/var/log"), "'/var/log'");
        assert_eq!(shell_quote("/tmp/a b"), "'/tmp/a b'");
        assert_eq!(shell_quote("/tmp/it's"), r"'/tmp/it'\''s'");
    }

    #[test]
    fn disk_scan_script_quotes_path() {
        let script = disk_scan_script("/");
        assert!(script.contains("du -a -x -k -d 1 '/'"));
        assert!(script.contains("@@RT_DU@@"));
    }
}
