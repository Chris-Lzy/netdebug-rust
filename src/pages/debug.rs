//! 调试页：NetDebug 原版布局（egui 重写）。
//!
//! 左：连接面板（协议下拉，字段随协议换）；右：工具条 + 日志 + 发送区。
//! 六种协议都已接后端：TCP Client/Server、UDP、WebSocket 走
//! debug_backend.rs 的会话线程；Modbus TCP/RTU 直接用 modbus.rs
//! 的轮询线程（和主站页一样）。参数和档案存在 config.json。

use crate::app::Shared;
use crate::debug_backend::{self, DebugCmd, DebugEvent, DebugSession};
use crate::industrial::IntervalTimer;
use crate::log::LogLevel;
use crate::modbus;
use crate::pages::Page;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc::Receiver, Arc};
use std::time::Duration;

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

    pub fn code(self) -> u8 {
        match self {
            FunctionCode::ReadCoils => 1,
            FunctionCode::ReadDiscrete => 2,
            FunctionCode::ReadHolding => 3,
            FunctionCode::ReadInput => 4,
            FunctionCode::WriteSingleCoil => 5,
            FunctionCode::WriteSingleRegister => 6,
            FunctionCode::WriteMultipleCoils => 15,
            FunctionCode::WriteMultipleRegisters => 16,
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
    /// 原始协议的收发会话（TCP Client/Server、UDP、WebSocket）。
    session: Option<DebugSession>,
    /// Modbus 轮询（连接按钮 / 轮询勾选框共用）。
    modbus_poll_rx: Option<Receiver<modbus::MasterMsg>>,
    modbus_poll_stop: Option<Arc<AtomicBool>>,
    /// Modbus 单次执行。
    modbus_once_rx: Option<Receiver<modbus::MasterMsg>>,
    /// 配置里的参数是否已读进界面（只读一次）。
    loaded: bool,
    /// 断线自动重连（TCP Client / WebSocket）。
    auto_reconnect: bool,
    /// 写功能码的值（`1,0,1` / `100,200` / `0x0064` / `on|off`）。
    write_text: String,
    /// 保存档案时用的名字（选档案时自动填档案名）。
    profile_name: String,
    /// 定时发送的计时器（定时发送开着才有）。
    timed_timer: Option<IntervalTimer>,
    timed_ms_last: u64,
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
            session: None,
            modbus_poll_rx: None,
            modbus_poll_stop: None,
            modbus_once_rx: None,
            loaded: false,
            auto_reconnect: true,
            write_text: "1".to_string(),
            profile_name: "默认".to_string(),
            timed_timer: None,
            timed_ms_last: 0,
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
                    egui::RichText::new("监听地址：127.0.0.1（仅本机）").size(12.0),
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
            let history = shared.config.debug.send_history.clone();
            Self::history_combo(ui, "send_history", &history, &mut self.raw_input);
            let w = (ui.available_width() - 110.0).max(120.0);
            ui.add(
                egui::TextEdit::singleline(&mut self.raw_input)
                    .desired_width(w)
                    .font(egui::FontId::monospace(13.0)),
            );
            if ui.add(crate::app::primary("发送")).clicked() {
                self.send_raw(shared);
            }
            if ui.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::Enter)) {
                self.send_raw(shared);
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
            if modbus::is_write(self.fc_code()) {
                ui.label("值");
                ui.add(
                    egui::TextEdit::singleline(&mut self.write_text)
                        .desired_width(120.0)
                        .font(egui::FontId::monospace(13.0)),
                );
                ui.colored_label(
                    crate::app::MUTED,
                    egui::RichText::new("如 1,0,1 或 100,200 或 0x0064").size(12.0),
                );
            }
            if ui.add(crate::app::primary("执行")).clicked() {
                self.execute_modbus(shared);
            }
        });
        ui.horizontal_wrapped(|ui| {
            let resp = ui.checkbox(&mut self.poll_enabled, "轮询");
            if resp.changed() {
                if self.poll_enabled {
                    self.start_modbus_poll(shared);
                } else {
                    self.stop_modbus_poll(shared);
                }
            }
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

impl DebugPage {
    fn fc_code(&self) -> u8 {
        self.function_code.code()
    }

    /// 收到的 / 发出的原始字节格式化成一行日志（HEX 打空格，文本直接显示）。
    fn format_data(&self, dir: &str, who: Option<String>, bytes: &[u8]) -> String {
        let payload = if self.hex_mode {
            bytes
                .iter()
                .map(|b| format!("{b:02X}"))
                .collect::<Vec<_>>()
                .join(" ")
        } else {
            String::from_utf8_lossy(bytes).into_owned()
        };
        match who {
            Some(w) if !w.is_empty() => format!("{dir} {w} {payload}"),
            _ => format!("{dir} {payload}"),
        }
    }

    /// Modbus 读结果格式化成一行日志。
    fn format_modbus_read(&self, addr: u16, bits: &[bool], regs: &[u16]) -> String {
        if !regs.is_empty() {
            let values: Vec<String> = regs.iter().map(|v| v.to_string()).collect();
            format!("← 地址={addr} 寄存器：{}", values.join(", "))
        } else {
            let values: Vec<String> = bits
                .iter()
                .map(|b| if *b { "1" } else { "0" }.to_string())
                .collect();
            format!("← 地址={addr} 线圈：{}", values.join(", "))
        }
    }

    /// 收后台事件：原始会话 + Modbus 轮询 + Modbus 单次。
    fn drain(&mut self, shared: &mut Shared) {
        let mut session_stopped = false;
        if let Some(s) = &self.session {
            while let Ok(ev) = s.rx.try_recv() {
                match ev {
                    DebugEvent::Log(level, text) => shared.log.push(level, text),
                    DebugEvent::Data { from, bytes } => {
                        self.frame_count += 1;
                        let line = self.format_data("←", from, &bytes);
                        shared.log.push(LogLevel::Rx, line);
                    }
                    DebugEvent::Sent { to, bytes } => {
                        self.frame_count += 1;
                        let line = self.format_data("→", to, &bytes);
                        shared.log.push(LogLevel::Tx, line);
                    }
                    DebugEvent::Connected(c) => shared.connected = c,
                    DebugEvent::Stopped => session_stopped = true,
                }
            }
        }
        if session_stopped {
            self.session = None;
            shared.connected = false;
        }

        let mut poll_stopped = false;
        if let Some(rx) = &self.modbus_poll_rx {
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    modbus::MasterMsg::Log(text) => {
                        let level = if text.starts_with('✗') {
                            LogLevel::Err
                        } else {
                            LogLevel::Sys
                        };
                        shared.log.push(level, text);
                    }
                    modbus::MasterMsg::Read { addr, bits, regs } => {
                        self.frame_count += 1;
                        shared
                            .log
                            .push(LogLevel::Rx, self.format_modbus_read(addr, &bits, &regs));
                    }
                    modbus::MasterMsg::Stopped => poll_stopped = true,
                }
            }
        }
        if poll_stopped {
            self.modbus_poll_rx = None;
            self.modbus_poll_stop = None;
            self.poll_enabled = false;
            if matches!(self.protocol, Protocol::ModbusTcp | Protocol::ModbusRtu) {
                shared.connected = false;
            }
        }

        let mut once_done = false;
        if let Some(rx) = &self.modbus_once_rx {
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    modbus::MasterMsg::Log(text) => {
                        let level = if text.starts_with('✗') {
                            LogLevel::Err
                        } else {
                            LogLevel::Sys
                        };
                        shared.log.push(level, text);
                    }
                    modbus::MasterMsg::Read { addr, bits, regs } => {
                        self.frame_count += 1;
                        shared
                            .log
                            .push(LogLevel::Rx, self.format_modbus_read(addr, &bits, &regs));
                    }
                    modbus::MasterMsg::Stopped => once_done = true,
                }
            }
        }
        if once_done {
            self.modbus_once_rx = None;
        }
    }

    /// 断开：原始会话 + Modbus 轮询都停。
    fn disconnect(&mut self, shared: &mut Shared) {
        if let Some(s) = &self.session {
            s.stop.store(true, Ordering::Relaxed);
        }
        self.session = None;
        self.stop_modbus_poll(shared);
        shared.connected = false;
    }

    /// 当前输入框内容转成要发的字节（HEX 模式按十六进制解析，文本模式直接取字节，再加结尾符）。
    fn parse_raw_payload(&self) -> Result<Vec<u8>, String> {
        let mut bytes = if self.hex_mode {
            let cleaned: String = self
                .raw_input
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect();
            if cleaned.is_empty() {
                return Ok(Vec::new());
            }
            if cleaned.len() % 2 != 0 {
                return Err("HEX 格式不对：需要成对的十六进制字符".into());
            }
            let chars: Vec<char> = cleaned.chars().collect();
            let mut out = Vec::with_capacity(cleaned.len() / 2);
            for pair in chars.chunks(2) {
                let s: String = pair.iter().collect();
                match u8::from_str_radix(&s, 16) {
                    Ok(b) => out.push(b),
                    Err(_) => return Err(format!("HEX 格式不对：{s} 不是十六进制")),
                }
            }
            out
        } else {
            self.raw_input.clone().into_bytes()
        };
        match self.eol.as_str() {
            "\\n" => bytes.push(b'\n'),
            "\\r\\n" => bytes.extend_from_slice(b"\r\n"),
            _ => {}
        }
        Ok(bytes)
    }

    /// 发原始数据（TCP/UDP/WS 会话；Modbus 协议走下面的 Modbus 页签）。
    fn send_raw(&mut self, shared: &mut Shared) {
        let Some(s) = &self.session else {
            shared.toast("还没连接");
            return;
        };
        if matches!(self.protocol, Protocol::ModbusTcp | Protocol::ModbusRtu) {
            shared.toast("Modbus 协议请用下面的 Modbus 页签发");
            return;
        }
        let bytes = match self.parse_raw_payload() {
            Ok(b) => b,
            Err(e) => {
                shared.toast(e);
                return;
            }
        };
        if bytes.is_empty() {
            shared.toast("发送内容为空");
            return;
        }
        // 发过的内容进历史（最近在前，最多 20 条，立即落盘）。
        let shown = self.raw_input.trim().to_string();
        if !shown.is_empty() {
            let list = &mut shared.config.debug.send_history;
            list.retain(|h| h != &shown);
            list.insert(0, shown);
            list.truncate(20);
            if let Err(e) = crate::config::save(&shared.config) {
                shared
                    .log
                    .push(LogLevel::Err, format!("保存发送历史失败：{e}"));
            }
        }
        if s.cmd_tx.send(DebugCmd::Send(bytes)).is_err() {
            shared.toast("发送失败：会话已断");
        }
    }

    /// 按当前 Modbus 页签拼一个 MasterJob（once：单次还是轮询）。
    fn build_master_job(&self, once: bool) -> modbus::MasterJob {
        let fc = self.fc_code();
        let values = if modbus::is_write(fc) {
            modbus::parse_values(&self.write_text)
        } else {
            Vec::new()
        };
        modbus::MasterJob {
            host: self.remote_host.clone(),
            port: self.remote_port,
            unit: self.unit_id,
            fc,
            addr: self.address,
            qty: modbus::clamp_qty(fc, self.quantity),
            values,
            interval_ms: self.poll_ms.max(50),
            once,
        }
    }

    fn rtu_settings(&self) -> crate::rtu::SerialSettings {
        crate::rtu::SerialSettings {
            port: self.serial_port.trim().to_string(),
            baud: self.baud,
            data_bits: self.data_bits,
            parity: self.parity.clone(),
            stop_bits: self.stop_bits.clone(),
        }
    }

    /// Modbus 单次执行（TCP 或 RTU）。
    fn execute_modbus(&mut self, shared: &mut Shared) {
        if self.modbus_once_rx.is_some() {
            shared.toast("上一次单次执行还没回完");
            return;
        }
        if !matches!(self.protocol, Protocol::ModbusTcp | Protocol::ModbusRtu) {
            shared.toast("请先把协议切到 Modbus TCP 或 Modbus RTU");
            return;
        }
        let job = self.build_master_job(true);
        let stop = Arc::new(AtomicBool::new(false));
        let rx = match self.protocol {
            Protocol::ModbusTcp => modbus::spawn_poller(job, stop),
            Protocol::ModbusRtu => modbus::spawn_rtu_poller(job, self.rtu_settings(), stop),
            _ => unreachable!(),
        };
        self.modbus_once_rx = Some(rx);
    }

    /// 开始 Modbus 轮询（连接按钮和轮询勾选框共用）。
    fn start_modbus_poll(&mut self, shared: &mut Shared) {
        if self.modbus_poll_rx.is_some() {
            return;
        }
        if !matches!(self.protocol, Protocol::ModbusTcp | Protocol::ModbusRtu) {
            shared.toast("请先把协议切到 Modbus TCP 或 Modbus RTU");
            self.poll_enabled = false;
            return;
        }
        let job = self.build_master_job(false);
        let stop = Arc::new(AtomicBool::new(false));
        let rx = match self.protocol {
            Protocol::ModbusTcp => modbus::spawn_poller(job, stop.clone()),
            Protocol::ModbusRtu => modbus::spawn_rtu_poller(job, self.rtu_settings(), stop.clone()),
            _ => unreachable!(),
        };
        self.modbus_poll_stop = Some(stop);
        self.modbus_poll_rx = Some(rx);
        self.poll_enabled = true;
        shared.connected = true;
    }

    /// 停止 Modbus 轮询。
    fn stop_modbus_poll(&mut self, shared: &mut Shared) {
        if let Some(s) = &self.modbus_poll_stop {
            s.store(true, Ordering::Relaxed);
        }
        self.modbus_poll_stop = None;
        self.modbus_poll_rx = None;
        self.poll_enabled = false;
        if matches!(self.protocol, Protocol::ModbusTcp | Protocol::ModbusRtu) {
            shared.connected = false;
        }
    }

    /// 选中档案：把档案里的协议和地址填进界面。
    fn apply_profile(&mut self, shared: &Shared) {
        let Some(p) = shared
            .config
            .debug
            .profiles
            .iter()
            .find(|p| p.name == self.profile)
        else {
            return;
        };
        self.protocol = Protocol::from_key(&p.protocol);
        self.remote_host = p.remote_host.clone();
        self.remote_port = p.remote_port;
        self.listen_port = p.listen_port;
        self.ws_url = p.ws_url.clone();
        self.unit_id = p.unit_id;
        self.profile_name = p.name.clone();
    }

    /// 保存当前界面参数为档案（同名覆盖，没有就新增），立即落盘。
    fn save_profile(&mut self, shared: &mut Shared) {
        let name = self.profile_name.trim().to_string();
        if name.is_empty() {
            shared.toast("档案名不能为空");
            return;
        }
        let p = crate::config::DebugProfile {
            name: name.clone(),
            protocol: self.protocol.key().to_string(),
            remote_host: self.remote_host.clone(),
            remote_port: self.remote_port,
            listen_port: self.listen_port,
            ws_url: self.ws_url.clone(),
            unit_id: self.unit_id,
        };
        let profiles = &mut shared.config.debug.profiles;
        if let Some(old) = profiles.iter_mut().find(|x| x.name == name) {
            *old = p;
        } else {
            profiles.push(p);
        }
        self.profile = name.clone();
        match crate::config::save(&shared.config) {
            Ok(()) => shared.toast(format!("档案“{name}”已保存")),
            Err(e) => shared.toast(format!("保存档案失败：{e}")),
        }
    }

    /// 日志保存：写到 config.json 同目录，文件名带日期时间。
    fn save_log(shared: &Shared) -> Result<String, String> {
        let dir = crate::config::config_path()
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        let name = format!("netdebug-{}.log", now_stamp());
        let path = dir.join(name);
        let mut text = String::new();
        for e in shared.log.entries() {
            text.push_str(&format!(
                "[{}] {} {}\n",
                e.time_string(),
                e.level.label(),
                e.text
            ));
        }
        std::fs::write(&path, text).map_err(|e| e.to_string())?;
        Ok(path.to_string_lossy().into_owned())
    }
}

/// 当前时间 "YYYYMMDD-HHMMSS"（东八区；不用 chrono，自己算历法）。
fn now_stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        + 8 * 3600;
    let days = (secs / 86400) as i64;
    let secs_of_day = secs % 86400;
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}{:02}",
        secs_of_day / 3600,
        (secs_of_day / 60) % 60,
        secs_of_day % 60
    )
}

/// 天数（Unix 纪元起）→ 年月日（Howard Hinnant 算法）。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
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
        // 配置里的参数只读一次进界面（重启后还原上次的）。
        if !self.loaded {
            self.loaded = true;
            let cfg = &shared.config.debug;
            self.protocol = Protocol::from_key(&cfg.protocol);
            self.remote_host = cfg.remote_host.clone();
            self.remote_port = cfg.remote_port;
            self.listen_port = cfg.listen_port;
            self.ws_url = cfg.ws_url.clone();
            self.unit_id = cfg.unit_id;
            self.hex_mode = cfg.hex_mode;
            self.show_timestamp = cfg.show_timestamp;
            self.auto_scroll = cfg.auto_scroll;
            self.auto_reconnect = cfg.auto_reconnect;
        }
        self.drain(shared);
        // 定时发送（原始数据，计时器到期就发当前内容）。
        let timed_due = if self.timed_send
            && self.session.is_some()
            && !matches!(self.protocol, Protocol::ModbusTcp | Protocol::ModbusRtu)
        {
            if self.timed_timer.is_none() || self.timed_ms_last != self.timed_ms {
                self.timed_timer = Some(IntervalTimer::new(Duration::from_millis(
                    self.timed_ms.max(10),
                )));
                self.timed_ms_last = self.timed_ms;
            }
            self.timed_timer.as_mut().map(|t| t.due()).unwrap_or(false)
        } else {
            self.timed_timer = None;
            false
        };
        if timed_due {
            self.send_raw(shared);
        }
        // 界面参数同步回配置，退出时存 config.json。
        {
            let cfg = &mut shared.config.debug;
            cfg.protocol = self.protocol.key().to_string();
            cfg.remote_host = self.remote_host.clone();
            cfg.remote_port = self.remote_port;
            cfg.listen_port = self.listen_port;
            cfg.ws_url = self.ws_url.clone();
            cfg.unit_id = self.unit_id;
            cfg.hex_mode = self.hex_mode;
            cfg.show_timestamp = self.show_timestamp;
            cfg.auto_scroll = self.auto_scroll;
            cfg.auto_reconnect = self.auto_reconnect;
        }

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
                            let names: Vec<String> = shared
                                .config
                                .debug
                                .profiles
                                .iter()
                                .map(|p| p.name.clone())
                                .collect();
                            egui::ComboBox::from_id_salt("profile")
                                .selected_text(self.profile.clone())
                                .width(ui.available_width())
                                .show_ui(ui, |ui| {
                                    for n in &names {
                                        if ui
                                            .selectable_value(
                                                &mut self.profile,
                                                n.clone(),
                                                n.as_str(),
                                            )
                                            .clicked()
                                        {
                                            self.apply_profile(shared);
                                        }
                                    }
                                });
                        });
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.label("档案名");
                            ui.text_edit_singleline(&mut self.profile_name);
                            if ui.button("保存档案").clicked() {
                                self.save_profile(shared);
                            }
                        });
                    });
                    ui.add_space(10.0);
                    let w = ui.available_width();
                    let label = if shared.connected { "断开" } else { "连接" };
                    if ui
                        .add_sized([w, 34.0], crate::app::primary(label))
                        .clicked()
                    {
                        if shared.connected {
                            self.disconnect(shared);
                        } else {
                            match self.protocol {
                                Protocol::TcpClient | Protocol::Udp | Protocol::ModbusTcp => {
                                    remember_address(shared, false, &self.remote_host.clone());
                                }
                                Protocol::WebSocket => {
                                    remember_address(shared, true, &self.ws_url.clone());
                                }
                                _ => {}
                            }
                            match self.protocol {
                                Protocol::TcpClient => {
                                    match debug_backend::start_tcp_client(
                                        self.remote_host.clone(),
                                        self.remote_port,
                                        self.auto_reconnect,
                                    ) {
                                        Ok(s) => self.session = Some(s),
                                        Err(e) => {
                                            shared.toast(e.clone());
                                            shared.log.push(LogLevel::Err, e);
                                        }
                                    }
                                }
                                Protocol::TcpServer => {
                                    match debug_backend::start_tcp_server(self.listen_port) {
                                        Ok(s) => self.session = Some(s),
                                        Err(e) => {
                                            shared.toast(e.clone());
                                            shared.log.push(LogLevel::Err, e);
                                        }
                                    }
                                }
                                Protocol::Udp => {
                                    use std::net::ToSocketAddrs;
                                    let target = (self.remote_host.as_str(), self.remote_port)
                                        .to_socket_addrs()
                                        .ok()
                                        .and_then(|mut it| it.next());
                                    match debug_backend::start_udp(self.listen_port, target) {
                                        Ok(s) => self.session = Some(s),
                                        Err(e) => {
                                            shared.toast(e.clone());
                                            shared.log.push(LogLevel::Err, e);
                                        }
                                    }
                                }
                                Protocol::WebSocket => {
                                    if !self.ws_url.starts_with("ws://") {
                                        shared.toast("只支持 ws://，不支持 wss://（没开 TLS）");
                                    } else {
                                        match debug_backend::start_websocket(
                                            self.ws_url.clone(),
                                            self.auto_reconnect,
                                        ) {
                                            Ok(s) => self.session = Some(s),
                                            Err(e) => {
                                                shared.toast(e.clone());
                                                shared.log.push(LogLevel::Err, e);
                                            }
                                        }
                                    }
                                }
                                Protocol::ModbusTcp | Protocol::ModbusRtu => {
                                    self.start_modbus_poll(shared);
                                }
                            }
                        }
                    }
                    ui.add_space(6.0);
                    crate::app::status(ui, shared.connected, "已连接", "未连接");
                    if matches!(self.protocol, Protocol::TcpClient | Protocol::WebSocket) {
                        ui.checkbox(&mut self.auto_reconnect, "断线自动重连（1s→2s→5s）");
                    }
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
                    match Self::save_log(shared) {
                        Ok(path) => shared.toast(format!("日志已保存：{path}")),
                        Err(e) => shared.toast(format!("保存失败：{e}")),
                    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_from_days_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19723), (2024, 1, 1));
        assert_eq!(civil_from_days(20000), (2024, 10, 4));
    }

    #[test]
    fn hex_payload_parses() {
        let mut page = DebugPage::default();
        page.hex_mode = true;
        page.raw_input = "01 03 00 00 00 0A".into();
        page.eol = "无".into();
        assert_eq!(
            page.parse_raw_payload().unwrap(),
            vec![0x01, 0x03, 0, 0, 0, 0x0A]
        );
        page.eol = "\\n".into();
        let b = page.parse_raw_payload().unwrap();
        assert_eq!(b.last(), Some(&b'\n'));
        page.raw_input = "01 0G".into();
        assert!(page.parse_raw_payload().is_err());
        page.raw_input = "01 0".into();
        assert!(page.parse_raw_payload().is_err());
    }

    #[test]
    fn text_payload_appends_eol() {
        let mut page = DebugPage::default();
        page.hex_mode = false;
        page.raw_input = "AT".into();
        page.eol = "\\r\\n".into();
        assert_eq!(page.parse_raw_payload().unwrap(), b"AT\r\n".to_vec());
    }
}
