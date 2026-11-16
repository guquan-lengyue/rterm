//! theme.rs — 全局主题（颜色 + 间距 + 圆角）
//!
//! 集中管理所有 UI 视觉常量。换主题 = 换一个 Theme 实现。
//!
//! 设计原则：
//! - **语义化命名**：`surface_bg` / `text_primary` / `border_subtle`，
//!   组件只关心"我是什么元素"，不关心颜色的具体 hex 值
//! - **背景 / 文字严格配对**：每个 surface 有对应的 text，确保对比度
//! - **gpui 0.3 类型**：直接返回 `gpui::Rgba` / `gpui::Pixels` 避免每个文件 import

use std::sync::Arc;

use gpui::{rgb, Rgba};

// ===========================================================================
// Theme trait — 换主题只需 impl 这个 trait
// ===========================================================================

pub trait Theme: 'static + Send + Sync {
    /// 主题名（显示在设置里）
    fn name(&self) -> &'static str;

    // --- 背景层（从底到顶 z-order） ---

    /// 最底层应用背景
    fn app_bg(&self) -> Rgba;
    /// 侧边栏背景
    fn sidebar_bg(&self) -> Rgba;
    /// 工具栏 / 标题栏背景
    fn toolbar_bg(&self) -> Rgba;
    /// 卡片 / Dialog 主体背景
    fn card_bg(&self) -> Rgba;
    /// Modal 标题栏 / 底部按钮栏
    fn card_header_bg(&self) -> Rgba;
    /// 列表行选中高亮
    fn row_selected_bg(&self) -> Rgba;
    /// 列表行 hover 高亮
    fn row_hover_bg(&self) -> Rgba;
    /// 输入框背景
    fn input_bg(&self) -> Rgba;

    // --- 文字（每个 surface 对应一个 text） ---

    fn text_on_app_bg(&self) -> Rgba;
    fn text_on_sidebar_bg(&self) -> Rgba;
    fn text_on_toolbar_bg(&self) -> Rgba;
    fn text_on_card_bg(&self) -> Rgba;
    fn text_on_card_header_bg(&self) -> Rgba;
    fn text_on_row_selected_bg(&self) -> Rgba;
    /// 次级文字（副标题 / hint）
    fn text_subtle(&self) -> Rgba;
    /// 占位符文字（非常淡）
    fn text_placeholder(&self) -> Rgba;
    /// 按钮文字（通常白色）
    fn text_on_primary_btn(&self) -> Rgba;
    fn text_on_secondary_btn(&self) -> Rgba;

    // --- 边框 ---

    fn border_subtle(&self) -> Rgba;
    fn border_medium(&self) -> Rgba;
    fn border_input_idle(&self) -> Rgba;
    fn border_input_focused(&self) -> Rgba;

    // --- 品牌色 ---

    fn primary(&self) -> Rgba;
    fn primary_hover(&self) -> Rgba;
    fn secondary_btn_bg(&self) -> Rgba;
    fn secondary_btn_hover(&self) -> Rgba;
    fn danger(&self) -> Rgba;
    /// 成功状态色（测试连接通过等）
    fn success(&self) -> Rgba;

    // --- Modal ---

    /// Modal 半透明遮罩（黑色 + alpha）
    fn modal_overlay(&self) -> Rgba;
}

// ===========================================================================
// Catppuccin Mocha — 默认深色主题
// ===========================================================================

pub struct CatppuccinMocha;

impl Theme for CatppuccinMocha {
    fn name(&self) -> &'static str { "Catppuccin Mocha" }

    // 背景层
    fn app_bg(&self) -> Rgba          { rgb(0x1e1e1e) }
    fn sidebar_bg(&self) -> Rgba      { rgb(0x181825) }
    fn toolbar_bg(&self) -> Rgba      { rgb(0x181825) }
    fn card_bg(&self) -> Rgba         { rgb(0x1e1e2e) }
    fn card_header_bg(&self) -> Rgba  { rgb(0x181825) }
    fn row_selected_bg(&self) -> Rgba { rgb(0x313244) }
    fn row_hover_bg(&self) -> Rgba    { rgb(0x313244) }
    fn input_bg(&self) -> Rgba        { rgb(0x1e1e2e) }

    // 文字
    fn text_on_app_bg(&self) -> Rgba          { rgb(0xcdd6f4) }
    fn text_on_sidebar_bg(&self) -> Rgba      { rgb(0xcdd6f4) }
    fn text_on_toolbar_bg(&self) -> Rgba      { rgb(0x9399b2) }
    fn text_on_card_bg(&self) -> Rgba         { rgb(0xcdd6f4) }
    fn text_on_card_header_bg(&self) -> Rgba  { rgb(0xcdd6f4) }
    fn text_on_row_selected_bg(&self) -> Rgba { rgb(0xcdd6f4) }
    fn text_subtle(&self) -> Rgba             { rgb(0x7f849c) }
    fn text_placeholder(&self) -> Rgba        { rgb(0x6c7086) }
    fn text_on_primary_btn(&self) -> Rgba     { rgb(0xffffff) }
    fn text_on_secondary_btn(&self) -> Rgba   { rgb(0xcdd6f4) }

    // 边框
    fn border_subtle(&self) -> Rgba      { rgb(0x313244) }
    fn border_medium(&self) -> Rgba      { rgb(0x45475a) }
    fn border_input_idle(&self) -> Rgba  { rgb(0x45475a) }
    fn border_input_focused(&self) -> Rgba { rgb(0x007acc) }

    // 品牌色
    fn primary(&self) -> Rgba              { rgb(0x007acc) }
    fn primary_hover(&self) -> Rgba        { rgb(0x2096e0) }
    fn secondary_btn_bg(&self) -> Rgba      { rgb(0x313244) }
    fn secondary_btn_hover(&self) -> Rgba  { rgb(0x45475a) }
    fn danger(&self) -> Rgba               { rgb(0xe64553) }
    fn success(&self) -> Rgba              { rgb(0xa6e3a1) }

    // Modal
    fn modal_overlay(&self) -> Rgba { rgb(0x66000000) }
}

// ===========================================================================
// Windows 11 Light — 浅色主题（未来用）
// ===========================================================================

pub struct WindowsLight;

impl Theme for WindowsLight {
    fn name(&self) -> &'static str { "Windows 11 Light" }

    fn app_bg(&self) -> Rgba          { rgb(0xf3f3f3) }
    fn sidebar_bg(&self) -> Rgba      { rgb(0xffffff) }
    fn toolbar_bg(&self) -> Rgba      { rgb(0xffffff) }
    fn card_bg(&self) -> Rgba         { rgb(0xffffff) }
    fn card_header_bg(&self) -> Rgba  { rgb(0xf8f8f8) }
    fn row_selected_bg(&self) -> Rgba { rgb(0xe5e5e5) }
    fn row_hover_bg(&self) -> Rgba    { rgb(0xebebeb) }
    fn input_bg(&self) -> Rgba        { rgb(0xffffff) }

    fn text_on_app_bg(&self) -> Rgba          { rgb(0x1a1a1a) }
    fn text_on_sidebar_bg(&self) -> Rgba      { rgb(0x1a1a1a) }
    fn text_on_toolbar_bg(&self) -> Rgba      { rgb(0x444444) }
    fn text_on_card_bg(&self) -> Rgba         { rgb(0x1a1a1a) }
    fn text_on_card_header_bg(&self) -> Rgba  { rgb(0x1a1a1a) }
    fn text_on_row_selected_bg(&self) -> Rgba { rgb(0x1a1a1a) }
    fn text_subtle(&self) -> Rgba             { rgb(0x666666) }
    fn text_placeholder(&self) -> Rgba        { rgb(0x999999) }
    fn text_on_primary_btn(&self) -> Rgba     { rgb(0xffffff) }
    fn text_on_secondary_btn(&self) -> Rgba   { rgb(0x1a1a1a) }

    fn border_subtle(&self) -> Rgba      { rgb(0xe0e0e0) }
    fn border_medium(&self) -> Rgba      { rgb(0xd0d0d0) }
    fn border_input_idle(&self) -> Rgba  { rgb(0xcccccc) }
    fn border_input_focused(&self) -> Rgba { rgb(0x0078d4) }

    fn primary(&self) -> Rgba              { rgb(0x0078d4) }
    fn primary_hover(&self) -> Rgba        { rgb(0x106ebe) }
    fn secondary_btn_bg(&self) -> Rgba      { rgb(0xf0f0f0) }
    fn secondary_btn_hover(&self) -> Rgba  { rgb(0xe0e0e0) }
    fn danger(&self) -> Rgba               { rgb(0xd13438) }
    fn success(&self) -> Rgba              { rgb(0x0f7b0f) }

    fn modal_overlay(&self) -> Rgba { rgb(0x66000000) }
}

// ===========================================================================
// Dracula — 深紫主题（Sprint 4）
// ===========================================================================

pub struct Dracula;

impl Theme for Dracula {
    fn name(&self) -> &'static str { "Dracula" }

    fn app_bg(&self) -> Rgba          { rgb(0x282a36) }
    fn sidebar_bg(&self) -> Rgba      { rgb(0x21222c) }
    fn toolbar_bg(&self) -> Rgba      { rgb(0x21222c) }
    fn card_bg(&self) -> Rgba         { rgb(0x282a36) }
    fn card_header_bg(&self) -> Rgba  { rgb(0x21222c) }
    fn row_selected_bg(&self) -> Rgba { rgb(0x44475a) }
    fn row_hover_bg(&self) -> Rgba    { rgb(0x3a3d4d) }
    fn input_bg(&self) -> Rgba        { rgb(0x21222c) }

    fn text_on_app_bg(&self) -> Rgba          { rgb(0xf8f8f2) }
    fn text_on_sidebar_bg(&self) -> Rgba      { rgb(0xf8f8f2) }
    fn text_on_toolbar_bg(&self) -> Rgba      { rgb(0xb8bfe0) }
    fn text_on_card_bg(&self) -> Rgba         { rgb(0xf8f8f2) }
    fn text_on_card_header_bg(&self) -> Rgba  { rgb(0xf8f8f2) }
    fn text_on_row_selected_bg(&self) -> Rgba { rgb(0xf8f8f2) }
    fn text_subtle(&self) -> Rgba             { rgb(0x9aa3c7) }
    fn text_placeholder(&self) -> Rgba        { rgb(0x6272a4) }
    fn text_on_primary_btn(&self) -> Rgba     { rgb(0x282a36) }
    fn text_on_secondary_btn(&self) -> Rgba   { rgb(0xf8f8f2) }

    fn border_subtle(&self) -> Rgba      { rgb(0x44475a) }
    fn border_medium(&self) -> Rgba      { rgb(0x6272a4) }
    fn border_input_idle(&self) -> Rgba  { rgb(0x44475a) }
    fn border_input_focused(&self) -> Rgba { rgb(0xbd93f9) }

    fn primary(&self) -> Rgba              { rgb(0xbd93f9) }
    fn primary_hover(&self) -> Rgba        { rgb(0xcaa9fa) }
    fn secondary_btn_bg(&self) -> Rgba      { rgb(0x44475a) }
    fn secondary_btn_hover(&self) -> Rgba  { rgb(0x565a72) }
    fn danger(&self) -> Rgba               { rgb(0xff5555) }
    fn success(&self) -> Rgba              { rgb(0x50fa7b) }

    fn modal_overlay(&self) -> Rgba { rgb(0x66000000) }
}

// ===========================================================================
// One Dark — Atom 风格（Sprint 4）
// ===========================================================================

pub struct OneDark;

impl Theme for OneDark {
    fn name(&self) -> &'static str { "One Dark" }

    fn app_bg(&self) -> Rgba          { rgb(0x282c34) }
    fn sidebar_bg(&self) -> Rgba      { rgb(0x21252b) }
    fn toolbar_bg(&self) -> Rgba      { rgb(0x21252b) }
    fn card_bg(&self) -> Rgba         { rgb(0x282c34) }
    fn card_header_bg(&self) -> Rgba  { rgb(0x21252b) }
    fn row_selected_bg(&self) -> Rgba { rgb(0x3e4451) }
    fn row_hover_bg(&self) -> Rgba    { rgb(0x333842) }
    fn input_bg(&self) -> Rgba        { rgb(0x21252b) }

    fn text_on_app_bg(&self) -> Rgba          { rgb(0xabb2bf) }
    fn text_on_sidebar_bg(&self) -> Rgba      { rgb(0xabb2bf) }
    fn text_on_toolbar_bg(&self) -> Rgba      { rgb(0x9da5b4) }
    fn text_on_card_bg(&self) -> Rgba         { rgb(0xabb2bf) }
    fn text_on_card_header_bg(&self) -> Rgba  { rgb(0xabb2bf) }
    fn text_on_row_selected_bg(&self) -> Rgba { rgb(0xd7dae0) }
    fn text_subtle(&self) -> Rgba             { rgb(0x7f8796) }
    fn text_placeholder(&self) -> Rgba        { rgb(0x5c6370) }
    fn text_on_primary_btn(&self) -> Rgba     { rgb(0xffffff) }
    fn text_on_secondary_btn(&self) -> Rgba   { rgb(0xabb2bf) }

    fn border_subtle(&self) -> Rgba      { rgb(0x3e4451) }
    fn border_medium(&self) -> Rgba      { rgb(0x4b5263) }
    fn border_input_idle(&self) -> Rgba  { rgb(0x3e4451) }
    fn border_input_focused(&self) -> Rgba { rgb(0x61afef) }

    fn primary(&self) -> Rgba              { rgb(0x61afef) }
    fn primary_hover(&self) -> Rgba        { rgb(0x7bbff5) }
    fn secondary_btn_bg(&self) -> Rgba      { rgb(0x3e4451) }
    fn secondary_btn_hover(&self) -> Rgba  { rgb(0x4b5263) }
    fn danger(&self) -> Rgba               { rgb(0xe06c75) }
    fn success(&self) -> Rgba              { rgb(0x98c379) }

    fn modal_overlay(&self) -> Rgba { rgb(0x66000000) }
}

// ===========================================================================
// TerminalPalette — 终端专用调色板（Sprint 6）
//
// UI 主题只覆盖控件颜色；终端需要独立的 fg / bg / ANSI 16 色 / 光标色。
// 每个内置 UI 主题对应一套终端调色板，`terminal_palette()` 按主题名取用。
// ===========================================================================

#[derive(Clone, Copy, Debug)]
pub struct TerminalPalette {
    pub fg: Rgba,
    pub bg: Rgba,
    /// ANSI 0–15：normal 0–7，bright 8–15
    pub ansi16: [Rgba; 16],
    pub cursor: Rgba,
}

impl TerminalPalette {
    pub fn selection_bg(&self) -> Rgba {
        self.bg.blend(self.fg.opacity(0.28))
    }
}

/// Catppuccin Mocha 终端配色
pub fn catppuccin_terminal() -> TerminalPalette {
    TerminalPalette {
        fg: rgb(0xcdd6f4),
        bg: rgb(0x1e1e2e),
        cursor: rgb(0xf5e0dc),
        ansi16: [
            rgb(0x45475a), rgb(0xf38ba8), rgb(0xa6e3a1), rgb(0xf9e2af),
            rgb(0x89b4fa), rgb(0xf5c2e7), rgb(0x94e2d5), rgb(0xbac2de),
            rgb(0x585b70), rgb(0xf38ba8), rgb(0xa6e3a1), rgb(0xf9e2af),
            rgb(0x89b4fa), rgb(0xf5c2e7), rgb(0x94e2d5), rgb(0xa6adc8),
        ],
    }
}

/// Windows 11 Light 终端配色（Campbell）
pub fn windows_light_terminal() -> TerminalPalette {
    TerminalPalette {
        fg: rgb(0x1a1a1a),
        bg: rgb(0xffffff),
        cursor: rgb(0x0078d4),
        ansi16: [
            rgb(0x0c0c0c), rgb(0xc50f1f), rgb(0x13a10e), rgb(0xc19c00),
            rgb(0x0037da), rgb(0x881798), rgb(0x3a96dd), rgb(0xcccccc),
            rgb(0x767676), rgb(0xe74856), rgb(0x16c60c), rgb(0xf9f1a5),
            rgb(0x3b78ff), rgb(0xb4009e), rgb(0x61d6d6), rgb(0xf2f2f2),
        ],
    }
}

/// Dracula 终端配色
pub fn dracula_terminal() -> TerminalPalette {
    TerminalPalette {
        fg: rgb(0xf8f8f2),
        bg: rgb(0x282a36),
        cursor: rgb(0xf8f8f2),
        ansi16: [
            rgb(0x21222c), rgb(0xff5555), rgb(0x50fa7b), rgb(0xf1fa8c),
            rgb(0xbd93f9), rgb(0xff79c6), rgb(0x8be9fd), rgb(0xf8f8f2),
            rgb(0x6272a4), rgb(0xff6e6e), rgb(0x69ff94), rgb(0xffffa5),
            rgb(0xd6acff), rgb(0xff92df), rgb(0xa4ffff), rgb(0xffffff),
        ],
    }
}

/// One Dark 终端配色
pub fn one_dark_terminal() -> TerminalPalette {
    TerminalPalette {
        fg: rgb(0xabb2bf),
        bg: rgb(0x282c34),
        cursor: rgb(0x528bff),
        ansi16: [
            rgb(0x282c34), rgb(0xe06c75), rgb(0x98c379), rgb(0xe5c07b),
            rgb(0x61afef), rgb(0xc678dd), rgb(0x56b6c2), rgb(0xabb2bf),
            rgb(0x5c6370), rgb(0xe06c75), rgb(0x98c379), rgb(0xe5c07b),
            rgb(0x61afef), rgb(0xc678dd), rgb(0x56b6c2), rgb(0xffffff),
        ],
    }
}

/// 按 UI 主题名取终端调色板；未知名回退 Catppuccin Mocha。
pub fn terminal_palette(theme_name: &str) -> TerminalPalette {
    match theme_name {
        "Catppuccin Mocha" => catppuccin_terminal(),
        "Windows 11 Light" => windows_light_terminal(),
        "Dracula" => dracula_terminal(),
        "One Dark" => one_dark_terminal(),
        _ => catppuccin_terminal(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_ui_theme_has_terminal_palette() {
        for theme in all_themes() {
            let palette = terminal_palette(theme.name());
            assert_ne!(palette.fg, palette.bg, "{} fg/bg 不应相同", theme.name());
            assert_eq!(palette.ansi16.len(), 16);
        }
    }

    #[test]
    fn palette_follows_theme_name() {
        let dracula = terminal_palette("Dracula");
        let one_dark = terminal_palette("One Dark");
        assert_eq!(dracula.bg, rgb(0x282a36));
        assert_eq!(one_dark.bg, rgb(0x282c34));
        assert_ne!(dracula.bg, one_dark.bg);
    }

    #[test]
    fn unknown_theme_falls_back_to_catppuccin() {
        let unknown = terminal_palette("No Such Theme");
        let catppuccin = terminal_palette("Catppuccin Mocha");
        assert_eq!(unknown.bg, catppuccin.bg);
        assert_eq!(unknown.fg, catppuccin.fg);
    }

    #[test]
    fn selection_is_blended_between_fg_and_bg() {
        let palette = catppuccin_terminal();
        let selection = palette.selection_bg();
        assert_ne!(selection, palette.bg);
        assert_ne!(selection, palette.fg);
    }
}

// ===========================================================================
// 全局访问入口
// ===========================================================================

/// 默认主题（可通过 AppState 切换）
pub fn default_theme() -> CatppuccinMocha { CatppuccinMocha }

/// 所有内置主题（设置界面按顺序展示）
pub fn all_themes() -> Vec<Arc<dyn Theme>> {
    vec![
        Arc::new(CatppuccinMocha),
        Arc::new(WindowsLight),
        Arc::new(Dracula),
        Arc::new(OneDark),
    ]
}

/// 按名称取主题；未知名回退到默认主题。
pub fn theme_by_name(name: &str) -> Arc<dyn Theme> {
    all_themes()
        .into_iter()
        .find(|t| t.name() == name)
        .unwrap_or_else(|| Arc::new(CatppuccinMocha))
}
