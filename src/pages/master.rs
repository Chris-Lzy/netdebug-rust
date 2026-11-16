//! 主站页：Modbus TCP 主站（轮询/单次读写，真实后端）。
//!
//! RTU 串口暂未接后端（点启动弹提示）；TCP 走 modbus.rs 的后台轮询线程。
//! 结果表和请求日志与从站页风格一致。

use crate::app::Shared;
use crate::modbus::{self, MasterJob, MasterMsg};
use crate::pages::Page;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;

const FCS: [(u8, &str); 8] = [
    (1, "01 读线圈"),
    (2, "02 读离散输入"),
    (3, "03 读保持寄存器"),
    (4, "04 读输入寄存器"),
    (5, "05 写单线圈"),
    (6, "06 写单寄存器"),
    (15, "15 写多线圈"),
    (16, "16 写多寄存器"),
];

pub struct MasterPage {
    rtu: bool,
    host: String,
    port: String,
    serial: String,
    baud: String,
    unit: u8,
    fc: u8,
    addr: u16,
    qty: u16,
    write_text: String,
    interval_ms: u64,
    continuous: bool,
    running: bool,
    stop: Option<Arc<AtomicBool>>,
    rx: Option<Receiver<MasterMsg>>,
    base: u16,
    result_fc: u8,
    bits: Vec<bool>,
    regs: Vec<u16>,
    log: Vec<String>,
    status: String,
    status_ok: bool,
}

impl Default for MasterPage {
    fn default() -> Self {
        Self {
            rtu: false,
            host: "127.0.0.1".to_string(),
            port: "502".to_string(),
            serial: "COM1".to_string(),
            baud: "9600".to_string(),
            unit: 1,
            fc: 3,
            addr: 0,
            qty: 10,
            write_text: String::new(),
            interval_ms: 500,
            continuous: true,
            running: false,
            stop: None,
            rx: None,
            base: 0,
            result_fc: 0,
            bits: Vec::new(),
            regs: Vec::new(),
            log: Vec::new(),
            status: "就绪".to_string(),
            status_ok: true,
        }
    }
}

impl MasterPage {
    fn set_status(&mut self, ok: bool, text: impl Into<String>) {
        self.status_ok = ok;
        self.status = text.into();
    }

    fn push_log(&mut self, line: String) {
        self.status_ok = !line.starts_with('✗');
        self.status = line.clone();
        self.log.push(line);
        if self.log.len() > 200 {
            self.log.remove(0);
        }
    }

    fn poll_backend(&mut self) {
        let Some(rx) = self.rx.take() else {
            return;
        };
        let mut done = false;
        while let Ok(msg) = rx.try_recv() {
            match msg {
                MasterMsg::Read { addr, bits, regs } => {
                    self.base = addr;
                    self.result_fc = self.fc;
                    self.bits = bits;
                    self.regs = regs;
                }
                MasterMsg::Log(l) => self.push_log(l),
                MasterMsg::Stopped => done = true,
            }
        }
        if done {
            self.running = false;
            self.stop = None;
        } else {
            self.rx = Some(rx);
        }
    }

    fn parse_values(&self) -> Result<Vec<u16>, String> {
        let vals: Vec<u16> = self
            .write_text
            .split([',', ' ', ';', '，'])
            .filter(|s| !s.trim().is_empty())
            .map(|s| {
                s.trim()
                    .parse::<u16>()
                    .map_err(|_| format!("写入值“{s}”不是 0-65535"))
            })
            .collect::<Result<_, _>>()?;
        if vals.is_empty() {
            return Err("写入值不能为空".into());
        }
        Ok(vals)
    }

    fn start(&mut self, shared: &mut Shared, once: bool) {
        if self.running {
            shared.toast("请先停止当前轮询");
            return;
        }
        if self.rtu {
            shared.toast("未接后端：Modbus RTU 串口待实现");
            return;
        }
        let port: u16 = match self.port.trim().parse() {
            Ok(p) if p > 0 => p,
            _ => {
                self.set_status(false, "✗ 端口非法");
                return;
            }
        };
        if self.host.trim().is_empty() {
            self.set_status(false, "✗ 远程地址不能为空");
            return;
        }
        let mut qty = self.qty.clamp(1, 125);
        let mut values = Vec::new();
        if modbus::is_write(self.fc) {
            values = match self.parse_values() {
                Ok(v) => v,
                Err(e) => {
                    self.set_status(false, format!("✗ {e}"));
                    return;
                }
            };
            qty = if matches!(self.fc, 5 | 6) {
                1
            } else {
                (values.len() as u16).clamp(1, 123)
            };
            values.truncate(qty as usize);
        }
        let job = MasterJob {
            host: self.host.trim().to_string(),
            port,
            unit: self.unit,
            fc: self.fc,
            addr: self.addr,
            qty,
            values,
            interval_ms: self.interval_ms.max(20),
            once: once || !self.continuous,
        };
        let stop = Arc::new(AtomicBool::new(false));
        self.stop = Some(stop.clone());
        self.rx = Some(modbus::spawn_poller(job, stop));
        self.running = true;
        self.set_status(true, if once { "✓ 单次请求已发出" } else { "✓ 轮询已启动" });
    }

    fn stop_poll(&mut self) {
        if let Some(s) = &self.stop {
            s.store(true, Ordering::Relaxed);
        }
        self.set_status(true, "…正在停止");
    }

    fn show_settings(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.label("接入");
            egui::ComboBox::from_id_salt("master_transport")
                .selected_text(if self.rtu { "RTU" } else { "TCP" })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.rtu, false, "TCP");
                    ui.selectable_value(&mut self.rtu, true, "RTU");
                });
            if self.rtu {
                ui.label("串口");
                ui.add_sized([80.0, 26.0], egui::TextEdit::singleline(&mut self.serial));
                ui.label("波特率");
                egui::ComboBox::from_id_salt("master_baud")
                    .selected_text(&self.baud)
                    .show_ui(ui, |ui| {
                        for b in ["9600", "19200", "38400", "115200"] {
                            ui.selectable_value(&mut self.baud, b.to_string(), b);
                        }
                    });
            } else {
                ui.label("远程地址");
                ui.add_sized([130.0, 26.0], egui::TextEdit::singleline(&mut self.host));
                ui.label("端口");
                ui.add_sized([60.0, 26.0], egui::TextEdit::singleline(&mut self.port));
            }
        });
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.label("从站地址");
            ui.add(egui::DragValue::new(&mut self.unit).range(1..=247));
            ui.label("功能码");
            let label = FCS
                .iter()
                .find(|(f, _)| *f == self.fc)
                .map(|(_, l)| *l)
                .unwrap_or("03");
            egui::ComboBox::from_id_salt("master_fc")
                .selected_text(label)
                .show_ui(ui, |ui| {
                    for (f, l) in FCS {
                        ui.selectable_value(&mut self.fc, f, l);
                    }
                });
            ui.label("起始地址");
            ui.add(egui::DragValue::new(&mut self.addr).range(0..=65535));
            if !matches!(self.fc, 5 | 6) {
                ui.label("数量");
                ui.add(egui::DragValue::new(&mut self.qty).range(1..=125));
            }
            if modbus::is_write(self.fc) {
                ui.label("写入值");
                ui.add_sized(
                    [160.0, 26.0],
                    egui::TextEdit::singleline(&mut self.write_text)
                        .hint_text("逗号分隔，如 100,200"),
                );
            }
        });
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.label("轮询间隔");
            ui.add(egui::DragValue::new(&mut self.interval_ms).range(20..=60000).suffix(" ms"));
            ui.checkbox(&mut self.continuous, "连续轮询");
        });
    }

    fn show_result(&mut self, ui: &mut egui::Ui) {
        if self.bits.is_empty() && self.regs.is_empty() {
            ui.colored_label(crate::app::MUTED, "暂无数据（启动轮询或单次读取后显示）");
            return;
        }
        egui::ScrollArea::both().max_height(220.0).show(ui, |ui| {
            egui::Grid::new("master_result").striped(true).show(ui, |ui| {
                if modbus::is_bits(self.result_fc) {
                    ui.strong("地址");
                    ui.strong("状态");
                    ui.end_row();
                    for (i, b) in self.bits.iter().enumerate() {
                        ui.monospace(format!("{}", self.base + i as u16));
                        ui.colored_label(
                            if *b { crate::app::OK } else { crate::app::MUTED },
                            if *b { "● 1" } else { "○ 0" },
                        );
                        ui.end_row();
                    }
                } else {
                    ui.strong("地址");
                    ui.strong("十进制");
                    ui.strong("十六进制");
                    ui.end_row();
                    for (i, v) in self.regs.iter().enumerate() {
                        ui.monospace(format!("{}", self.base + i as u16));
                        ui.monospace(format!("{v}"));
                        ui.monospace(format!("0x{v:04X}"));
                        ui.end_row();
                    }
                }
            });
        });
    }
}

impl Page for MasterPage {
    fn id(&self) -> &'static str {
        "master"
    }
    fn title(&self) -> &'static str {
        "主站"
    }
    fn icon(&self) -> &'static str {
        "↻"
    }

    fn show(&mut self, ui: &mut egui::Ui, shared: &mut Shared) {
        self.poll_backend();

        egui::Panel::bottom("master_bottom").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                let label = if self.running { "停止" } else { "启动" };
                if ui.add_sized([90.0, 32.0], crate::app::primary(label)).clicked() {
                    if self.running {
                        self.stop_poll();
                    } else {
                        self.start(shared, false);
                    }
                }
                let once_label = if modbus::is_write(self.fc) { "写入一次" } else { "读取一次" };
                if ui.button(once_label).clicked() {
                    self.start(shared, true);
                }
                let color = if self.status_ok { crate::app::OK } else { crate::app::ERR };
                ui.colored_label(color, &self.status);
            });
            ui.add_space(4.0);
        });

        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add_space(4.0);
            crate::app::card_frame().show(ui, |ui| {
                crate::app::section(ui, "连接与请求");
                self.show_settings(ui);
            });
            ui.add_space(6.0);
            crate::app::card_frame().show(ui, |ui| {
                crate::app::section(ui, "读取结果");
                self.show_result(ui);
            });
            ui.add_space(6.0);
            crate::app::card_frame().show(ui, |ui| {
                crate::app::section(ui, "请求日志");
                egui::ScrollArea::vertical()
                    .max_height(140.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        if self.log.is_empty() {
                            ui.colored_label(crate::app::MUTED, "暂无请求");
                        }
                        for line in &self.log {
                            let color = if line.starts_with('✗') {
                                crate::app::ERR
                            } else {
                                crate::app::MUTED
                            };
                            ui.colored_label(color, egui::RichText::new(line).monospace().size(12.0));
                        }
                    });
            });
        });
    }
}