//! 网卡只读后端：PowerShell Get-NetAdapter / Get-NetIPConfiguration。
//!
//! 只枚举和读地址，不改系统网络，不需要管理员权限。
//! 后台线程跑 PowerShell（约 1 秒），结果经 mpsc 送回界面线程。

use std::process::Command;
use std::sync::mpsc::{self, Receiver};

#[derive(Clone, Debug, Default)]
pub struct AdapterInfo {
    pub name: String,
    pub up: bool,
    pub ip: String,
    pub mask: String,
    pub gateway: String,
    pub dns1: String,
    pub dns2: String,
}

const SCRIPT: &str = "[Console]::OutputEncoding=[System.Text.Encoding]::UTF8; \
$ad = Get-NetAdapter; \
$cf = Get-NetIPConfiguration; \
@($ad | ForEach-Object { \
$i = $_; \
$c = $cf | Where-Object InterfaceIndex -eq $i.ifIndex | Select-Object -First 1; \
$a = $c.IPv4Address | Select-Object -First 1; \
[pscustomobject]@{ \
name = $i.Name; \
status = $i.Status.ToString(); \
ip = $a.IPAddress; \
prefix = $a.PrefixLength; \
gw = ($c.IPv4DefaultGateway | Select-Object -First 1).NextHop; \
dns = @($c.DNSServer | Where-Object { $_.AddressFamily -eq 2 } | Select-Object -ExpandProperty ServerAddresses) \
} }) | ConvertTo-Json -Compress -Depth 5";

/// 起后台线程枚举网卡；接收端在界面线程 try_recv。
pub fn spawn_fetch() -> Receiver<Vec<AdapterInfo>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let infos = fetch_blocking().unwrap_or_default();
        let _ = tx.send(infos);
    });
    rx
}

fn fetch_blocking() -> Option<Vec<AdapterInfo>> {
    // Windows 专属：不弹控制台窗口（CREATE_NO_WINDOW 只在 Windows 上存在）。
    // 非 Windows 上跳过这一步，`powershell.exe` 不存在，`output()` 直接失败，
    // 返回 None，和 Windows 上取不到网卡时一样；Windows 上的行为原样不动。
    #[cfg(windows)]
    use std::os::windows::process::CommandExt;
    #[cfg(windows)]
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut cmd = Command::new("powershell.exe");
    cmd.args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT]);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    parse(&String::from_utf8_lossy(&out.stdout))
}

/// 解析 PowerShell 的 JSON（单网卡时 ConvertTo-Json 会退化成对象而非数组）。
pub fn parse(text: &str) -> Option<Vec<AdapterInfo>> {
    let v: serde_json::Value = serde_json::from_str(text.trim()).ok()?;
    let items = match &v {
        serde_json::Value::Array(a) => a.clone(),
        serde_json::Value::Object(_) => vec![v],
        _ => return None,
    };
    let mut list = Vec::new();
    for it in items {
        let s = |k: &str| it[k].as_str().unwrap_or("").to_string();
        let prefix = it["prefix"].as_u64().unwrap_or(0) as u32;
        let dns: Vec<String> = it["dns"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|d| d.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        list.push(AdapterInfo {
            name: s("name"),
            up: s("status") == "Up",
            ip: s("ip"),
            mask: mask_from_prefix(prefix),
            gateway: s("gw"),
            dns1: dns.first().cloned().unwrap_or_default(),
            dns2: dns.get(1).cloned().unwrap_or_default(),
        });
    }
    Some(list)
}

pub fn mask_from_prefix(prefix: u32) -> String {
    if prefix > 32 {
        return String::new();
    }
    let m = if prefix == 0 {
        0
    } else {
        u32::MAX.checked_shl(32 - prefix).unwrap_or(0)
    };
    format!(
        "{}.{}.{}.{}",
        (m >> 24) & 255,
        (m >> 16) & 255,
        (m >> 8) & 255,
        m & 255
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_math() {
        assert_eq!(mask_from_prefix(24), "255.255.255.0");
        assert_eq!(mask_from_prefix(16), "255.255.0.0");
        assert_eq!(mask_from_prefix(0), "0.0.0.0");
        assert_eq!(mask_from_prefix(32), "255.255.255.255");
    }

    #[test]
    fn parse_array_and_single() {
        let one = r#"{"name":"Wifi","status":"Up","ip":"192.168.1.5","prefix":24,"gw":"192.168.1.1","dns":["192.168.1.1","8.8.8.8"]}"#;
        let list = parse(one).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "Wifi");
        assert!(list[0].up);
        assert_eq!(list[0].mask, "255.255.255.0");
        assert_eq!(list[0].dns2, "8.8.8.8");
        let arr = format!("[{one},{one}]");
        assert_eq!(parse(&arr).unwrap().len(), 2);
        assert!(parse("not json").is_none());
    }

    #[test]
    fn parse_null_ip() {
        let v = r#"{"name":"Ethernet","status":"Disconnected","ip":null,"prefix":null,"gw":null,"dns":[]}"#;
        let list = parse(v).unwrap();
        assert_eq!(list[0].ip, "");
        assert!(!list[0].up);
    }
}
