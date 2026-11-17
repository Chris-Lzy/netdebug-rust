//! 网卡页：NetworkIPTool 原版布局（egui 重写）。
//!
//! 每块网卡 5 条 IP 档案，一键切换；改表自动保存到 config.json。
//! 注意：
//! - 适配器枚举和当前 IP 是真实读取（PowerShell Get-NetAdapter，只读不提权）；
//! - “应用此 IP”最终要调 netsh（需管理员权限，清单 requireAdministrator
//!   在打包时加），现在是 TODO stub，点确认后只弹“未接后端”提示；
//! - 档案的增删改和保存到 config.json 是真的（不碰系统网络，不用提权）；
//! - 表里四条真实档案是 Chris 现有的（信捷/雷赛、欧姆龙、Festo、施耐德）。

use crate::app::Shared;
use crate::config::{self, NicProfile};
use crate::nic_backend::{self, AdapterInfo};
use crate::pages::Page;
use std::sync::mpsc::Receiver;

pub struct NicPage {
    adapters: Vec<String>,
    selected_adapter: String,
    profiles: Vec<NicProfile>,
    selected_row: Option<usize>,
    status: String,
    status_ok: bool,
    /// 正在确认应用的档案行（None = 没有确认框）
    confirm_apply: Option<usize>,
    /// 重命名弹窗是否打开
    rename_open: bool,
    rename_new_name: String,
    /// 当前表所属的适配器（切适配器时先落盘旧表再读新表，避免串表覆盖）
    loaded_adapter: String,
    /// 真实网卡枚举结果（PowerShell 后台线程）
    infos: Vec<AdapterInfo>,
    nic_rx: Option<Receiver<Vec<AdapterInfo>>>,
    enumerated: bool,
}

impl Default for NicPage {
    fn default() -> Self {
        Self {
            adapters: vec![
                "本体网口".to_string(),
                "以太网 3".to_string(),
                "以太网 6".to_string(),
                "Type-C HUB".to_string(),
                "USB网卡".to_string(),
                "Wifi".to_string(),
            ],
            selected_adapter: "本体网口".to_string(),
            profiles: Vec::new(),
            selected_row: None,
            status: "就绪".to_string(),
            status_ok: true,
            confirm_apply: None,
            rename_open: false,
            rename_new_name: String::new(),
            loaded_adapter: String::new(),
            infos: Vec::new(),
            nic_rx: None,
            enumerated: false,
        }
    }
}

impl NicPage {
    /// 默认档案：四条示例档案（开源版，示例地址；原版来自 ip_config.xml，
    /// 真实车间地址已替换掉，第一次用时按自己的 PLC 网段改）。
    fn default_profiles() -> Vec<NicProfile> {
        let p = |name: &str, ip: &str| NicProfile {
            name: name.to_string(),
            ip: ip.to_string(),
            ..Default::default()
        };
        vec![
            p("信捷/雷赛", "192.168.1.11"),
            p("欧姆龙", "192.168.1.200"),
            p("Festo", "192.168.1.2"),
            p("施耐德", "192.168.1.201"),
            NicProfile {
                name: "配置 5".to_string(),
                ..Default::default()
            },
        ]
    }

    /// 切换适配器时：先把当前表静默落盘到旧适配器，再读新适配器的表，
    /// 避免 A 的档案被写进 B 造成覆盖。首次打开读 config，没有就用默认四条。
    fn sync_adapter(&mut self, shared: &mut Shared) {
        if self.loaded_adapter == self.selected_adapter {
            return;
        }
        if !self.loaded_adapter.is_empty() {
            let old = self.loaded_adapter.clone();
            self.write_profiles(shared, &old);
            let _ = config::save(&shared.config);
        }
        self.loaded_adapter = self.selected_adapter.clone();
        // 枚举完成前选中项还是占位名，写进 last_adapter 会覆盖真实偏好
        if self.enumerated {
            shared.config.nic.last_adapter = self.selected_adapter.clone();
        }
        self.profiles = shared
            .config
            .nic
            .profiles
            .iter()
            .find(|a| a.adapter == self.selected_adapter)
            .map(|a| a.profiles.clone())
            .unwrap_or_else(Self::default_profiles);
        self.selected_row = None;
    }

    /// 把当前表写进 config 内存中指定适配器的条目（不落盘）。
    fn write_profiles(&mut self, shared: &mut Shared, adapter: &str) {
        let entry = shared
            .config
            .nic
            .profiles
            .iter_mut()
            .find(|a| a.adapter == adapter);
        match entry {
            Some(e) => e.profiles = self.profiles.clone(),
            None => shared.config.nic.profiles.push(config::NicAdapterProfiles {
                adapter: adapter.to_string(),
                profiles: self.profiles.clone(),
            }),
        }
    }

    /// 把当前表写回 config.json（改表自动保存也走这里）。
    fn save_to_config(&mut self, shared: &mut Shared) {
        let adapter = self.selected_adapter.clone();
        self.write_profiles(shared, &adapter);
        match config::save(&shared.config) {
            Ok(()) => self.set_status(true, "✓ 配置已保存到 config.json"),
            Err(e) => self.set_status(false, format!("✗ 保存配置失败：{e}")),
        }
    }

    fn set_status(&mut self, ok: bool, text: impl Into<String>) {
        self.status_ok = ok;
        self.status = text.into();
    }

    fn current_info(&self) -> Option<&AdapterInfo> {
        self.infos.iter().find(|i| i.name == self.selected_adapter)
    }

    /// 收后台枚举结果；首次拿到真实列表时把选中项落到真实网卡上。
    fn apply_infos(&mut self, shared: &mut Shared, infos: Vec<AdapterInfo>) {
        let was_real = self.enumerated;
        self.enumerated = true;
        self.infos = infos;
        self.adapters = self.infos.iter().map(|i| i.name.clone()).collect();
        if self.adapters.is_empty() {
            self.set_status(false, "✗ 没枚举到网卡（PowerShell 不可用？）");
            return;
        }
        if !self.adapters.contains(&self.selected_adapter) {
            let preferred = shared.config.nic.last_adapter.clone();
            self.selected_adapter = if self.adapters.contains(&preferred) {
                preferred
            } else {
                self.infos
                    .iter()
                    .find(|i| i.up)
                    .or_else(|| self.infos.first())
                    .map(|i| i.name.clone())
                    .unwrap_or_default()
            };
        }
        if self.loaded_adapter != self.selected_adapter {
            // 旧表属于真实网卡时先落盘；首次枚举的占位名没落过盘，直接换表
            if was_real && !self.loaded_adapter.is_empty() {
                let old = self.loaded_adapter.clone();
                self.write_profiles(shared, &old);
                let _ = config::save(&shared.config);
            }
            self.loaded_adapter = self.selected_adapter.clone();
            shared.config.nic.last_adapter = self.selected_adapter.clone();
            self.profiles = shared
                .config
                .nic
                .profiles
                .iter()
                .find(|a| a.adapter == self.selected_adapter)
                .map(|a| a.profiles.clone())
                .unwrap_or_else(Self::default_profiles);
            self.selected_row = None;
        }
        self.set_status(true, "✓ 网卡已枚举");
    }

    fn poll_nic_backend(&mut self, shared: &mut Shared) {
        if !self.enumerated && self.nic_rx.is_none() {
            self.nic_rx = Some(nic_backend::spawn_fetch());
        }
        let Some(rx) = self.nic_rx.take() else {
            return;
        };
        match rx.try_recv() {
            Ok(infos) => self.apply_infos(shared, infos),
            Err(std::sync::mpsc::TryRecvError::Empty) => self.nic_rx = Some(rx),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.enumerated = true;
                self.set_status(false, "✗ 网卡枚举线程异常退出");
            }
        }
    }

    fn show_profile_grid(&mut self, ui: &mut egui::Ui) {
        let headers = [
            "配置名称",
            "IP 地址",
            "子网掩码",
            "默认网关",
            "首选 DNS",
            "备用 DNS",
            "操作",
        ];
        egui::Grid::new("nic_grid").striped(true).show(ui, |ui| {
            for h in headers {
                ui.strong(h);
            }
            ui.end_row();
            for row in 0..self.profiles.len() {
                // 点配置名称框即选中该行
                let mut p = self.profiles[row].clone();
                let resp_name =
                    ui.add_sized([90.0, 26.0], egui::TextEdit::singleline(&mut p.name));
                ui.add_sized([125.0, 26.0], egui::TextEdit::singleline(&mut p.ip));
                ui.add_sized([110.0, 26.0], egui::TextEdit::singleline(&mut p.mask));
                ui.add_sized([125.0, 26.0], egui::TextEdit::singleline(&mut p.gateway));
                ui.add_sized([90.0, 26.0], egui::TextEdit::singleline(&mut p.dns1));
                ui.add_sized([90.0, 26.0], egui::TextEdit::singleline(&mut p.dns2));
                if ui.button("应用此 IP").clicked() {
                    if p.ip.trim().is_empty() {
                        self.set_status(false, "✗ IP 地址不能为空！");
                    } else {
                        self.profiles[row] = p.clone();
                        self.confirm_apply = Some(row);
                    }
                }
                if resp_name.clicked() {
                    self.selected_row = Some(row);
                }
                // 改了就写回内存（自动保存到 config.json 在失去焦点时做，
                // 这里先记下来，保存按钮/切页时统一落盘）
                if p.name != self.profiles[row].name
                    || p.ip != self.profiles[row].ip
                    || p.mask != self.profiles[row].mask
                    || p.gateway != self.profiles[row].gateway
                    || p.dns1 != self.profiles[row].dns1
                    || p.dns2 != self.profiles[row].dns2
                {
                    self.profiles[row] = p;
                }
                ui.end_row();
            }
        });
    }

    /// “应用此 IP”的确认弹窗。
    fn show_confirm_dialog(&mut self, ui: &mut egui::Ui, shared: &mut Shared) {
        let Some(row) = self.confirm_apply else {
            return;
        };
        let p = self.profiles[row].clone();
        egui::Window::new("确认")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ui.ctx(), |ui| {
                ui.label(format!(
                    "确认将 [{}] 设置为：\n\nIP: {}\n掩码: {}\n网关: {}\nDNS: {} / {}",
                    p.name, p.ip, p.mask, p.gateway, p.dns1, p.dns2
                ));
                ui.horizontal(|ui| {
                    if ui.button("确定").clicked() {
                        // TODO: 调 netsh 应用静态 IP（需管理员权限，打包时加
                        // requireAdministrator 清单）；网关为空传 none，DNS1
                        // 用 set dns primary，DNS2 用 add dns index=2。
                        self.set_status(true, format!("✓ 已应用（演示）：{}", p.ip));
                        shared.toast("未接后端：netsh 调用待实现");
                        self.confirm_apply = None;
                    }
                    if ui.button("取消").clicked() {
                        self.confirm_apply = None;
                    }
                });
            });
    }
}

impl Page for NicPage {
    fn id(&self) -> &'static str {
        "nic"
    }
    fn title(&self) -> &'static str {
        "网卡"
    }
    fn icon(&self) -> &'static str {
        "▤"
    }

    fn show(&mut self, ui: &mut egui::Ui, shared: &mut Shared) {
        self.sync_adapter(shared);
        self.poll_nic_backend(shared);

        // ---- 底栏（先注册，避免被滚动区挤掉）----
        egui::Panel::bottom("nic_bottom").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                if ui.button("保存配置").clicked() {
                    self.save_to_config(shared);
                }
                if ui.button("+ 添加配置").clicked() {
                    let n = self.profiles.len() + 1;
                    self.profiles.push(NicProfile {
                        name: format!("配置 {n}"),
                        ..Default::default()
                    });
                    self.save_to_config(shared);
                }
                if ui.button("- 删除选中").clicked() {
                    if let Some(row) = self.selected_row {
                        if row < self.profiles.len() {
                            self.profiles.remove(row);
                            self.selected_row = None;
                            self.save_to_config(shared);
                        }
                    }
                }
                let color = if self.status_ok {
                    crate::app::OK
                } else {
                    crate::app::ERR
                };
                ui.colored_label(color, &self.status);
            });
            ui.add_space(4.0);
        });

        // ---- 工具行 ----
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.label("适配器");
            let selected_text = match self.current_info() {
                Some(i) => format!(
                    "{} [{}]",
                    i.name,
                    if i.up { "已连接" } else { "未连接" }
                ),
                None => format!("{} [枚举中…]", self.selected_adapter),
            };
            egui::ComboBox::from_id_salt("nic_adapter")
                .selected_text(selected_text)
                .show_ui(ui, |ui| {
                    for a in self.adapters.clone() {
                        ui.selectable_value(&mut self.selected_adapter, a.clone(), a);
                    }
                });
            if ui.button("刷新适配器").clicked() {
                self.nic_rx = Some(nic_backend::spawn_fetch());
            }
            if ui.button("恢复 DHCP").clicked() {
                // TODO: netsh set address/dns dhcp（需管理员权限）。
                shared.toast("未接后端：恢复 DHCP 待实现");
            }
            if ui.button("重命名适配器").clicked() {
                self.rename_new_name = self.selected_adapter.clone();
                self.rename_open = true;
            }
            let current = match self.current_info() {
                Some(i) if !i.ip.is_empty() => {
                    let dns_list: Vec<&str> = [i.dns1.as_str(), i.dns2.as_str()]
                        .into_iter()
                        .filter(|s| !s.is_empty())
                        .collect();
                    let dns = if dns_list.is_empty() {
                        String::new()
                    } else {
                        format!("  DNS: {}", dns_list.join(" / "))
                    };
                    format!(
                        "当前 IP: {}  掩码: {}  网关: {}{}",
                        i.ip, i.mask, i.gateway, dns
                    )
                }
                Some(_) => "当前 IP: 未分配（DHCP 未获取或网线未插）".to_string(),
                None => "当前 IP: -".to_string(),
            };
            ui.colored_label(crate::app::MUTED, current);
        });
        ui.add_space(6.0);

        // ---- 档案表 ----
        crate::app::card_frame().show(ui, |ui| {
            egui::ScrollArea::both().show(ui, |ui| {
                self.show_profile_grid(ui);
            });
        });

        // ---- 弹窗 ----
        self.show_confirm_dialog(ui, shared);
        if self.rename_open {
            egui::Window::new("重命名适配器")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ui.ctx(), |ui| {
                    ui.horizontal(|ui| {
                        ui.label("新名称:");
                        ui.text_edit_singleline(&mut self.rename_new_name);
                    });
                    ui.horizontal(|ui| {
                        if ui.button("确定").clicked() {
                            // TODO: netsh interface set interface newname=...
                            shared.toast("未接后端：重命名待实现");
                            self.rename_open = false;
                        }
                        if ui.button("取消").clicked() {
                            self.rename_open = false;
                        }
                    });
                });
        }
    }
}
