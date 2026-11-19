//! 构建脚本：把 Windows 图标资源嵌入 rterm-app.exe。
//!
//! Windows 对没有 `WM_SETICON` 的窗口使用 exe 的第一个图标组，
//! 因此嵌入后窗口 / 任务栏 / 资源管理器都会显示应用图标。

fn main() {
    println!("cargo:rerun-if-changed=assets/rterm.rc");
    println!("cargo:rerun-if-changed=assets/rterm.ico");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("assets/rterm.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("嵌入 Windows 图标资源失败");
    }
}
