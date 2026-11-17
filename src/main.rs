//! Netdebug —— 入口（版本号见 Cargo.toml，界面用 `VERSION` 取它）。
//!
//! 全 Rust 编写（egui/eframe），没有 HTML/CSS/JS，没有 webview，没有 Tauri。
//! 窗口默认 1280×800；高 DPI 小屏下首帧自动缩回屏幕内（见 app.rs）。

// 双击 exe 只出 GUI，不弹控制台：debug 和 release 都一样。
// 原先只在 release 下关控制台，debug 构建（cargo run / target\debug
// 下的 exe）双击会先弹一个命令窗口；用户要求直接只有应用，所以全关。
// 代价：debug 下崩溃也不会在控制台留任何提示；需要看 panic 信息时，
// 把这行临时改回 `#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]` 再编。
#![windows_subsystem = "windows"]

mod app;
mod bus;
mod config;
mod debug_backend;
mod industrial;
mod log;
mod modbus;
mod nic_backend;
mod pages;
mod rtu;
mod slave_backend;
mod theme;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Netdebug")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([480.0, 320.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Netdebug",
        options,
        Box::new(|cc| Ok(Box::new(app::NetDebugApp::new(cc)))),
    )
}
