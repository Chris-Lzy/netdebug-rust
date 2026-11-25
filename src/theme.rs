//! UI 配色：Muse 风格（深色暖调）。
//!
//! 只管颜色，不管布局：间距、圆角、字号、控件位置一律不动，
//! 全部在 `app.rs` 里原样保留。所有页面都走 `app.rs` 的 helper
//!（`card_frame` / `primary` / `section` / `status`）和这里的常量，
//! 所以改这一处就改了全站，不用逐页翻颜色。
//!
//! Muse 方式的三条取舍：
//! 1. 暖底代替冷底 —— 原版是偏蓝的冷黑（#0a0e13），长时间看日志
//!    偏冷发青；这里用暖炭色打底，纸面是暖米白，眼睛没那么累。
//! 2. 一个主强调色 —— 原版青绿 #2dd4bf 很亮、很“工具感”；
//!    改成暖琥珀 #e8a33d，只用在主按钮、选中态、链接和提示上，
//!    其余界面全部用中性暖灰，彩色不抢戏。
//! 3. 语义色只表状态 —— TX 蓝 / RX 绿 / ERR 红各自只负责一种含义，
//!    全部降了饱和度，和琥珀主色拉开距离，扫一眼不会认错。

use egui::Color32;

/// 底色：暖炭黑（主窗口面板）。
pub const BACKGROUND: Color32 = Color32::from_rgb(0x14, 0x12, 0x10);
/// 卡片底色（窗口、卡片）。
pub const CARD_FILL: Color32 = Color32::from_rgb(0x1d, 0x1a, 0x16);
/// 最深底色（日志区等需要沉下去的地方）。
pub const EXTREME_BG: Color32 = Color32::from_rgb(0x0e, 0x0c, 0x0a);
/// 边框。
pub const BORDER: Color32 = Color32::from_rgb(0x2e, 0x29, 0x24);
/// 悬停时的边框（比默认边框亮一档）。
pub const BORDER_STRONG: Color32 = Color32::from_rgb(0x45, 0x3d, 0x33);

/// 主文字：暖米白（不用纯白，纯白在深底上发刺）。
pub const TEXT: Color32 = Color32::from_rgb(0xf3, 0xee, 0xe6);
/// 次要文字：暖灰（小节标题、时间戳、说明）。
pub const MUTED: Color32 = Color32::from_rgb(0xa3, 0x9c, 0x90);
/// 更弱的文字（占位、脚注）。
#[allow(dead_code)]
pub const FAINT: Color32 = Color32::from_rgb(0x7d, 0x76, 0x6b);

/// 主强调色：暖琥珀（主按钮、选中、链接、提示）。
pub const ACCENT: Color32 = Color32::from_rgb(0xe8, 0xa3, 0x3d);
/// 压在强调色上的文字：深棕（琥珀底 + 深字，对比度才够）。
pub const ON_ACCENT: Color32 = Color32::from_rgb(0x24, 0x17, 0x08);

/// 成功 / 接收（RX）：柔和的绿。
pub const OK: Color32 = Color32::from_rgb(0x5d, 0xbb, 0x8d);
/// 发送（TX）：柔和的蓝，和绿拉开色相，扫一眼能分清。
pub const TX: Color32 = Color32::from_rgb(0x7f, 0xa8, 0xe6);
/// 错误：柔和的陶土红（不用正红，长时间看不刺眼）。
pub const ERR: Color32 = Color32::from_rgb(0xe5, 0x6b, 0x5d);
/// 警告：复用主强调色琥珀，不再单开一种黄。
pub const WARN: Color32 = ACCENT;

/// 按钮 / 输入框 idle 底色。
const WIDGET_IDLE: Color32 = Color32::from_rgb(0x26, 0x22, 0x1d);
/// 按钮 / 输入框悬停底色。
const WIDGET_HOVER: Color32 = Color32::from_rgb(0x33, 0x2c, 0x24);
/// 按钮按下底色：深琥珀棕，配琥珀描边，按下去有反馈又不闪。
const WIDGET_ACTIVE: Color32 = Color32::from_rgb(0x3d, 0x2f, 0x1a);

/// 整套深色暖调 visuals。圆角、间距一律不动：
/// 圆角沿用原来的 8，控件圆角 6 留在 `app.rs` 的 style 里。
pub fn muse_visuals() -> egui::Visuals {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = BACKGROUND;
    visuals.window_fill = CARD_FILL;
    visuals.extreme_bg_color = EXTREME_BG;
    visuals.faint_bg_color = CARD_FILL;
    visuals.code_bg_color = EXTREME_BG;
    visuals.override_text_color = Some(TEXT);
    visuals.hyperlink_color = ACCENT;
    visuals.warn_fg_color = WARN;
    visuals.error_fg_color = ERR;
    visuals.selection.bg_fill = ACCENT;
    visuals.selection.stroke = egui::Stroke::new(1.0, ON_ACCENT);
    visuals.window_stroke = egui::Stroke::new(1.0, BORDER);
    visuals.window_corner_radius = egui::CornerRadius::same(8);
    visuals.menu_corner_radius = egui::CornerRadius::same(8);

    let w = &mut visuals.widgets;
    w.noninteractive.bg_fill = CARD_FILL;
    w.noninteractive.bg_stroke = egui::Stroke::new(1.0, BORDER);
    w.noninteractive.fg_stroke = egui::Stroke::new(1.0, MUTED);
    w.inactive.bg_fill = WIDGET_IDLE;
    w.inactive.bg_stroke = egui::Stroke::new(1.0, BORDER);
    w.inactive.fg_stroke = egui::Stroke::new(1.0, TEXT);
    w.hovered.bg_fill = WIDGET_HOVER;
    w.hovered.bg_stroke = egui::Stroke::new(1.0, BORDER_STRONG);
    w.hovered.fg_stroke = egui::Stroke::new(1.0, TEXT);
    w.active.bg_fill = WIDGET_ACTIVE;
    w.active.bg_stroke = egui::Stroke::new(1.0, ACCENT);
    w.active.fg_stroke = egui::Stroke::new(1.0, TEXT);
    w.open.bg_fill = WIDGET_IDLE;
    w.open.bg_stroke = egui::Stroke::new(1.0, ACCENT);
    w.open.fg_stroke = egui::Stroke::new(1.0, TEXT);
    visuals
}

/// 把 Muse 配色应用到 egui 上下文（锁死深色，不跟随系统浅色）。
pub fn apply(ctx: &egui::Context) {
    ctx.set_visuals_of(egui::Theme::Dark, muse_visuals());
    ctx.options_mut(|o| o.theme_preference = egui::ThemePreference::Dark);
}

/// 相对亮度（WCAG 2.x 定义），只给下面的对比度测试用。
#[cfg(test)]
fn luminance(c: Color32) -> f32 {
    let f = |v: u8| {
        let s = v as f32 / 255.0;
        if s <= 0.03928 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
}

/// 两个颜色的对比度（WCAG 2.x）。正文 ≥ 4.5 才算可读。
#[cfg(test)]
fn contrast(a: Color32, b: Color32) -> f32 {
    let (hi, lo) = {
        let (x, y) = (luminance(a), luminance(b));
        if x > y {
            (x, y)
        } else {
            (y, x)
        }
    };
    (hi + 0.05) / (lo + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_text_is_readable_on_background() {
        assert!(contrast(TEXT, BACKGROUND) >= 7.0);
        assert!(contrast(MUTED, BACKGROUND) >= 4.5);
    }

    #[test]
    fn button_text_is_readable_on_accent() {
        assert!(contrast(ON_ACCENT, ACCENT) >= 4.5);
    }

    #[test]
    fn log_colors_are_readable_on_log_background() {
        assert!(contrast(TX, EXTREME_BG) >= 4.5);
        assert!(contrast(OK, EXTREME_BG) >= 4.5);
        assert!(contrast(ERR, EXTREME_BG) >= 4.5);
        assert!(contrast(MUTED, EXTREME_BG) >= 4.5);
    }
}
