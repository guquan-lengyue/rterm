//! theme.rs — 全局主题（颜色 + 间距 + 圆角）
//!
//! 集中管理所有 UI 视觉常量。换主题 = 换一个 Theme 实现。
//!
//! 设计原则：
//! - **语义化命名**：`surface_bg` / `text_primary` / `border_subtle`，
//!   组件只关心"我是什么元素"，不关心颜色的具体 hex 值
//! - **背景 / 文字严格配对**：每个 surface 有对应的 text，确保对比度
//! - **gpui 0.3 类型**：直接返回 `gpui::Rgba` / `gpui::Pixels` 避免每个文件 import

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
// 全局访问入口
// ===========================================================================

/// 默认主题（可通过 AppState 切换）
pub fn default_theme() -> CatppuccinMocha { CatppuccinMocha }
