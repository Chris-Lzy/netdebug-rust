//! 调试页：NetDebug 原版布局（egui 重写）。
//!
//! 左：连接面板（协议下拉，字段随协议换）；右：工具条 + 日志 + 发送区。
//! 本文件只画界面，所有联网/串口/Modbus 逻辑都是 TODO stub，
//! 点按钮会弹“未接后端”提示，不会假装已经连上。

use crate::app::Shared;
use crate::pages::Page;

/// 6 种协议（和 NetDebug 原版一致）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    TcpClient,
    TcpServer,
    Udp,
    WebSocket,
    ModbusTcp,
    ModbusRtu,
}

/// 框架阶段预留：`key`/`from_key` 在配置存取接上后使用。
#[allow(dead_code)]
impl Protocol {
    pub const ALL: [Protocol; 6] = [
        Protocol::TcpClient,
        Protocol::TcpServer,
        Protocol::Udp,
        Protocol::WebSocket,
        Protocol::ModbusTcp,
        Protocol::ModbusRtu,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Protocol::TcpClient => "TCP Client",
            Protocol::TcpServer => "TCP Server",
            Protocol::Udp => "UDP",
            Protocol::WebSocket => "WebSocket",
            Protocol::ModbusTcp => "Modbus TCP",
            Protocol::ModbusRtu => "Modbus RTU",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Protocol::TcpClient => "tcp-client",
            Protocol::TcpServer => "tcp-server",
            Protocol::Udp => "udp",
            Protocol::WebSocket => "websocket",
            Protocol::ModbusTcp => "modbus-tcp",
            Protocol::ModbusRtu => "modbus-rtu",
        }
    }

    pub fn from_key(key: &str) -> Self {
        match key {
            "tcp-server" => Protocol::TcpServer,
            "udp" => Protocol::Udp,
            "websocket" => Protocol::WebSocket,
            "modbus-tcp" => Protocol::ModbusTcp,
            "modbus-rtu" => Protocol::ModbusRtu,
            _ => Protocol::TcpClient,
        }
    }
}

/// Modbus 功能码（原版支持的 8 个）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionCode {
    ReadCoils,              // 01
    ReadDiscrete,           // 02
    ReadHolding,            // 03
    ReadInput,              // 04
    WriteSingleCoil,        // 05
    WriteSingleRegister,    // 06
    WriteMultipleCoils,     // 15
    WriteMultipleRegisters, // 16
}

impl FunctionCode {
    pub const ALL: [FunctionCode; 8] = [
        FunctionCode::ReadCoils,
        FunctionCode::ReadDiscrete,
        FunctionCode::ReadHolding,
        FunctionCode::ReadInput,
        FunctionCode::WriteSingleCoil,
        FunctionCode::WriteSingleRegister,
        FunctionCode::WriteMultipleCoils,
        FunctionCode::WriteMultipleRegisters,
    ];

    pub fn label(self) -> &'static str {
        match self {
            FunctionCode::ReadCoils => "01 读线圈",
            FunctionCode::ReadDiscrete => "02 读离散输入",
            FunctionCode::ReadHolding => "03 读保持寄存器",
            FunctionCode::ReadInput => "04 读输入寄存器",
            FunctionCode::WriteSingleCoil => "05 写单个线圈",
            FunctionCode::WriteSingleRegister => "06 写单个寄存器",
            FunctionCode::WriteMultipleCoils => "15 写多个线圈",
            FunctionCode::WriteMultipleRegisters => "16 写多个寄存器",
        }
    }
}

pub struct DebugPage {
    protocol: Protocol,
    remote_host: String,
    remote_port: u16,
    listen_port: u16,
    ws_url: String,
    serial_port: String,
    baud: u32,
    data_bits: u8,
    parity: String,
    stop_bits: String,
    unit_id: u8,
    function_code: FunctionCode,
    address: u16,
    quantity: u16,
    poll_enabled: bool,
    poll_ms: u64,
    hex_mode: bool,
    show_timestamp: bool,
    auto_scroll: bool,
    raw_input: String,
    eol: String,
    timed_send: bool,
    timed_ms: u64,
    composer_tab: usize, // 0 原始数据，1 Modbus
    profile: String,
    frame_count: u64,
}

impl Default for DebugPage {
    fn default() -> Self {
        Self {
            protocol: Protocol::TcpClient,
            remote_host: "127.0.0.1".to_string(),
            remote_port: 502,
            listen_port: 9000,
            ws_url: "ws://127.0.0.1:8080".to_string(),
            serial_port: "COM1".to_string(),
            baud: 9600,
            data_bits: 8,
            parity: "None".to_string(),
            stop_bits: "1".to_string(),
            unit_id: 1,
            function_code: FunctionCode::ReadHolding,
            address: 0,
            quantity: 10,
            poll_enabled: false,
            poll_ms: 1000,
            hex_mode: true,
            show_timestamp: true,
            auto_scroll: true,
            raw_input: String::new(),
            eol: "无".to_string(),
            timed_send: false,
            timed_ms: 1000,
            composer_tab: 0,
            profile: "默认".to_string(),
            frame_count: 0,
        }
    }
}

impl DebugPage {
    /// 地址历史下拉：选中即回填输入框。
    fn history_combo(ui: &mut egui::Ui, id: &str, history: &[String], current: &mut String) {
        egui::ComboBox::from_id_salt(id)
            .selected_text("")
            .width(34.0)
            .show_ui(ui, |ui| {
                ui.set_min_width(180.0);
                if history.is_empty() {
                    ui.colored_label(crate::app::MUTED, "暂无历史记录");
                }
                for h in history {
                    if ui.selectable_label(false, h.as_str()).clicked() {
                        *current = h.clone();
                    }
                }
            });
    }

    /// 连接面板里的协议相关字段。
    fn show_connection_fields(&mut self, ui: &mut egui::Ui, shared: &mut Shared) {
        match self.protocol {
            Protocol::TcpClient | Protocol::Udp | Protocol::ModbusTcp => {
                ui.horizontal(|ui| {
                    ui.label("远程地址");
                    let history = shared.config.debug.host_history.clone();
                    Self::history_combo(ui, "host_history", &history, &mut self.remote_host);
                    let resp = ui.text_edit_singleline(&mut self.remote_host);
                    if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        remember_address(shared, false, &self.remote_host.clone());
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("远程端口");
                    ui.add(egui::DragValue::new(&mut self.remote_port).range(1..=65535));
                });
                if self.protocol == Protocol::ModbusTcp {
                    ui.horizontal(|ui| {
                        ui.label("从站地址");
                        ui.add(egui::DragValue::new(&mut self.unit_id).range(1..=247));
                    });
                }
            }
            Protocol::TcpServer => {
                ui.horizontal(|ui| {
                    ui.label("监听端口");
                    ui.add(egui::DragValue::new(&mut self.listen_port).range(1..=65535));
                });
                ui.colored_label(
                    crate::app::MUTED,
                    egui::RichText::new("监听地址：0.0.0.0（以后可改只绑 127.0.0.1）").size(12.0),
                );
            }
            Protocol::WebSocket => {
                ui.horizontal(|ui| {
                    ui.label("WS 地址");
                    let history = shared.config.debug.ws_history.clone();
                    Self::history_combo(ui, "ws_history", &history, &mut self.ws_url);
                    let resp = ui.text_edit_singleline(&mut self.ws_url);
                    if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        remember_address(shared, true, &self.ws_url.clone());
                    }
                });
            }
            Protocol::ModbusRtu => {
                ui.horizontal(|ui| {
                    ui.label("串口");
                    ui.text_edit_singleline(&mut self.serial_port);
                });
                ui.horizontal(|ui| {
                    ui.label("波特率");
                    egui::ComboBox::from_id_salt("rtu_baud")
                        .selected_text(self.baud.to_string())
                        .show_ui(ui, |ui| {
                            for b in [1200, 2400, 4800, 9600, 19200, 38400, 57600, 115200] {
                                ui.selectable_value(&mut self.baud, b, b.to_string());
                            }
                        });
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("数据位");
                    egui::ComboBox::from_id_salt("rtu_bits")
                        .selected_text(self.data_bits.to_string())
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.data_bits, 8, "8");
                            ui.selectable_value(&mut self.data_bits, 7, "7");
                        });
                    ui.label("校验");
                    egui::ComboBox::from_id_salt("rtu_parity")
                        .selected_text(self.parity.clone())
                        .show_ui(ui, |ui| {
                            for p in ["None", "Even", "Odd"] {
                                ui.selectable_value(&mut self.parity, p.to_string(), p);
                            }
                        });
                    ui.label("停止位");
                    egui::ComboBox::from_id_salt("rtu_stop")
                        .selected_text(self.stop_bits.clone())
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.stop_bits, "1".to_string(), "1");
                            ui.selectable_value(&mut self.stop_bits, "2".to_string(), "2");
                        });
                });
            }
        }
    }

    /// 发送区：原始数据页签。
    fn show_raw_composer(&mut self, ui: &mut egui::Ui, shared: &mut Shared) {
        ui.horizontal(|ui| {
            ui.label("数据");
            let w = (ui.available_width() - 76.0).max(120.0);
            ui.add(
                egui::TextEdit::singleline(&mut self.raw_input)
                    .desired_width(w)
                    .font(egui::FontId::monospace(13.0)),
            );
            if ui.add(crate::app::primary("发送")).clicked() {
                // TODO: 接入发送逻辑（TCP/UDP/WS/RTU 按当前协议发）。
                shared.toast("未接后端：发送功能待实现");
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut self.hex_mode, "HEX");
            ui.label("结尾符");
            egui::ComboBox::from_id_salt("eol")
                .selected_text(self.eol.clone())
                .show_ui(ui, |ui| {
                    for e in ["无", "\\n", "\\r\\n"] {
                        ui.selectable_value(&mut self.eol, e.to_string(), e);
                    }
                });
            ui.checkbox(&mut self.timed_send, "定时发送");
            ui.add_enabled(
                self.timed_send,
                egui::DragValue::new(&mut self.timed_ms)
                    .range(10..=60000)
                    .suffix(" ms"),
            );
            ui.colored_label(
                crate::app::MUTED,
                egui::RichText::new("Ctrl+Enter 发送").size(12.0),
            );
        });
    }

    /// 发送区：Modbus 页签。
    fn show_modbus_composer(&mut self, ui: &mut egui::Ui, shared: &mut Shared) {
        ui.horizontal_wrapped(|ui| {
            ui.label("从站地址");
            ui.add(egui::DragValue::new(&mut self.unit_id).range(1..=247));
            ui.label("功能码");
            egui::ComboBox::from_id_salt("fc")
                .selected_text(self.function_code.label())
                .show_ui(ui, |ui| {
                    for fc in FunctionCode::ALL {
                        ui.selectable_value(&mut self.function_code, fc, fc.label());
                    }
                });
            ui.label("地址");
            ui.add(egui::DragValue::new(&mut self.address).range(0..=65535));
            ui.label("数量");
            ui.add(egui::DragValue::new(&mut self.quantity).range(1..=2000));
            if ui.add(crate::app::primary("执行")).clicked() {
                // TODO: 接入 Modbus 主站请求（TCP MBAP / RTU CRC16）。
                shared.toast("未接后端：Modbus 请求待实现");
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut self.poll_enabled, "轮询");
            ui.add_enabled(
                self.poll_enabled,
                egui::DragValue::new(&mut self.poll_ms)
                    .range(50..=60000)
                    .suffix(" ms"),
            );
            ui.colored_label(
                crate::app::MUTED,
                egui::RichText::new("读上限：线圈 2000 / 寄存器 125").size(12.0),
            );
        });
    }
}

/// 地址入历史：去重、置顶、最多 10 条，立即落盘 config.json。
fn remember_address(shared: &mut Shared, ws: bool, value: &str) {
    let v = value.trim();
    if v.is_empty() {
        return;
    }
    let list = if ws {
        &mut shared.config.debug.ws_history
    } else {
        &mut shared.config.debug.host_history
    };
    list.retain(|h| h != v);
    list.insert(0, v.to_string());
    list.truncate(10);
    if let Err(e) = crate::config::save(&shared.config) {
        shared
            .log
            .push(crate::log::LogLevel::Err, format!("保存地址历史失败：{e}"));
    }
}

impl Page for DebugPage {
    fn id(&self) -> &'static str {
        "debug"
    }
    fn title(&self) -> &'static str {
        "调试"
    }
    fn icon(&self) -> &'static str {
        "⇄"
    }

    fn show(&mut self, ui: &mut egui::Ui, shared: &mut Shared) {
        // ---- 左：连接面板 ----
        let left_w = 318.0_f32.min(ui.available_width() * 0.45);
        egui::Panel::left("debug_left")
            .resizable(true)
            .default_size(left_w)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.add_space(4.0);
                    crate::app::section(ui, "连接");
                    ui.add_space(4.0);
                    crate::app::card_frame().show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label("协议");
                            egui::ComboBox::from_id_salt("protocol")
                                .selected_text(self.protocol.label())
                                .width(ui.available_width())
                                .show_ui(ui, |ui| {
                                    for p in Protocol::ALL {
                                        ui.selectable_value(&mut self.protocol, p, p.label());
                                    }
                                });
                        });
                        ui.add_space(6.0);
                        self.show_connection_fields(ui, shared);
                    });
                    ui.add_space(10.0);
                    crate::app::card_frame().show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label("配置档案");
                            egui::ComboBox::from_id_salt("profile")
                                .selected_text(self.profile.clone())
                                .width(ui.available_width())
                                .show_ui(ui, |ui| {
                                    for p in ["默认", "PLC-502", "夹爪设备"] {
                                        ui.selectable_value(&mut self.profile, p.to_string(), p);
                                    }
                                });
                        });
                    });
                    ui.add_space(10.0);
                    let w = ui.available_width();
                    let label = if shared.connected { "断开" } else { "连接" };
                    if ui
                        .add_sized([w, 34.0], crate::app::primary(label))
                        .clicked()
                    {
                        match self.protocol {
                            Protocol::TcpClient | Protocol::Udp | Protocol::ModbusTcp => {
                                remember_address(shared, false, &self.remote_host.clone());
                            }
                            Protocol::WebSocket => {
                                remember_address(shared, true, &self.ws_url.clone());
                            }
                            _ => {}
                        }
                        // TODO: 接入连接逻辑（5 秒超时、断线自动重连 1s→2s→5s）。
                        shared.toast("未接后端：连接功能待实现");
                    }
                    ui.add_space(6.0);
                    crate::app::status(ui, shared.connected, "已连接", "未连接");
                    ui.add_space(4.0);
                });
            });

        // ---- 右：工作区 ----
        egui::CentralPanel::default().show(ui, |ui| {
            // 发送区放底部
            egui::Panel::bottom("debug_composer")
                .resizable(true)
                .default_size(132.0)
                .show(ui, |ui| {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut self.composer_tab, 0, "原始数据");
                        ui.selectable_value(&mut self.composer_tab, 1, "Modbus");
                    });
                    ui.separator();
                    if self.composer_tab == 0 {
                        self.show_raw_composer(ui, shared);
                    } else {
                        self.show_modbus_composer(ui, shared);
                    }
                    ui.add_space(4.0);
                });

            // 工具条
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut self.hex_mode, "HEX");
                ui.checkbox(&mut self.show_timestamp, "时间戳");
                ui.checkbox(&mut self.auto_scroll, "自动滚动");
                ui.colored_label(
                    crate::app::MUTED,
                    format!("帧数：{}", self.frame_count),
                );
                if ui.button("清空").clicked() {
                    shared.log.clear();
                }
                if ui.button("保存").clicked() {
                    // TODO: 日志保存为 .log 文件。
                    shared.toast("未接后端：日志保存待实现");
                }
            });
            ui.add_space(4.0);

            // 日志区
            crate::app::card_frame().show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .stick_to_bottom(self.auto_scroll)
                    .show(ui, |ui| {
                        if shared.log.is_empty() {
                            ui.colored_label(
                                crate::app::MUTED,
                                "暂无数据（日志上限 3000 行，超了丢最旧）",
                            );
                        }
                        for entry in shared.log.entries() {
                            ui.horizontal(|ui| {
                                if self.show_timestamp {
                                    ui.monospace(
                                        egui::RichText::new(entry.time_string())
                                            .size(12.0)
                                            .color(crate::app::MUTED),
                                    );
                                }
                                ui.monospace(
                                    egui::RichText::new(entry.level.label())
                                        .size(12.0)
                                        .color(entry.level.color()),
                                );
                                ui.monospace(egui::RichText::new(entry.text.as_str()).size(12.5));
                            });
                        }
                    });
            });
        });
    }
}
