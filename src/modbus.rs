//! Modbus TCP 主站：请求打包、响应解析、后台轮询线程。
//!
//! RTU 串口暂未接（需要串口依赖），界面侧对 RTU 弹 stub 提示。
//! 轮询用 industrial::IntervalTimer（落后不追赶），结果走有界通道。

use crate::industrial::{bounded_channel, IntervalTimer};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct MasterJob {
    pub host: String,
    pub port: u16,
    pub unit: u8,
    pub fc: u8,
    pub addr: u16,
    pub qty: u16,
    /// 写功能码的值（05/06 单个，15/16 多个）
    pub values: Vec<u16>,
    pub interval_ms: u64,
    /// true = 只发一次就退出
    pub once: bool,
}

#[derive(Debug)]
pub enum MasterMsg {
    /// 读成功：线圈/离散用 bits，寄存器用 regs
    Read { addr: u16, bits: Vec<bool>, regs: Vec<u16> },
    Log(String),
    Stopped,
}

pub fn is_write(fc: u8) -> bool {
    matches!(fc, 5 | 6 | 15 | 16)
}

pub fn is_bits(fc: u8) -> bool {
    matches!(fc, 1 | 2)
}

/// PDU 部分（功能码 + 地址/数量/值），TCP 和 RTU 共用。
pub fn build_pdu(job: &MasterJob) -> Vec<u8> {
    let mut pdu = vec![job.fc];
    pdu.extend_from_slice(&job.addr.to_be_bytes());
    match job.fc {
        1 | 2 | 3 | 4 => pdu.extend_from_slice(&job.qty.to_be_bytes()),
        5 => pdu.extend_from_slice(&if job.values.first().copied().unwrap_or(0) != 0 {
            0xFF00u16
        } else {
            0x0000u16
        }
        .to_be_bytes()),
        6 => pdu.extend_from_slice(&job.values.first().copied().unwrap_or(0).to_be_bytes()),
        15 => {
            pdu.extend_from_slice(&job.qty.to_be_bytes());
            let n = (job.qty as usize + 7) / 8;
            pdu.push(n as u8);
            let mut bytes = vec![0u8; n];
            for i in 0..job.qty as usize {
                if job.values.get(i).copied().unwrap_or(0) != 0 {
                    bytes[i / 8] |= 1 << (i % 8);
                }
            }
            pdu.extend_from_slice(&bytes);
        }
        16 => {
            pdu.extend_from_slice(&job.qty.to_be_bytes());
            pdu.push((job.qty * 2) as u8);
            for i in 0..job.qty as usize {
                let v = job.values.get(i).copied().unwrap_or(0);
                pdu.extend_from_slice(&v.to_be_bytes());
            }
        }
        _ => {}
    }
    pdu
}

/// MBAP + PDU 请求帧（Modbus TCP）。
pub fn build_request(tid: u16, job: &MasterJob) -> Vec<u8> {
    let pdu = build_pdu(job);
    let mut frame = Vec::with_capacity(7 + pdu.len());
    frame.extend_from_slice(&tid.to_be_bytes());
    frame.extend_from_slice(&0u16.to_be_bytes());
    frame.extend_from_slice(&((pdu.len() + 1) as u16).to_be_bytes());
    frame.push(job.unit);
    frame.extend_from_slice(&pdu);
    frame
}

/// 按功能码把数量夹到合法范围（读线圈 2000 / 读寄存器 125 / 写多线圈 1968 / 写多寄存器 123）。
pub fn clamp_qty(fc: u8, qty: u16) -> u16 {
    match fc {
        1 | 2 => qty.clamp(1, 2000),
        3 | 4 => qty.clamp(1, 125),
        15 => qty.clamp(1, 1968),
        16 => qty.clamp(1, 123),
        _ => 1,
    }
}

/// 解析写值：按 `,` / 空格 / `;` / `，` 切，接受十进制、`0x` 十六进制、
/// `on|off|true|false`（线圈 1/0）。解析失败的当 0。
pub fn parse_values(text: &str) -> Vec<u16> {
    parse_values_strict(text).unwrap_or_else(|_| {
        text.split(|c| c == ',' || c == ' ' || c == ';' || c == '，' || c == '\t' || c == '\n')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| parse_single_value(s).unwrap_or(0))
            .collect()
    })
}

fn parse_single_value(s: &str) -> Result<u16, String> {
    match s.to_ascii_lowercase().as_str() {
        "on" | "true" => Ok(1),
        "off" | "false" => Ok(0),
        _ => {
            if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
                u16::from_str_radix(h, 16).map_err(|_| format!("写入值“{s}”不是有效的十六进制 0-FFFF"))
            } else {
                s.parse::<u16>()
                    .map_err(|_| format!("写入值“{s}”不是 0-65535"))
            }
        }
    }
}

/// 严格解析写值（支持十进制、`0x` 十六进制、`on|off|true|false`），空或非法返回 Err。
pub fn parse_values_strict(text: &str) -> Result<Vec<u16>, String> {
    let vals: Vec<u16> = text
        .split(|c| c == ',' || c == ' ' || c == ';' || c == '，' || c == '\t' || c == '\n')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(parse_single_value)
        .collect::<Result<_, _>>()?;
    if vals.is_empty() {
        return Err("写入值不能为空".into());
    }
    Ok(vals)
}

/// 解析响应 PDU（不含 MBAP）。异常码返回 Err。
pub fn parse_response(fc: u8, pdu: &[u8]) -> Result<(Vec<bool>, Vec<u16>), String> {
    let Some(&rfc) = pdu.first() else {
        return Err("响应为空".into());
    };
    if rfc == fc | 0x80 {
        let code = pdu.get(1).copied().unwrap_or(0);
        return Err(format!(
            "异常响应 0x{rfc:02X}，异常码 {code}（{}）",
            match code {
                1 => "非法功能码",
                2 => "非法数据地址",
                3 => "非法数据值",
                4 => "从站设备故障",
                _ => "未知",
            }
        ));
    }
    if rfc != fc {
        return Err(format!("功能码不匹配：期望 {fc:02X} 收到 {rfc:02X}"));
    }
    match fc {
        1 | 2 => {
            let n = *pdu.get(1).ok_or("响应缺字节数")? as usize;
            let data = pdu.get(2..2 + n).ok_or("响应数据长度不足")?;
            Ok((data.iter().flat_map(|b| (0..8).map(move |i| b & (1 << i) != 0)).collect(), Vec::new()))
        }
        3 | 4 => {
            let n = *pdu.get(1).ok_or("响应缺字节数")? as usize;
            let data = pdu.get(2..2 + n).ok_or("响应数据长度不足")?;
            if n % 2 != 0 {
                return Err("寄存器字节数为奇数".into());
            }
            Ok((Vec::new(), data.chunks(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect()))
        }
        5 | 6 | 15 | 16 => Ok((Vec::new(), Vec::new())),
        _ => Err(format!("不支持的功能码 {fc:02X}")),
    }
}

/// 起后台线程：连接 → 按周期发请求 → 结果送通道。stop 置 true 后退出。
pub fn spawn_poller(job: MasterJob, stop: Arc<AtomicBool>) -> Receiver<MasterMsg> {
    let (tx, rx) = bounded_channel::<MasterMsg>(64);
    std::thread::spawn(move || {
        let addr = format!("{}:{}", job.host, job.port);
        let mut stream = match TcpStream::connect(&addr) {
            Ok(s) => s,
            Err(e) => {
                let _ = tx.try_send(MasterMsg::Log(format!("✗ 连接 {addr} 失败：{e}")));
                let _ = tx.try_send(MasterMsg::Stopped);
                return;
            }
        };
        let _ = stream.set_read_timeout(Some(Duration::from_millis(1000)));
        let _ = stream.set_write_timeout(Some(Duration::from_millis(1000)));
        let _ = tx.try_send(MasterMsg::Log(format!("✓ 已连接 {addr}")));
        let mut timer = IntervalTimer::new(Duration::from_millis(job.interval_ms.max(20)));
        let mut tid = 0u16;
        loop {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            if timer.due() {
                tid = tid.wrapping_add(1);
                let req = build_request(tid, &job);
                let label = format!(
                    "→ FC{:02} 地址={} 数量={}",
                    job.fc, job.addr, job.qty
                );
                let result = stream
                    .write_all(&req)
                    .and_then(|_| read_frame(&mut stream, tid, job.unit))
                    .map_err(|e| {
                        let fatal = is_fatal_io_err(&e);
                        (format!("✗ {label} 失败：{e}"), fatal)
                    })
                    .and_then(|pdu| {
                        parse_response(job.fc, &pdu).map_err(|e| (format!("✗ {label} {e}"), false))
                    });
                match result {
                    Ok((bits, regs)) => {
                        if is_write(job.fc) {
                            let _ = tx.try_send(MasterMsg::Log(format!("{label} ✓ 写入确认")));
                        } else {
                            let _ = tx.try_send(MasterMsg::Log(format!(
                                "{label} ✓ {} 项",
                                if is_bits(job.fc) {
                                    bits.len()
                                } else {
                                    regs.len()
                                }
                            )));
                            let _ = tx.try_send(MasterMsg::Read {
                                addr: job.addr,
                                bits,
                                regs,
                            });
                        }
                    }
                    Err((e, fatal)) => {
                        let _ = tx.try_send(MasterMsg::Log(e));
                        if fatal {
                            let _ = tx.try_send(MasterMsg::Log("✗ 连接已断开，轮询停止".into()));
                            break;
                        }
                    }
                }
                if job.once {
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let _ = tx.try_send(MasterMsg::Stopped);
    });
    rx
}

/// 起后台线程（RTU）：打开串口 → 按周期发 RTU 请求 → 结果送通道。
/// 响应用 3.5 字符静默间隔判帧尾、验 CRC；超时 1 秒。stop 置 true 后退出。
pub fn spawn_rtu_poller(
    job: MasterJob,
    settings: crate::rtu::SerialSettings,
    stop: Arc<AtomicBool>,
) -> Receiver<MasterMsg> {
    let (tx, rx) = bounded_channel::<MasterMsg>(64);
    std::thread::spawn(move || {
        let mut port = match settings.open() {
            Ok(p) => p,
            Err(e) => {
                let _ = tx.try_send(MasterMsg::Log(format!("✗ {e}")));
                let _ = tx.try_send(MasterMsg::Stopped);
                return;
            }
        };
        let t35 = crate::rtu::char_time_3_5(settings.baud);
        let _ = tx.try_send(MasterMsg::Log(format!(
            "✓ 串口 {} 已打开（{} 波特，8/N/1）",
            settings.port, settings.baud
        )));
        let mut timer = IntervalTimer::new(Duration::from_millis(job.interval_ms.max(20)));
        loop {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            if timer.due() {
                let pdu = build_pdu(&job);
                let label = format!("→ RTU FC{:02} 地址={} 数量={}", job.fc, job.addr, job.qty);
                let result = crate::rtu::write_rtu_frame(port.as_mut(), job.unit, &pdu)
                    .map_err(|e| format!("✗ {label} 失败：{e}"))
                    .and_then(|_| {
                        crate::rtu::read_rtu_frame(port.as_mut(), t35, Duration::from_millis(1000))
                            .map_err(|e| format!("✗ {label} {e}"))
                    })
                    .and_then(|frame| {
                        crate::rtu::strip_rtu_frame(&frame).map_err(|e| format!("✗ {label} {e}"))
                    })
                    .and_then(|(unit, pdu)| {
                        if unit != job.unit {
                            return Err(format!(
                                "✗ {label} 从站地址不匹配：期望 {} 收到 {unit}",
                                job.unit
                            ));
                        }
                        parse_response(job.fc, &pdu).map_err(|e| format!("✗ {label} {e}"))
                    });
                match result {
                    Ok((bits, regs)) => {
                        if is_write(job.fc) {
                            let _ = tx.try_send(MasterMsg::Log(format!("{label} ✓ 写入确认")));
                        } else {
                            let _ = tx.try_send(MasterMsg::Log(format!(
                                "{label} ✓ {} 项",
                                if is_bits(job.fc) {
                                    bits.len()
                                } else {
                                    regs.len()
                                }
                            )));
                            let _ = tx.try_send(MasterMsg::Read {
                                addr: job.addr,
                                bits,
                                regs,
                            });
                        }
                    }
                    Err(e) => {
                        let _ = tx.try_send(MasterMsg::Log(e));
                    }
                }
                if job.once {
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let _ = tx.try_send(MasterMsg::Stopped);
    });
    rx
}

fn is_fatal_io_err(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::UnexpectedEof
            | std::io::ErrorKind::NotConnected
    )
}

/// 读一帧：MBAP 头 6 字节 + 长度指定的剩余部分，并校验协议号、TID 与从站地址。
fn read_frame(
    stream: &mut TcpStream,
    expected_tid: u16,
    expected_unit: u8,
) -> std::io::Result<Vec<u8>> {
    let mut head = [0u8; 6];
    stream.read_exact(&mut head)?;
    let tid = u16::from_be_bytes([head[0], head[1]]);
    let proto = u16::from_be_bytes([head[2], head[3]]);
    let len = u16::from_be_bytes([head[4], head[5]]) as usize;
    if proto != 0 || len < 2 || len > 260 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("MBAP 头非法（proto={proto}, len={len}）"),
        ));
    }
    let mut rest = vec![0u8; len];
    stream.read_exact(&mut rest)?;
    if tid != expected_tid {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("TID 不匹配：期望 {expected_tid} 收到 {tid}"),
        ));
    }
    if rest[0] != expected_unit {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("从站地址不匹配：期望 {expected_unit} 收到 {}", rest[0]),
        ));
    }
    Ok(rest[1..].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(fc: u8) -> MasterJob {
        MasterJob {
            host: "127.0.0.1".into(),
            port: 502,
            unit: 1,
            fc,
            addr: 0,
            qty: 10,
            values: vec![],
            interval_ms: 100,
            once: true,
        }
    }

    #[test]
    fn read_request_frame() {
        let f = build_request(7, &job(3));
        assert_eq!(f, vec![0, 7, 0, 0, 0, 6, 1, 3, 0, 0, 0, 10]);
    }

    #[test]
    fn write_multi_regs_frame() {
        let mut j = job(16);
        j.qty = 2;
        j.values = vec![0x1234, 0x5678];
        let f = build_request(1, &j);
        assert_eq!(f, vec![0, 1, 0, 0, 0, 11, 1, 16, 0, 0, 0, 2, 4, 0x12, 0x34, 0x56, 0x78]);
    }

    #[test]
    fn write_multi_regs_pads_missing_values_to_match_qty() {
        let mut j = job(16);
        j.qty = 3;
        j.values = vec![0x1234];
        let pdu = build_pdu(&j);
        // byte_count = 6, 后面必须有 6 个字节数据（缺失补 0）
        assert_eq!(pdu, vec![16, 0, 0, 0, 3, 6, 0x12, 0x34, 0, 0, 0, 0]);
    }

    #[test]
    fn parse_values_strict_supports_hex_and_bool() {
        assert_eq!(
            parse_values_strict("10, 0x0064, on, off").unwrap(),
            vec![10, 100, 1, 0]
        );
        assert!(parse_values_strict("").is_err());
        assert!(parse_values_strict("abc").is_err());
    }

    #[test]
    fn write_coil_frame() {
        let mut j = job(5);
        j.values = vec![1];
        let f = build_request(1, &j);
        assert_eq!(f[10..], [0xFF, 0x00]);
    }

    #[test]
    fn parse_regs_and_bits() {
        let (bits, regs) = parse_response(3, &[3, 4, 0x12, 0x34, 0x56, 0x78]).unwrap();
        assert!(bits.is_empty());
        assert_eq!(regs, vec![0x1234, 0x5678]);
        let (bits, regs) = parse_response(1, &[1, 1, 0b0000_0101]).unwrap();
        assert!(regs.is_empty());
        assert_eq!(bits, vec![true, false, true, false, false, false, false, false]);
    }

    #[test]
    fn parse_exception() {
        let e = parse_response(3, &[0x83, 2]).unwrap_err();
        assert!(e.contains("非法数据地址"));
    }

    #[test]
    fn poller_roundtrip_against_fake_slave() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 256];
            let _n = s.read(&mut buf).unwrap();
            let pdu = [3u8, 4, 0x00, 0x0A, 0x00, 0x14];
            let mut resp = vec![buf[0], buf[1], 0, 0];
            resp.extend_from_slice(&((pdu.len() + 1) as u16).to_be_bytes());
            resp.push(1);
            resp.extend_from_slice(&pdu);
            s.write_all(&resp).unwrap();
        });
        let mut j = job(3);
        j.port = port;
        j.qty = 2;
        let rx = spawn_poller(j, Arc::new(AtomicBool::new(false)));
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut got = None;
        while std::time::Instant::now() < deadline {
            match rx.try_recv() {
                Ok(MasterMsg::Read { regs, .. }) => {
                    got = Some(regs);
                    break;
                }
                Ok(_) => {}
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        assert_eq!(got, Some(vec![10, 20]));
    }

    #[test]
    fn poller_stops_on_connection_drop() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            drop(s);
        });
        let mut j = job(3);
        j.port = port;
        j.once = false;
        j.interval_ms = 20;
        let rx = spawn_poller(j, Arc::new(AtomicBool::new(false)));
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let mut stopped = false;
        while std::time::Instant::now() < deadline {
            match rx.try_recv() {
                Ok(MasterMsg::Stopped) => {
                    stopped = true;
                    break;
                }
                Ok(_) => {}
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        assert!(stopped, "连接断开后轮询线程应退出并发出 Stopped");
    }

    #[test]
    fn poller_rejects_mismatched_tid() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 256];
            let _ = s.read(&mut buf).unwrap();
            // 故意回错 TID (0x99, 0x99)
            let resp = [0x99u8, 0x99, 0, 0, 0, 5, 1, 3, 2, 0, 1];
            s.write_all(&resp).unwrap();
        });
        let mut j = job(3);
        j.port = port;
        j.qty = 1;
        let rx = spawn_poller(j, Arc::new(AtomicBool::new(false)));
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let mut got_tid_err = false;
        while std::time::Instant::now() < deadline {
            match rx.try_recv() {
                Ok(MasterMsg::Log(msg)) if msg.contains("TID 不匹配") => {
                    got_tid_err = true;
                    break;
                }
                Ok(_) => {}
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        assert!(got_tid_err, "应拦截 TID 不匹配的响应帧");
    }
}