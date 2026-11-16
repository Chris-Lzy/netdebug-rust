//! 网络调试助手 —— 入口。
//!
//! 全 Rust 编写（egui/eframe），没有 HTML/CSS/JS，没有 webview，没有 Tauri。
//! 窗口默认 1280×800；高 DPI 小屏下首帧自动缩回屏幕内（见 app.rs）。

// release 双击 exe 只出 GUI，不弹控制台；debug 保留控制台看日志
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod bus;
mod config;
mod industrial;
mod log;
mod modbus;
mod nic_backend;
mod pages;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("网络调试助手")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([480.0, 320.0]),
        ..Default::default()
    };
    eframe::run_native(
        "网络调试助手",
        options,
        Box::new(|cc| Ok(Box::new(app::NetDebugApp::new(cc)))),
    )
}
