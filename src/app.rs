//! 外壳：顶栏 + 页签栏 + 页签内容区。
//!
//! 页签可拖出成独立窗口再收回：用 eframe 多视口（`show_viewport_immediate`），
//! 弹出窗口和主窗口共享同一份 `Shared` 状态；
//! 如果后端不支持多视口，egui 会自动退化成主窗口内的嵌入窗口，功能不丢。
//! 关掉弹出窗口时自动收回页签。
//!
//! 注意：eframe 0.37 的 `App` trait 没有 `update`，只有 `ui(&mut Ui, &mut Frame)`。

use crate::bus::{Bus, Event};
use crate::config::{self, AppConfig};
use crate::log::{LogLevel, LogStore};
use crate::pages::{self, Page};
use egui::{Color32, ViewportBuilder, ViewportId};
use std::collections::HashSet;
use std::time::{Duration, Instant};

// ---- 原版 web 风格配色 ----
pub const ACCENT: Color32 = Color32::from_rgb(0x2d, 0xd4, 0xbf);
pub const CARD_FILL: Color32 = Color32::from_rgb(0x0f, 0x14, 0x1b);
pub const BORDER: Color32 = Color32::from_rgb(0x1f, 0x29, 0x37);
pub const MUTED: Color32 = Color32::from_rgb(0x9c, 0xa3, 0xaf);
pub const OK: Color32 = Color32::from_rgb(0x34, 0xd3, 0x99);
pub const ERR: Color32 = Color32::from_rgb(0xf8, 0x71, 0x71);
const ON_ACCENT: Color32 = Color32::from_rgb(0x06, 0x2a, 0x26);

/// 卡片容器：深色底 + 细边框 + 圆角，原版 web 的 panel 观感。
pub fn card_frame() -> egui::Frame {
    egui::Frame::group(&egui::Style::default())
        .fill(CARD_FILL)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::same(12))
}

/// 主操作按钮（连接/发送/执行）：青绿底深色字。
pub fn primary(text: &str) -> egui::Button<'static> {
    egui::Button::new(egui::RichText::new(text).strong().color(ON_ACCENT)).fill(ACCENT)
}

/// 卡片内的小节标题。
pub fn section(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).size(13.0).strong().color(MUTED));
}

/// 状态灯：● 绿 / ○ 灰。
pub fn status(ui: &mut egui::Ui, on: bool, on_text: &str, off_text: &str) {
    if on {
        ui.colored_label(OK, format!("● {on_text}"));
    } else {
        ui.colored_label(MUTED, format!("○ {off_text}"));
    }
}

/// 所有页面共享的状态：配置、日志、总线、提示、连接状态。
pub struct Shared {
    pub config: AppConfig,
    pub log: LogStore,
    pub bus: Bus,
    /// 右下角提示：(出现时间, 文本)，4 秒后自动消失
    pub toasts: Vec<(Instant, String)>,
    /// 是否已连接（框架阶段恒为 false，业务逻辑接上后由连接代码改）
    pub connected: bool,
}

impl Shared {
    pub fn new(config: AppConfig) -> Self {
        let mut log = LogStore::with_default_capacity();
        log.push(
            LogLevel::Sys,
            "网络调试助手 v0.1.0 已启动（框架版，未接后端）",
        );
        Self {
            config,
            log,
            bus: Bus::new(),
            toasts: Vec::new(),
            connected: false,
        }
    }

    /// 右下角弹一条提示（最多留 5 条）。
    pub fn toast(&mut self, text: impl Into<String>) {
        self.toasts.push((Instant::now(), text.into()));
        if self.toasts.len() > 5 {
            self.toasts.remove(0);
        }
    }

    /// 记一条日志。
    pub fn log(&mut self, level: LogLevel, text: impl Into<String>) {
        self.log.push(level, text);
    }
}

pub struct NetDebugApp {
    pages: Vec<Box<dyn Page>>,
    active: usize,
    detached: HashSet<&'static str>,
    shared: Shared,
    /// 页签拖拽累计距离（拖出阈值用）
    drag_accum: f32,
    /// 首帧窗口自适应是否已判定（只判定一次，之后尊重用户手动调整）
    window_fitted: bool,
}

impl NetDebugApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // ---- 深色主题（NetDebug 原版配色）----
        // 必须锁死 ThemePreference::Dark：默认 System 会跟随 Windows 浅色主题，
        // 把这里设的深色 visuals 覆盖掉。
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = Color32::from_rgb(0x0a, 0x0e, 0x13);
        visuals.window_fill = Color32::from_rgb(0x0f, 0x14, 0x1b);
        visuals.extreme_bg_color = Color32::from_rgb(0x07, 0x0a, 0x0f);
        visuals.selection.bg_fill = ACCENT;
        visuals.selection.stroke = egui::Stroke::new(1.0, Color32::from_rgb(0x06, 0x2a, 0x26));
        visuals.hyperlink_color = ACCENT;
        visuals.window_stroke = egui::Stroke::new(1.0, BORDER);
        visuals.window_corner_radius = egui::CornerRadius::same(8);
        visuals.menu_corner_radius = egui::CornerRadius::same(8);
        visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, BORDER);
        cc.egui_ctx.set_visuals_of(egui::Theme::Dark, visuals);
        cc.egui_ctx
            .options_mut(|o| o.theme_preference = egui::ThemePreference::Dark);
        cc.egui_ctx.style_mut_of(egui::Theme::Dark, |style| {
            style.spacing.item_spacing = egui::Vec2::new(10.0, 8.0);
            style.spacing.button_padding = egui::Vec2::new(12.0, 6.0);
            let r = egui::CornerRadius::same(6);
            style.visuals.widgets.noninteractive.corner_radius = r;
            style.visuals.widgets.inactive.corner_radius = r;
            style.visuals.widgets.hovered.corner_radius = r;
            style.visuals.widgets.active.corner_radius = r;
            style.visuals.widgets.open.corner_radius = r;
        });

        let config = config::load();
        let pages = pages::all_pages();
        let active = pages
            .iter()
            .position(|p| p.id() == config.last_page)
            .unwrap_or(0);

        Self {
            pages,
            active,
            detached: HashSet::new(),
            shared: Shared::new(config),
            drag_accum: 0.0,
            window_fitted: false,
        }
    }

    /// 保存配置（退出时调用；失败不弹框，记一条日志）。
    fn save_config(&mut self) {
        if let Some(page) = self.pages.get(self.active) {
            self.shared.config.last_page = page.id().to_string();
        }
        if let Err(e) = config::save(&self.shared.config) {
            self.shared
                .log
                .push(LogLevel::Err, format!("保存配置失败：{e}"));
        }
    }

    /// 顶栏：品牌 + 版本 + 状态灯。
    fn show_top_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("top_bar").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("⇄ 网络调试助手")
                        .size(18.0)
                        .strong()
                        .color(Color32::WHITE),
                );
                ui.colored_label(MUTED, egui::RichText::new("v0.1.0").size(12.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    status(ui, self.shared.connected, "已连接", "未连接");
                    ui.colored_label(MUTED, egui::RichText::new("框架版").size(12.0));
                });
            });
        });
    }

    /// 页签栏：点切换、点 ⧉ 拖出、拖拽页签也能拖出；已弹出的页签显示收回按钮。
    fn show_tab_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("tab_bar").show(ui, |ui| {
            ui.horizontal(|ui| {
                for i in 0..self.pages.len() {
                    let id = self.pages[i].id();
                    let title = self.pages[i].title().to_string();
                    let icon = self.pages[i].icon().to_string();
                    if self.detached.contains(id) {
                        ui.colored_label(
                            Color32::from_rgb(0x9c, 0xa3, 0xaf),
                            format!("{icon} {title}（已弹出）"),
                        );
                        if ui.button("收回").clicked() {
                            self.detached.remove(id);
                        }
                        ui.separator();
                        continue;
                    }
                    let resp = ui.selectable_label(self.active == i, format!("{icon} {title}"));
                    if resp.clicked() {
                        self.active = i;
                    }
                    // 拖拽页签超过 60px 即拖出
                    if resp.dragged() {
                        self.drag_accum += resp.drag_delta().length();
                        if self.drag_accum > 60.0 {
                            self.detached.insert(id);
                            self.drag_accum = 0.0;
                        }
                    }
                    if resp.drag_stopped() {
                        self.drag_accum = 0.0;
                    }
                    // 点 ⧉ 拖出
                    if ui.button("⧉").on_hover_text("拖出为独立窗口").clicked() {
                        self.detached.insert(id);
                    }
                    ui.separator();
                }
            });
        });
    }

    /// 主窗口内容区：当前页签（已弹出的显示占位 + 收回按钮）。
    fn show_content(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default().show(ui, |ui| {
            let active = self.active;
            let page_id = self.pages[active].id();
            if self.detached.contains(page_id) {
                ui.vertical_centered(|ui| {
                    ui.add_space(40.0);
                    ui.heading(format!(
                        "「{}」已弹出到独立窗口",
                        self.pages[active].title()
                    ));
                    ui.label("独立窗口和主窗口共享同一份数据。");
                    if ui.button("⬅ 收回页签").clicked() {
                        self.detached.remove(page_id);
                    }
                });
                return;
            }
            let page = &mut self.pages[active];
            page.show(ui, &mut self.shared);
        });
    }

    /// 已弹出的页签：每个开一个独立 OS 窗口，关窗口自动收回。
    fn show_detached_viewports(&mut self, ctx: &egui::Context) {
        let ids: Vec<&'static str> = self.detached.iter().copied().collect();
        for id in ids {
            let Some(idx) = self.pages.iter().position(|p| p.id() == id) else {
                continue;
            };
            let title = self.pages[idx].title().to_string();
            let viewport_id = ViewportId::from_hash_of(id);
            let builder = ViewportBuilder::default()
                .with_title(format!("网络调试助手 - {title}"))
                .with_inner_size([900.0, 620.0])
                .with_min_inner_size([640.0, 420.0]);
            let page = &mut self.pages[idx];
            let shared = &mut self.shared;
            let mut dock = false;
            ctx.show_viewport_immediate(viewport_id, builder, |ui, _class| {
                egui::Panel::top("dock_bar").show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if ui.button("⬅ 收回页签").clicked() {
                            dock = true;
                        }
                        ui.strong(&title);
                        ui.colored_label(
                            Color32::from_rgb(0x9c, 0xa3, 0xaf),
                            "独立窗口，与主窗口共享同一份数据",
                        );
                    });
                });
                egui::CentralPanel::default().show(ui, |ui| {
                    page.show(ui, shared);
                });
                // 用户点了窗口右上角 ×：自动收回页签
                if ui.input(|i| i.viewport().close_requested()) {
                    dock = true;
                }
            });
            if dock {
                self.detached.remove(id);
            }
        }
    }

    /// 首帧检查窗口是否超出屏幕（高 DPI 缩放下逻辑尺寸换算成物理像素
    /// 可能比屏幕大，eframe 还会恢复上次退出时的几何），超出就缩回屏幕内。
    /// 只判定一次，之后尊重用户手动拖拽/最大化。
    fn fit_window_to_screen(&mut self, ctx: &egui::Context) {
        if self.window_fitted {
            return;
        }
        let (monitor, screen, inner) = ctx.input(|i| {
            (
                i.viewport().monitor_size,
                i.raw.screen_rect,
                i.viewport().inner_rect,
            )
        });
        let screen = monitor
            .map(|s| egui::Rect::from_min_size(egui::Pos2::ZERO, s))
            .or(screen);
        let (Some(screen), Some(inner)) = (screen, inner) else {
            return;
        };
        self.window_fitted = true;
        let off_screen = inner.left() < screen.left()
            || inner.top() < screen.top()
            || inner.right() > screen.right()
            || inner.bottom() > screen.bottom();
        if !off_screen {
            return;
        }
        let size = egui::Vec2::new(
            (screen.width() - 16.0).clamp(320.0, 1280.0),
            (screen.height() - 48.0).clamp(240.0, 800.0),
        );
        ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(egui::Vec2::new(
            size.x.min(480.0),
            size.y.min(320.0),
        )));
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(
            screen.left() + 8.0,
            screen.top() + 8.0,
        )));
    }

    /// 右下角提示（主窗口底部）。
    fn show_toasts(&mut self, ui: &mut egui::Ui) {
        let now = Instant::now();
        self.shared
            .toasts
            .retain(|(t, _)| now.duration_since(*t) < Duration::from_secs(4));
        if let Some((_, text)) = self.shared.toasts.last() {
            let text = text.clone();
            egui::Panel::bottom("toast_bar").show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(Color32::from_rgb(0x2d, 0xd4, 0xbf), "ⓘ");
                    ui.label(text);
                });
            });
        }
    }
}

impl eframe::App for NetDebugApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.fit_window_to_screen(ui.ctx());

        // 总线事件：后台线程发的 toast/日志在这里落到界面上
        for event in self.shared.bus.drain() {
            match event {
                Event::Toast(t) => self.shared.toast(t),
                Event::Log(level, text) => self.shared.log.push(level, text),
            }
        }

        self.show_top_bar(ui);
        self.show_tab_bar(ui);
        self.show_content(ui);
        let ctx = ui.ctx().clone();
        self.show_detached_viewports(&ctx);
        self.show_toasts(ui);
    }

    fn on_exit(&mut self) {
        self.save_config();
    }
}
