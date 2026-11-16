//! 网络调试助手 —— 配置（config.json，放在 exe 同目录）。
//!
//! 规则：
//! - 配置文件只由 Rust 读写，不经过任何前端/脚本；
//! - 文件不存在、损坏、版本不匹配时，全部回落到默认值，绝不闪退；
//!   损坏的旧文件会被改名备份成 `config.json.bak`（只留一份），再写一份默认配置；
//! - 以后加字段只在结构体里加带 `#[serde(default)]` 的字段，老配置文件照样能读。

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

// ---------------------------------------------------------------- 配置结构

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    /// 配置格式版本，以后改结构就升它
    pub version: u32,
    /// 上次打开的页签：debug / slave / nic
    pub last_page: String,
    /// 调试页：上次的连接参数（重启后自动还原）
    pub debug: DebugConfig,
    /// 从站页：上次的从站参数
    pub slave: SlaveConfig,
    /// 网卡页：档案存在这里（以后从老 ip_config.xml 导入一次）
    pub nic: NicConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DebugConfig {
    /// 上次用的协议：tcp-client / tcp-server / udp / websocket / modbus-tcp / modbus-rtu
    pub protocol: String,
    /// 远程地址（TCP Client / UDP / Modbus TCP）
    pub remote_host: String,
    /// 远程端口（Modbus TCP 默认 502）
    pub remote_port: u16,
    /// 监听端口（TCP Server 默认 9000）
    pub listen_port: u16,
    /// WebSocket 直连地址
    pub ws_url: String,
    /// Modbus 从站地址
    pub unit_id: u8,
    /// 远程地址历史（最近在前，最多 10 条）
    pub host_history: Vec<String>,
    /// WS 地址历史（最近在前，最多 10 条）
    pub ws_history: Vec<String>,
    /// 显示偏好
    pub hex_mode: bool,
    pub show_timestamp: bool,
    pub auto_scroll: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SlaveConfig {
    /// 从站地址
    pub slave_id: u8,
    /// Modbus TCP 从站监听端口
    pub tcp_port: u16,
    /// 串口波特率（RTU 从站）
    pub baud: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NicConfig {
    /// 上次选中的网卡名
    pub last_adapter: String,
    /// IP 档案：网卡名 -> 档案列表（以后从 ip_config.xml 导入）
    pub profiles: Vec<NicAdapterProfiles>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NicAdapterProfiles {
    pub adapter: String,
    pub profiles: Vec<NicProfile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NicProfile {
    /// 配置名称，如“信捷/雷赛”
    pub name: String,
    pub ip: String,
    pub mask: String,
    pub gateway: String,
    pub dns1: String,
    pub dns2: String,
}

// ---------------------------------------------------------------- 默认值

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            version: 1,
            last_page: "debug".to_string(),
            debug: DebugConfig::default(),
            slave: SlaveConfig::default(),
            nic: NicConfig::default(),
        }
    }
}

impl Default for DebugConfig {
    fn default() -> Self {
        Self {
            protocol: "tcp-client".to_string(),
            remote_host: "127.0.0.1".to_string(),
            remote_port: 502,
            listen_port: 9000,
            ws_url: "ws://127.0.0.1:8080".to_string(),
            unit_id: 1,
            host_history: Vec::new(),
            ws_history: Vec::new(),
            hex_mode: true,
            show_timestamp: true,
            auto_scroll: true,
        }
    }
}

impl Default for SlaveConfig {
    fn default() -> Self {
        Self {
            slave_id: 1,
            tcp_port: 502,
            baud: 9600,
        }
    }
}

impl Default for NicConfig {
    fn default() -> Self {
        Self {
            last_adapter: String::new(),
            profiles: Vec::new(),
        }
    }
}

impl Default for NicAdapterProfiles {
    fn default() -> Self {
        Self {
            adapter: String::new(),
            profiles: Vec::new(),
        }
    }
}

impl Default for NicProfile {
    fn default() -> Self {
        Self {
            name: String::new(),
            ip: String::new(),
            // 原版 nettool 的默认掩码
            mask: "255.255.255.0".to_string(),
            gateway: String::new(),
            dns1: String::new(),
            dns2: String::new(),
        }
    }
}

// ---------------------------------------------------------------- 读写

/// 配置文件路径：exe 同目录下的 config.json。
/// 开发时 exe 在 target/debug/，配置就落在那里，不会污染源码目录。
pub fn config_path() -> PathBuf {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    exe_dir.join("config.json")
}

/// 解析配置文本：损坏或缺字段都回落到默认值（缺的字段用 Default 补）。
/// 框架阶段只在单测里用；以后导入老配置/做配置迁移时走这里。
#[allow(dead_code)]
pub fn parse(text: &str) -> AppConfig {
    serde_json::from_str::<AppConfig>(text).unwrap_or_default()
}

/// 读配置：文件缺失或损坏都回落到默认值。
/// 损坏时把旧文件改名成 config.json.bak（覆盖旧备份），然后返回默认配置。
pub fn load() -> AppConfig {
    let path = config_path();
    match fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str::<AppConfig>(&text) {
            Ok(cfg) => cfg,
            Err(_) => {
                let bak = path.with_extension("json.bak");
                let _ = fs::rename(&path, &bak);
                AppConfig::default()
            }
        },
        Err(_) => AppConfig::default(),
    }
}

/// 写配置：先写临时文件再改名，保证写一半断电不会留坏文件。
pub fn save(cfg: &AppConfig) -> Result<(), String> {
    let path = config_path();
    let text = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, text).map_err(|e| e.to_string())?;
    fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_round_trips() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.version, 1);
        assert_eq!(cfg.last_page, "debug");
        assert_eq!(cfg.debug.remote_port, 502);
        assert_eq!(cfg.debug.listen_port, 9000);
        assert_eq!(cfg.slave.slave_id, 1);
        assert_eq!(cfg.slave.tcp_port, 502);
        // 序列化再读回，必须一模一样
        let text = serde_json::to_string(&cfg).unwrap();
        let back: AppConfig = serde_json::from_str(&text).unwrap();
        assert_eq!(back.debug.ws_url, "ws://127.0.0.1:8080");
        assert_eq!(back.nic.profiles.len(), 0);
    }

    #[test]
    fn old_config_with_missing_fields_still_parses() {
        // 模拟老版本配置：只有 version 和 debug.protocol，其余字段都缺失
        let old = r#"{"version": 1, "debug": {"protocol": "modbus-tcp"}}"#;
        let cfg = parse(old);
        assert_eq!(cfg.debug.protocol, "modbus-tcp");
        // 缺失字段用默认值补齐
        assert_eq!(cfg.debug.remote_port, 502);
        assert_eq!(cfg.debug.listen_port, 9000);
        assert_eq!(cfg.last_page, "debug");
        assert_eq!(cfg.slave.tcp_port, 502);
    }

    #[test]
    fn corrupt_config_falls_back_to_default() {
        let cfg = parse("{ this is not json");
        assert_eq!(cfg.version, 1);
        assert_eq!(cfg.debug.remote_host, "127.0.0.1");
    }
}
