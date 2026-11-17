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

// ---------------------------------------------------------------- netsh（改 IP / DHCP / 重命名）
//
// Windows 专属：netsh interface ip ...，需要管理员权限（打包时用
// app.manifest 的 requireAdministrator 清单提权）。
// 每条命令 10 秒超时；失败停在当步并把 netsh 的报错翻成中文带回来。
// 非 Windows 上这些函数直接返回 Err（netsh 只在 Windows 上有），
// 界面上点按钮会如实提示，不会假装成功。

use crate::config::NicProfile;
#[cfg(windows)]
use std::process::Stdio;
#[cfg(windows)]
use std::time::{Duration, Instant};

/// 校验 IPv4（四段，每段 0-255）。
pub fn check_ipv4(text: &str) -> Result<(), String> {
    let parts: Vec<&str> = text.trim().split('.').collect();
    if parts.len() != 4 {
        return Err(format!("{text} 不是 IPv4 地址（要四段，如 192.168.1.11）"));
    }
    for p in parts {
        if p.is_empty()
            || p.len() > 3
            || !p.bytes().all(|b| b.is_ascii_digit())
            || p.parse::<u32>().unwrap_or(256) > 255
        {
            return Err(format!("{text} 不是 IPv4 地址"));
        }
    }
    Ok(())
}

fn static_ip_args(adapter: &str, ip: &str, mask: &str, gateway: &str) -> Vec<String> {
    let mut a = vec![
        "interface".into(),
        "ip".into(),
        "set".into(),
        "address".into(),
        format!("name={adapter}"),
        "static".into(),
        ip.into(),
        mask.into(),
    ];
    if !gateway.trim().is_empty() {
        a.push(gateway.trim().to_string());
        a.push("1".into()); // 网关跃点
    }
    a
}

fn set_dns_args(adapter: &str, dns: &str) -> Vec<String> {
    vec![
        "interface".into(),
        "ip".into(),
        "set".into(),
        "dns".into(),
        format!("name={adapter}"),
        "static".into(),
        dns.into(),
    ]
}

fn add_dns_args(adapter: &str, dns: &str) -> Vec<String> {
    vec![
        "interface".into(),
        "ip".into(),
        "add".into(),
        "dns".into(),
        format!("name={adapter}"),
        format!("addr={dns}"),
        "index=2".into(),
    ]
}

fn dhcp_address_args(adapter: &str) -> Vec<String> {
    vec![
        "interface".into(),
        "ip".into(),
        "set".into(),
        "address".into(),
        format!("name={adapter}"),
        "dhcp".into(),
    ]
}

fn dhcp_dns_args(adapter: &str) -> Vec<String> {
    vec![
        "interface".into(),
        "ip".into(),
        "set".into(),
        "dns".into(),
        format!("name={adapter}"),
        "dhcp".into(),
    ]
}

fn rename_args(old: &str, new: &str) -> Vec<String> {
    vec![
        "interface".into(),
        "set".into(),
        "interface".into(),
        format!("name={old}"),
        format!("newname={new}"),
    ]
}

/// 把 netsh 的报错翻成能看懂的中文（保留原文关键行）。
#[cfg(windows)]
fn netsh_error(text: &str) -> String {
    let t = text.trim();
    if t.to_lowercase().contains("requires elevation") || t.contains("需要提升") {
        return "需要管理员权限：请用管理员身份运行 Netdebug（打包时已加 requireAdministrator 清单，仍报这个错就是没提权）"
            .into();
    }
    let short: String = t.chars().take(200).collect();
    if short.is_empty() {
        "netsh 报错（没给原因）".into()
    } else {
        format!("netsh 报错：{short}")
    }
}

/// 跑一条 netsh 命令（10 秒超时）。成功返回 netsh 输出，失败返回中文原因。
fn run_netsh(args: &[String]) -> Result<String, String> {
    #[cfg(windows)]
    let r = run_netsh_windows(args);
    #[cfg(not(windows))]
    let r: Result<String, String> = {
        let _ = args;
        Err("netsh 只支持 Windows，在 Windows 上运行 Netdebug 才能改 IP".into())
    };
    r
}

#[cfg(windows)]
fn run_netsh_windows(args: &[String]) -> Result<String, String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut child = Command::new("netsh")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("启动 netsh 失败：{e}"))?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("netsh 超时（10 秒没回）".into());
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(format!("等 netsh 退出失败：{e}")),
        }
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("读 netsh 输出失败：{e}"))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if out.status.success() {
        Ok(text)
    } else {
        Err(netsh_error(&text))
    }
}

/// 应用静态 IP 档案：先校验，再按顺序调 netsh（地址 → 首选 DNS → 备用 DNS），
/// 哪步失败就停在哪步并返回原因。成功返回一行成功提示。
pub fn apply_static(adapter: &str, p: &NicProfile) -> Result<String, String> {
    let adapter = adapter.trim();
    if adapter.is_empty() {
        return Err("没选中适配器".into());
    }
    let ip = p.ip.trim();
    if ip.is_empty() {
        return Err("IP 地址不能为空！".into());
    }
    check_ipv4(ip)?;
    let mask = if p.mask.trim().is_empty() {
        "255.255.255.0".to_string()
    } else {
        check_ipv4(p.mask.trim())?;
        p.mask.trim().to_string()
    };
    let gateway = p.gateway.trim();
    if !gateway.is_empty() {
        check_ipv4(gateway)?;
    }
    let dns1 = p.dns1.trim();
    let dns2 = p.dns2.trim();
    if !dns1.is_empty() {
        check_ipv4(dns1)?;
    }
    if !dns2.is_empty() {
        check_ipv4(dns2)?;
    }
    run_netsh(&static_ip_args(adapter, ip, &mask, gateway))
        .map_err(|e| format!("设置静态 IP 失败：{e}"))?;
    if !dns1.is_empty() {
        run_netsh(&set_dns_args(adapter, dns1)).map_err(|e| format!("设置首选 DNS 失败：{e}"))?;
    }
    if !dns2.is_empty() {
        run_netsh(&add_dns_args(adapter, dns2)).map_err(|e| format!("设置备用 DNS 失败：{e}"))?;
    }
    Ok(format!("✓ 已应用 {ip}（静态 IP）"))
}

/// 恢复 DHCP（地址 + DNS 都改回自动获取）。
pub fn restore_dhcp(adapter: &str) -> Result<String, String> {
    let adapter = adapter.trim();
    if adapter.is_empty() {
        return Err("没选中适配器".into());
    }
    run_netsh(&dhcp_address_args(adapter)).map_err(|e| format!("恢复 DHCP 地址失败：{e}"))?;
    run_netsh(&dhcp_dns_args(adapter)).map_err(|e| format!("恢复 DHCP DNS 失败：{e}"))?;
    Ok("✓ 已恢复 DHCP（地址和 DNS 都自动获取）".into())
}

/// 重命名适配器。
pub fn rename_adapter(old: &str, new: &str) -> Result<String, String> {
    let old = old.trim();
    let new = new.trim();
    if old.is_empty() {
        return Err("没选中适配器".into());
    }
    if new.is_empty() {
        return Err("新名称不能为空".into());
    }
    if old == new {
        return Err("新名称和原来一样".into());
    }
    run_netsh(&rename_args(old, new)).map_err(|e| format!("重命名失败：{e}"))?;
    Ok(format!("✓ 已重命名为 {new}"))
}

/// 后台线程跑 apply_static；界面线程 try_recv 拿结果。
pub fn spawn_apply_static(adapter: String, p: NicProfile) -> Receiver<Result<String, String>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(apply_static(&adapter, &p));
    });
    rx
}

/// 后台线程跑 restore_dhcp。
pub fn spawn_restore_dhcp(adapter: String) -> Receiver<Result<String, String>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(restore_dhcp(&adapter));
    });
    rx
}

/// 后台线程跑 rename_adapter。
pub fn spawn_rename(old: String, new: String) -> Receiver<Result<String, String>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(rename_adapter(&old, &new));
    });
    rx
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

    #[test]
    fn ipv4_check() {
        assert!(check_ipv4("192.168.1.11").is_ok());
        assert!(check_ipv4("0.0.0.0").is_ok());
        assert!(check_ipv4("255.255.255.255").is_ok());
        assert!(check_ipv4("192.168.1").is_err());
        assert!(check_ipv4("192.168.1.256").is_err());
        assert!(check_ipv4("192.168.1.x").is_err());
        assert!(check_ipv4("").is_err());
    }

    #[test]
    fn netsh_args() {
        assert_eq!(
            static_ip_args("以太网 3", "192.168.1.11", "255.255.255.0", ""),
            vec![
                "interface",
                "ip",
                "set",
                "address",
                "name=以太网 3",
                "static",
                "192.168.1.11",
                "255.255.255.0"
            ]
        );
        assert_eq!(
            static_ip_args("以太网", "192.168.1.11", "255.255.255.0", "192.168.1.1"),
            vec![
                "interface",
                "ip",
                "set",
                "address",
                "name=以太网",
                "static",
                "192.168.1.11",
                "255.255.255.0",
                "192.168.1.1",
                "1"
            ]
        );
        assert_eq!(
            rename_args("以太网", "本体网口"),
            vec![
                "interface",
                "set",
                "interface",
                "name=以太网",
                "newname=本体网口"
            ]
        );
        assert_eq!(
            set_dns_args("以太网", "8.8.8.8"),
            vec![
                "interface",
                "ip",
                "set",
                "dns",
                "name=以太网",
                "static",
                "8.8.8.8"
            ]
        );
        assert_eq!(
            add_dns_args("以太网", "8.8.4.4"),
            vec![
                "interface",
                "ip",
                "add",
                "dns",
                "name=以太网",
                "addr=8.8.4.4",
                "index=2"
            ]
        );
    }

    #[test]
    fn apply_validates_without_running_netsh() {
        // 校验在调 netsh 之前，Linux 上也能跑（不真执行）。
        let bad = NicProfile {
            ip: "999.1.1.1".into(),
            ..Default::default()
        };
        assert!(apply_static("以太网", &bad).is_err());
        let empty = NicProfile {
            ip: "".into(),
            ..Default::default()
        };
        assert!(apply_static("以太网", &empty).is_err());
        assert!(apply_static("", &NicProfile::default()).is_err());
        assert!(rename_adapter("以太网", "").is_err());
        assert!(rename_adapter("以太网", "以太网").is_err());
        assert!(restore_dhcp("").is_err());
    }
}
