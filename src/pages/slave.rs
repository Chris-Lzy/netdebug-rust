//! 从站页：Modbus 从站模拟（egui 重写）。
//!
//! 程序当从站，响应主站的读写；四区数据表可在线改值，退出时存 config.json。
//! 后端在 slave_backend.rs（TCP 监听 + RTU 串口）；仿真（自增/随机）
//! 未定稿：只放一个禁用的“待定”区，不实现。

use crate::app::Shared;
use crate::pages::Page;
use crate::slave_backend::{self, SlaveData, SlaveHandle, SlaveMsg};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

pub struct SlavePage {
    slave_id: u8,
    transport: String, // "TCP" / "RTU"
    tcp_port: u16,
    serial_port: String,
    baud: u32,
    data_bits: u8,
    parity: String,
    stop_bits: String,
    running: bool,
    /// 四区数据表：界面和后台线程共享同一份，改了立即生效。
    data: Arc<Mutex<SlaveData>>,
    /// 运行中的从站服务（启动成功后才有）。
    handle: Option<SlaveHandle>,
    /// TCP 实际监听端口（界面状态栏显示用；RTU 时为 0）。
    listen_port: u16,
    /// 配置里的从站参数/数据表是否已读进界面（只读一次）。
    loaded: bool,
    requests: Vec<String>,
}

impl Default for SlavePage {
    fn default() -> Self {
        Self {
            slave_id: 1,
            transport: "TCP".to_string(),
            tcp_port: 502,
            serial_port: "COM1".to_string(),
            baud: 9600,
            data_bits: 8,
            parity: "None".to_string(),
            stop_bits: "1".to_string(),
            running: false,
            data: Arc::new(Mutex::new(SlaveData::new(16))),
            handle: None,
            listen_port: 0,
            loaded: false,
            requests: Vec::new(),
        }
    }
}

impl SlavePage {
    /// 一区数据表：线圈/离散输入（bool 格）。
    /// 注意是关联函数而不是方法：调用时 `Self::bool_table(ui, …, &mut self.coils)`，
    /// 否则 `&mut self` 和 `&mut self.coils` 会借用冲突。
    fn bool_table(ui: &mut egui::Ui, title: &str, data: &mut [bool]) {
        ui.label(title);
        egui::Grid::new(title).striped(true).show(ui, |ui| {
            for (i, v) in data.iter_mut().enumerate() {
                ui.label(format!("{i}"));
                ui.checkbox(v, "");
                if i % 4 == 3 {
                    ui.end_row();
                }
            }
        });
    }

    /// 一区数据表：保持/输入寄存器（u16 格）。
    fn u16_table(ui: &mut egui::Ui, title: &str, data: &mut [u16]) {
        ui.label(title);
        egui::Grid::new(title).striped(true).show(ui, |ui| {
            for (i, v) in data.iter_mut().enumerate() {
                ui.label(format!("{i}"));
                ui.add(egui::DragValue::new(v).range(0..=65535));
                if i % 4 == 3 {
                    ui.end_row();
                }
            }
        });
    }
}

impl Page for SlavePage {
    fn id(&self) -> &'static str {
        "slave"
    }
    fn title(&self) -> &'static str {
        "从站"
    }
    fn icon(&self) -> &'static str {
        "▣"
    }

    fn show(&mut self, ui: &mut egui::Ui, shared: &mut Shared) {
        // 配置里的从站参数/数据表只读一次进界面。
        if !self.loaded {
            self.loaded = true;
            let cfg = &shared.config.slave;
            self.slave_id = cfg.slave_id;
            self.tcp_port = cfg.tcp_port;
            self.baud = cfg.baud;
            if cfg.holding.len() == 16 {
                let mut data = self.data.lock().unwrap_or_else(|e| e.into_inner());
                data.coils.clone_from(&cfg.coils);
                data.discrete.clone_from(&cfg.discrete);
                data.holding.clone_from(&cfg.holding);
                data.input.clone_from(&cfg.input);
            }
        }
        // 收后台日志；收到 Stopped 说明服务已停。
        let mut stopped = false;
        if let Some(h) = &self.handle {
            while let Ok(msg) = h.rx.try_recv() {
                match msg {
                    SlaveMsg::Log(line) => {
                        self.requests.push(line);
                        if self.requests.len() > 200 {
                            self.requests.remove(0);
                        }
                    }
                    SlaveMsg::Stopped => stopped = true,
                }
            }
        }
        if stopped {
            self.handle = None;
            self.running = false;
        }

        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add_space(4.0);
            // ---- 接入设置 ----
            crate::app::card_frame().show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label("从站地址");
                    ui.add(egui::DragValue::new(&mut self.slave_id).range(1..=247));
                    ui.label("接入");
                    egui::ComboBox::from_id_salt("slave_transport")
                        .selected_text(self.transport.clone())
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.transport, "TCP".to_string(), "Modbus TCP");
                            ui.selectable_value(&mut self.transport, "RTU".to_string(), "Modbus RTU");
                        });
                    if self.transport == "TCP" {
                        ui.label("监听端口");
                        ui.add(egui::DragValue::new(&mut self.tcp_port).range(1..=65535));
                    } else {
                        ui.label("串口");
                        ui.add(egui::TextEdit::singleline(&mut self.serial_port).desired_width(80.0));
                        ui.label("波特率");
                        egui::ComboBox::from_id_salt("slave_baud")
                            .selected_text(self.baud.to_string())
                            .show_ui(ui, |ui| {
                                for b in [1200, 2400, 4800, 9600, 19200, 38400, 57600, 115200] {
                                    ui.selectable_value(&mut self.baud, b, b.to_string());
                                }
                            });
                        ui.label("数据位");
                        egui::ComboBox::from_id_salt("slave_bits")
                            .selected_text(self.data_bits.to_string())
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut self.data_bits, 8, "8");
                                ui.selectable_value(&mut self.data_bits, 7, "7");
                            });
                        ui.label("校验");
                        egui::ComboBox::from_id_salt("slave_parity")
                            .selected_text(self.parity.clone())
                            .show_ui(ui, |ui| {
                                for p in ["None", "Even", "Odd"] {
                                    ui.selectable_value(&mut self.parity, p.to_string(), p);
                                }
                            });
                        ui.label("停止位");
                        egui::ComboBox::from_id_salt("slave_stop")
                            .selected_text(self.stop_bits.clone())
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut self.stop_bits, "1".to_string(), "1");
                                ui.selectable_value(&mut self.stop_bits, "2".to_string(), "2");
                            });
                    }
                    if ui
                        .add(crate::app::primary(if self.running {
                            "停止"
                        } else {
                            "启动"
                        }))
                        .clicked()
                    {
                        if self.running {
                            if let Some(h) = &self.handle {
                                h.stop.store(true, Ordering::Relaxed);
                            }
                        } else {
                            let data = self.data.clone();
                            let res = if self.transport == "TCP" {
                                slave_backend::start_tcp_slave(self.tcp_port, self.slave_id, data)
                            } else {
                                let settings = crate::rtu::SerialSettings {
                                    port: self.serial_port.trim().to_string(),
                                    baud: self.baud,
                                    data_bits: self.data_bits,
                                    parity: self.parity.clone(),
                                    stop_bits: self.stop_bits.clone(),
                                };
                                slave_backend::start_rtu_slave(settings, self.slave_id, data)
                            };
                            match res {
                                Ok(handle) => {
                                    self.listen_port = handle.port;
                                    self.handle = Some(handle);
                                    self.running = true;
                                }
                                Err(e) => {
                                    shared.toast(e.clone());
                                    self.requests.push(format!("✗ {e}"));
                                }
                            }
                        }
                    }
                    let run_label = if self.transport == "TCP" && self.listen_port > 0 {
                        format!("监听中 :{}", self.listen_port)
                    } else {
                        "监听中".to_string()
                    };
                    crate::app::status(ui, self.running, &run_label, "已停止");
                });
            });
            ui.add_space(10.0);

            // ---- 四区数据表（和后台线程共享同一份，改了立即生效）----
            let mut data = self.data.lock().unwrap_or_else(|e| e.into_inner());
            ui.columns(2, |cols| {
                crate::app::card_frame().show(&mut cols[0], |ui| {
                    Self::bool_table(ui, "线圈（01/05/15）", &mut data.coils);
                });
                crate::app::card_frame().show(&mut cols[1], |ui| {
                    Self::bool_table(ui, "离散输入（02）", &mut data.discrete);
                });
            });
            ui.add_space(8.0);
            ui.columns(2, |cols| {
                crate::app::card_frame().show(&mut cols[0], |ui| {
                    Self::u16_table(ui, "保持寄存器（03/06/16）", &mut data.holding);
                });
                crate::app::card_frame().show(&mut cols[1], |ui| {
                    Self::u16_table(ui, "输入寄存器（04）", &mut data.input);
                });
            });
            // 数据表同步回配置，退出时存 config.json。
            shared.config.slave.coils = data.coils.clone();
            shared.config.slave.discrete = data.discrete.clone();
            shared.config.slave.holding = data.holding.clone();
            shared.config.slave.input = data.input.clone();
            shared.config.slave.slave_id = self.slave_id;
            shared.config.slave.tcp_port = self.tcp_port;
            shared.config.slave.baud = self.baud;
            drop(data);
            ui.add_space(8.0);

            // ---- 仿真（未定稿，占位）----
            ui.add_enabled(false, egui::Button::new("仿真（自增/随机）——待定"));
            ui.add_space(8.0);

            // ---- 请求日志 ----
            crate::app::section(ui, "请求日志");
            ui.add_space(4.0);
            crate::app::card_frame().show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(160.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        if self.requests.is_empty() {
                            ui.colored_label(
                                crate::app::MUTED,
                                "暂无请求（主站每次读写会记一条：功能码、地址、数量、是否成功）",
                            );
                        }
                        for r in &self.requests {
                            ui.monospace(r.as_str());
                        }
                    });
            });
            ui.add_space(4.0);
        });
    }
}
