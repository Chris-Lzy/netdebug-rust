//! Modbus 从站模拟：TCP 监听 + RTU 串口，四区数据表共享。
//!
//! 程序当从站，响应主站的读写：功能码 01/02/03/04 读、05/06/15/16 写，
//! 不支持的报异常码（01 非法功能码 / 02 非法数据地址 / 03 非法数据值）。
//! 四区数据表（线圈 / 离散输入 / 保持寄存器 / 输入寄存器）由界面和
//! 后台线程共享（`Arc<Mutex<SlaveData>>`），界面改值立即生效，
//! 主站写进来的值界面上也立即能看到。
//! TCP 监听 0.0.0.0（局域网里的主站要能连进来，日志里会写明）；
//! RTU 走 rtu.rs（CRC16 + 3.5 字符静默判帧）。

use crate::industrial::bounded_channel;
use crate::rtu::SerialSettings;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 四区数据表（默认各 16 项，和界面一致）。
#[derive(Debug, Clone)]
pub struct SlaveData {
    pub coils: Vec<bool>,
    pub discrete: Vec<bool>,
    pub holding: Vec<u16>,
    pub input: Vec<u16>,
}

impl SlaveData {
    pub fn new(n: usize) -> Self {
        Self {
            coils: vec![false; n],
            discrete: vec![false; n],
            holding: vec![0; n],
            input: vec![0; n],
        }
    }
}

/// 后台发回界面的消息。
#[derive(Debug)]
pub enum SlaveMsg {
    /// 请求/连接日志行。
    Log(String),
    /// 服务已停（界面收到后把 running 置 false）。
    Stopped,
}

/// 运行中的从站服务：停它只要把 stop 置 true。
pub struct SlaveHandle {
    pub stop: Arc<AtomicBool>,
    pub rx: std::sync::mpsc::Receiver<SlaveMsg>,
    /// TCP 实际监听端口（传 0 时由系统分配，界面显示用）。
    pub port: u16,
}

fn exc(fc: u8, code: u8) -> Vec<u8> {
    vec![fc | 0x80, code]
}

fn exc_name(code: u8) -> &'static str {
    match code {
        1 => "非法功能码",
        2 => "非法数据地址",
        3 => "非法数据值",
        4 => "从站设备故障",
        _ => "未知",
    }
}

fn read_u16(pdu: &[u8], at: usize) -> Option<u16> {
    pdu.get(at..at + 2)
        .map(|b| u16::from_be_bytes([b[0], b[1]]))
}

/// 处理一个 PDU，返回（响应 PDU，日志行）。
/// 只动 `data` 里的表，不碰网络和串口，TCP 和 RTU 从站共用。
pub fn handle_pdu(data: &mut SlaveData, pdu: &[u8]) -> (Vec<u8>, String) {
    let Some(&fc) = pdu.first() else {
        return (exc(0, 3), "✗ 空请求".into());
    };
    match fc {
        1 | 2 => {
            let (Some(addr), Some(qty)) = (read_u16(pdu, 1), read_u16(pdu, 3)) else {
                return (exc(fc, 3), format!("✗ FC{fc:02} 请求太短（非法数据值）"));
            };
            let table = if fc == 1 { &data.coils } else { &data.discrete };
            if qty == 0 || qty > 2000 || addr as usize + qty as usize > table.len() {
                return (
                    exc(fc, 2),
                    format!("✗ FC{fc:02} 地址={addr} 数量={qty} 异常码 2（非法数据地址）"),
                );
            }
            let nbytes = (qty as usize + 7) / 8;
            let mut resp = vec![fc, nbytes as u8];
            let mut packed = vec![0u8; nbytes];
            for i in 0..qty as usize {
                if table[addr as usize + i] {
                    packed[i / 8] |= 1 << (i % 8);
                }
            }
            resp.extend_from_slice(&packed);
            (
                resp,
                format!("→ FC{fc:02} 地址={addr} 数量={qty} ✓ {} 项", qty),
            )
        }
        3 | 4 => {
            let (Some(addr), Some(qty)) = (read_u16(pdu, 1), read_u16(pdu, 3)) else {
                return (exc(fc, 3), format!("✗ FC{fc:02} 请求太短（非法数据值）"));
            };
            let table = if fc == 3 { &data.holding } else { &data.input };
            if qty == 0 || qty > 125 || addr as usize + qty as usize > table.len() {
                return (
                    exc(fc, 2),
                    format!("✗ FC{fc:02} 地址={addr} 数量={qty} 异常码 2（非法数据地址）"),
                );
            }
            let mut resp = vec![fc, (qty * 2) as u8];
            for i in 0..qty as usize {
                resp.extend_from_slice(&table[addr as usize + i].to_be_bytes());
            }
            (
                resp,
                format!("→ FC{fc:02} 地址={addr} 数量={qty} ✓ {} 项", qty),
            )
        }
        5 => {
            let (Some(addr), Some(value)) = (read_u16(pdu, 1), read_u16(pdu, 3)) else {
                return (exc(fc, 3), format!("✗ FC{fc:02} 请求太短（非法数据值）"));
            };
            if addr as usize >= data.coils.len() {
                return (
                    exc(fc, 2),
                    format!("✗ FC{fc:02} 地址={addr} 异常码 2（非法数据地址）"),
                );
            }
            if value != 0xFF00 && value != 0x0000 {
                return (
                    exc(fc, 3),
                    format!("✗ FC{fc:02} 地址={addr} 值={value:#06X} 异常码 3（非法数据值）"),
                );
            }
            data.coils[addr as usize] = value == 0xFF00;
            (
                vec![
                    fc,
                    (addr >> 8) as u8,
                    (addr & 0xFF) as u8,
                    (value >> 8) as u8,
                    (value & 0xFF) as u8,
                ],
                format!("→ FC{fc:02} 地址={addr} 值={} ✓", value == 0xFF00),
            )
        }
        6 => {
            let (Some(addr), Some(value)) = (read_u16(pdu, 1), read_u16(pdu, 3)) else {
                return (exc(fc, 3), format!("✗ FC{fc:02} 请求太短（非法数据值）"));
            };
            if addr as usize >= data.holding.len() {
                return (
                    exc(fc, 2),
                    format!("✗ FC{fc:02} 地址={addr} 异常码 2（非法数据地址）"),
                );
            }
            data.holding[addr as usize] = value;
            (
                vec![
                    fc,
                    (addr >> 8) as u8,
                    (addr & 0xFF) as u8,
                    (value >> 8) as u8,
                    (value & 0xFF) as u8,
                ],
                format!("→ FC{fc:02} 地址={addr} 值={value} ✓"),
            )
        }
        15 => {
            let (Some(addr), Some(qty)) = (read_u16(pdu, 1), read_u16(pdu, 3)) else {
                return (exc(fc, 3), format!("✗ FC{fc:02} 请求太短（非法数据值）"));
            };
            let nbytes = (qty as usize + 7) / 8;
            let Some(&count) = pdu.get(5) else {
                return (exc(fc, 3), format!("✗ FC{fc:02} 请求太短（非法数据值）"));
            };
            if qty == 0
                || qty > 1968
                || count as usize != nbytes
                || pdu.len() < 6 + nbytes
                || addr as usize + qty as usize > data.coils.len()
            {
                return (
                    exc(fc, 2),
                    format!("✗ FC{fc:02} 地址={addr} 数量={qty} 异常码 2（非法数据地址）"),
                );
            }
            for i in 0..qty as usize {
                data.coils[addr as usize + i] = pdu[6 + i / 8] & (1 << (i % 8)) != 0;
            }
            (
                vec![
                    fc,
                    (addr >> 8) as u8,
                    (addr & 0xFF) as u8,
                    (qty >> 8) as u8,
                    (qty & 0xFF) as u8,
                ],
                format!("→ FC{fc:02} 地址={addr} 数量={qty} ✓"),
            )
        }
        16 => {
            let (Some(addr), Some(qty)) = (read_u16(pdu, 1), read_u16(pdu, 3)) else {
                return (exc(fc, 3), format!("✗ FC{fc:02} 请求太短（非法数据值）"));
            };
            let Some(&count) = pdu.get(5) else {
                return (exc(fc, 3), format!("✗ FC{fc:02} 请求太短（非法数据值）"));
            };
            if qty == 0
                || qty > 123
                || count as usize != qty as usize * 2
                || pdu.len() < 6 + qty as usize * 2
                || addr as usize + qty as usize > data.holding.len()
            {
                return (
                    exc(fc, 2),
                    format!("✗ FC{fc:02} 地址={addr} 数量={qty} 异常码 2（非法数据地址）"),
                );
            }
            for i in 0..qty as usize {
                let at = 6 + i * 2;
                data.holding[addr as usize + i] = u16::from_be_bytes([pdu[at], pdu[at + 1]]);
            }
            (
                vec![
                    fc,
                    (addr >> 8) as u8,
                    (addr & 0xFF) as u8,
                    (qty >> 8) as u8,
                    (qty & 0xFF) as u8,
                ],
                format!("→ FC{fc:02} 地址={addr} 数量={qty} ✓"),
            )
        }
        _ => (
            exc(fc, 1),
            format!("✗ FC{fc:02} 异常码 1（{}）", exc_name(1)),
        ),
    }
}

/// 启动 TCP 从站（监听 0.0.0.0:port，局域网主站能连进来）。
/// 端口传 0 由系统分配，实际端口在返回的 handle.port 里。
/// 监听失败（比如端口被占）直接返回 Err，界面上弹提示。
pub fn start_tcp_slave(
    port: u16,
    unit: u8,
    data: Arc<Mutex<SlaveData>>,
) -> Result<SlaveHandle, String> {
    let listener = TcpListener::bind(("0.0.0.0", port))
        .map_err(|e| format!("监听端口 {port} 失败：{e}（可能被占用）"))?;
    let actual_port = listener.local_addr().map(|a| a.port()).unwrap_or(port);
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("监听端口 {port} 失败：{e}"))?;
    let (tx, rx) = bounded_channel::<SlaveMsg>(64);
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    let _ = tx.try_send(SlaveMsg::Log(format!(
        "✓ 从站已启动：TCP 0.0.0.0:{actual_port}，站地址 {unit}（局域网可访问）"
    )));
    std::thread::spawn(move || {
        loop {
            if stop2.load(Ordering::Relaxed) {
                break;
            }
            match listener.accept() {
                Ok((stream, addr)) => {
                    let _ = tx.try_send(SlaveMsg::Log(format!("→ 客户端 {addr} 已连接")));
                    let txc = tx.clone();
                    let datac = data.clone();
                    let stopc = stop2.clone();
                    std::thread::spawn(move || {
                        handle_tcp_client(stream, addr, unit, datac, stopc, txc);
                    });
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(e) => {
                    let _ = tx.try_send(SlaveMsg::Log(format!("✗ 监听出错：{e}")));
                    break;
                }
            }
        }
        let _ = tx.try_send(SlaveMsg::Stopped);
    });
    Ok(SlaveHandle {
        stop,
        rx,
        port: actual_port,
    })
}

/// 一个 TCP 客户端的收发循环：读 MBAP + PDU → 处理 → 回响应。
fn handle_tcp_client(
    mut stream: TcpStream,
    addr: std::net::SocketAddr,
    unit: u8,
    data: Arc<Mutex<SlaveData>>,
    stop: Arc<AtomicBool>,
    tx: std::sync::mpsc::SyncSender<SlaveMsg>,
) {
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let mut head = [0u8; 7];
        match stream.read_exact(&mut head) {
            Ok(()) => {}
            Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => continue,
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(_) => break,
        }
        let tid = u16::from_be_bytes([head[0], head[1]]);
        let proto = u16::from_be_bytes([head[2], head[3]]);
        let len = u16::from_be_bytes([head[4], head[5]]) as usize;
        let req_unit = head[6];
        if proto != 0 || len < 2 || len > 261 {
            let _ = tx.try_send(SlaveMsg::Log(format!("✗ {addr} 发来非法 MBAP 头，已断开")));
            break;
        }
        let mut pdu = vec![0u8; len - 1];
        match stream.read_exact(&mut pdu) {
            Ok(()) => {}
            Err(_) => break,
        }
        if req_unit != unit {
            let _ = tx.try_send(SlaveMsg::Log(format!(
                "→ {addr} 站地址 {req_unit} 不是本站（{unit}），已忽略"
            )));
            continue;
        }
        let (resp_pdu, line) = {
            let mut data = data.lock().unwrap_or_else(|e| e.into_inner());
            handle_pdu(&mut data, &pdu)
        };
        let _ = tx.try_send(SlaveMsg::Log(line));
        let mut resp = Vec::with_capacity(7 + resp_pdu.len());
        resp.extend_from_slice(&tid.to_be_bytes());
        resp.extend_from_slice(&0u16.to_be_bytes());
        resp.extend_from_slice(&((resp_pdu.len() + 1) as u16).to_be_bytes());
        resp.push(unit);
        resp.extend_from_slice(&resp_pdu);
        if stream.write_all(&resp).is_err() {
            break;
        }
    }
    let _ = tx.try_send(SlaveMsg::Log(format!("→ 客户端 {addr} 已断开")));
}

/// 启动 RTU 从站（串口）。打开失败直接返回 Err。
pub fn start_rtu_slave(
    settings: SerialSettings,
    unit: u8,
    data: Arc<Mutex<SlaveData>>,
) -> Result<SlaveHandle, String> {
    let mut port = settings.open()?;
    let t35 = crate::rtu::char_time_3_5(settings.baud);
    let (tx, rx) = bounded_channel::<SlaveMsg>(64);
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    let port_name = settings.port.clone();
    let baud = settings.baud;
    let _ = tx.try_send(SlaveMsg::Log(format!(
        "✓ 从站已启动：RTU {}（{} 波特），站地址 {unit}",
        port_name, baud
    )));
    std::thread::spawn(move || {
        loop {
            if stop2.load(Ordering::Relaxed) {
                break;
            }
            match crate::rtu::read_rtu_frame(port.as_mut(), t35, Duration::from_millis(1000)) {
                Ok(frame) => {
                    let Ok((req_unit, pdu)) = crate::rtu::strip_rtu_frame(&frame) else {
                        continue;
                    };
                    if req_unit != unit {
                        continue;
                    }
                    let (resp_pdu, line) = {
                        let mut data = data.lock().unwrap_or_else(|e| e.into_inner());
                        handle_pdu(&mut data, &pdu)
                    };
                    let _ = tx.try_send(SlaveMsg::Log(line));
                    let resp = crate::rtu::build_rtu_frame(unit, &resp_pdu);
                    if port.write_all(&resp).and_then(|_| port.flush()).is_err() {
                        let _ = tx.try_send(SlaveMsg::Log("✗ RTU 响应写回失败".into()));
                        break;
                    }
                }
                Err(ref e) if e.starts_with("RTU 响应超时") => {
                    // 空闲，没主站问，继续等。
                }
                Err(e) => {
                    let _ = tx.try_send(SlaveMsg::Log(format!("✗ {e}")));
                }
            }
        }
        let _ = tx.try_send(SlaveMsg::Stopped);
    });
    Ok(SlaveHandle { stop, rx, port: 0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data_with_values() -> SlaveData {
        let mut d = SlaveData::new(16);
        d.holding[0] = 100;
        d.holding[1] = 200;
        d.coils[0] = true;
        d
    }

    #[test]
    fn read_holding_regs() {
        let mut d = data_with_values();
        let (resp, _) = handle_pdu(&mut d, &[3, 0, 0, 0, 2]);
        assert_eq!(resp, vec![3, 4, 0, 100, 0, 200]);
    }

    #[test]
    fn write_single_reg_then_read_back() {
        let mut d = data_with_values();
        let (resp, _) = handle_pdu(&mut d, &[6, 0, 1, 0x12, 0x34]);
        assert_eq!(resp, vec![6, 0, 1, 0x12, 0x34]);
        assert_eq!(d.holding[1], 0x1234);
        let (resp, _) = handle_pdu(&mut d, &[3, 0, 1, 0, 1]);
        assert_eq!(resp, vec![3, 2, 0x12, 0x34]);
    }

    #[test]
    fn read_and_write_coils() {
        let mut d = data_with_values();
        let (resp, _) = handle_pdu(&mut d, &[1, 0, 0, 0, 8]);
        assert_eq!(resp, vec![1, 1, 0b0000_0001]);
        let (resp, _) = handle_pdu(&mut d, &[5, 0, 1, 0xFF, 0x00]);
        assert_eq!(resp, vec![5, 0, 1, 0xFF, 0x00]);
        assert!(d.coils[1]);
        let (resp, _) = handle_pdu(&mut d, &[5, 0, 1, 0x12, 0x34]);
        assert_eq!(resp[0], 0x85); // 异常：写线圈值只能是 FF00/0000
        assert_eq!(resp[1], 3);
    }

    #[test]
    fn write_multi_regs_and_coils() {
        let mut d = data_with_values();
        let (resp, _) = handle_pdu(&mut d, &[16, 0, 2, 0, 2, 4, 0x00, 0x0A, 0x00, 0x14]);
        assert_eq!(resp, vec![16, 0, 2, 0, 2]);
        assert_eq!(d.holding[2], 10);
        assert_eq!(d.holding[3], 20);
        let (resp, _) = handle_pdu(&mut d, &[15, 0, 0, 0, 4, 1, 0b0000_1010]);
        assert_eq!(resp, vec![15, 0, 0, 0, 4]);
        assert!(!d.coils[0] && d.coils[1] && !d.coils[2] && d.coils[3]);
    }

    #[test]
    fn out_of_range_is_exception_02() {
        let mut d = data_with_values();
        let (resp, line) = handle_pdu(&mut d, &[3, 0, 15, 0, 2]);
        assert_eq!(resp, vec![0x83, 2]);
        assert!(line.contains("非法数据地址"));
    }

    #[test]
    fn unknown_fc_is_exception_01() {
        let mut d = data_with_values();
        let (resp, _) = handle_pdu(&mut d, &[7, 0, 0, 0, 1]);
        assert_eq!(resp, vec![0x87, 1]);
    }

    #[test]
    fn tcp_slave_roundtrip() {
        let data = Arc::new(Mutex::new(data_with_values()));
        let handle = start_tcp_slave(0, 1, data).unwrap();
        let port = handle.port;
        assert!(port > 0);
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        // 读保持寄存器 0..1
        let req = [0u8, 1, 0, 0, 0, 6, 1, 3, 0, 0, 0, 2];
        s.write_all(&req).unwrap();
        let mut head = [0u8; 7];
        s.read_exact(&mut head).unwrap();
        let len = u16::from_be_bytes([head[4], head[5]]) as usize;
        let mut pdu = vec![0u8; len - 1];
        s.read_exact(&mut pdu).unwrap();
        assert_eq!(pdu, vec![3, 4, 0, 100, 0, 200]);
        // 写单个寄存器再读回
        let req = [0u8, 2, 0, 0, 0, 6, 1, 6, 0, 1, 0x12, 0x34];
        s.write_all(&req).unwrap();
        s.read_exact(&mut head).unwrap();
        let len = u16::from_be_bytes([head[4], head[5]]) as usize;
        let mut pdu = vec![0u8; len - 1];
        s.read_exact(&mut pdu).unwrap();
        assert_eq!(pdu, vec![6, 0, 1, 0x12, 0x34]);
        handle.stop.store(true, Ordering::Relaxed);
    }
}
