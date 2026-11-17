//! Modbus RTU：CRC16、RTU 帧、串口（主站 RTU / 从站 RTU / 调试页 RTU 共用）。
//!
//! 帧格式：`[站地址 1B][PDU][CRC 低字节][CRC 高字节]`，CRC 是 Modbus CRC16
//!（多项式 0xA001，初值 0xFFFF，低字节在前）。
//! 帧与帧之间至少隔 3.5 个字符时间：波特率 > 19200 时按 3.5×11 位算，
//! ≤ 19200 时按规范取固定 1.75 ms。
//! 读帧用“静默间隔”判定帧尾：收到字节后超过 3.5 字符没新字节，就认为
//! 一帧收完了，再验 CRC。

use std::time::{Duration, Instant};

/// Modbus CRC16（多项式 0xA001，初值 0xFFFF）。
pub fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &b in data {
        crc ^= b as u16;
        for _ in 0..8 {
            let lsb = crc & 1;
            crc >>= 1;
            if lsb != 0 {
                crc ^= 0xA001;
            }
        }
    }
    crc
}

/// 打包 RTU 请求/响应帧：站地址 + PDU + CRC（低字节在前）。
pub fn build_rtu_frame(unit: u8, pdu: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(pdu.len() + 3);
    frame.push(unit);
    frame.extend_from_slice(pdu);
    let crc = crc16(&frame);
    frame.push((crc & 0xFF) as u8);
    frame.push((crc >> 8) as u8);
    frame
}

/// 拆 RTU 帧：验长度、验 CRC，返回（站地址，PDU）。
pub fn strip_rtu_frame(frame: &[u8]) -> Result<(u8, Vec<u8>), String> {
    if frame.len() < 4 {
        return Err(format!("RTU 帧太短（{} 字节）", frame.len()));
    }
    let (body, crc_bytes) = frame.split_at(frame.len() - 2);
    let want = crc16(body);
    let got = (crc_bytes[0] as u16) | ((crc_bytes[1] as u16) << 8);
    if want != got {
        return Err(format!(
            "RTU CRC 校验失败（算出 {want:04X}，收到 {got:04X}）"
        ));
    }
    Ok((body[0], body[1..].to_vec()))
}

/// 3.5 个字符的静默时间（见模块头注释）。
pub fn char_time_3_5(baud: u32) -> Duration {
    if baud > 19200 {
        // 3.5 字符 × 11 位/字符 ÷ 波特率
        let us = 3.5 * 11.0 * 1_000_000.0 / baud.max(1) as f64;
        Duration::from_micros(us.ceil() as u64)
    } else {
        Duration::from_micros(1750)
    }
}

/// 串口设置（界面上的串口 / 波特率 / 数据位 / 校验 / 停止位）。
#[derive(Debug, Clone)]
pub struct SerialSettings {
    pub port: String,
    pub baud: u32,
    pub data_bits: u8,
    pub parity: String,
    pub stop_bits: String,
}

impl SerialSettings {
    /// 主站 RTU 的常用默认值：8 数据位、无校验、1 停止位。
    pub fn master_default(port: &str, baud: u32) -> Self {
        Self {
            port: port.to_string(),
            baud,
            data_bits: 8,
            parity: "None".to_string(),
            stop_bits: "1".to_string(),
        }
    }

    /// 打开串口。超时设 50 ms：读帧循环靠短超时做静默检测，
    /// 不要用大超时，否则每帧都要等满超时才算收完。
    pub fn open(&self) -> Result<Box<dyn serialport::SerialPort>, String> {
        let data_bits = match self.data_bits {
            7 => serialport::DataBits::Seven,
            _ => serialport::DataBits::Eight,
        };
        let parity = match self.parity.as_str() {
            "Even" => serialport::Parity::Even,
            "Odd" => serialport::Parity::Odd,
            _ => serialport::Parity::None,
        };
        let stop_bits = match self.stop_bits.as_str() {
            "2" => serialport::StopBits::Two,
            _ => serialport::StopBits::One,
        };
        serialport::new(&self.port, self.baud)
            .data_bits(data_bits)
            .parity(parity)
            .stop_bits(stop_bits)
            .flow_control(serialport::FlowControl::None)
            .timeout(Duration::from_millis(50))
            .open()
            .map_err(|e| format!("打开串口 {} 失败：{e}", self.port))
    }
}

/// 列出系统串口名（COM1… / ttyUSB…），失败返回空列表。
/// 下阶段从站/调试页的串口下拉框用，先留着。
#[allow(dead_code)]
pub fn list_ports() -> Vec<String> {
    serialport::available_ports()
        .map(|ports| ports.into_iter().map(|p| p.port_name).collect())
        .unwrap_or_default()
}

/// 写一帧 RTU（打包 + 写 + 刷出）。
pub fn write_rtu_frame(
    port: &mut dyn serialport::SerialPort,
    unit: u8,
    pdu: &[u8],
) -> Result<(), String> {
    let frame = build_rtu_frame(unit, pdu);
    port.write_all(&frame)
        .and_then(|_| port.flush())
        .map_err(|e| format!("串口写入失败：{e}"))
}

/// 读一帧 RTU：收到字节后静默满 3.5 字符即帧尾；总超时 `timeout`。
/// 返回原始帧（含站地址和 CRC），调用方再用 `strip_rtu_frame` 拆。
pub fn read_rtu_frame(
    port: &mut dyn serialport::SerialPort,
    t35: Duration,
    timeout: Duration,
) -> Result<Vec<u8>, String> {
    let deadline = Instant::now() + timeout;
    let mut buf: Vec<u8> = Vec::new();
    let mut last_byte = Instant::now();
    let mut chunk = [0u8; 256];
    loop {
        match port.read(&mut chunk) {
            Ok(0) => {}
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                last_byte = Instant::now();
            }
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => return Err(format!("串口读取失败：{e}")),
        }
        if !buf.is_empty() && last_byte.elapsed() >= t35 {
            return Ok(buf);
        }
        if Instant::now() >= deadline {
            if buf.is_empty() {
                return Err("RTU 响应超时（1 秒内没收到数据）".into());
            }
            return Ok(buf);
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc16_textbook_vector() {
        // 教科书例子：01 03 00 00 00 0A 的 CRC 是 CD C5（低字节在前）。
        assert_eq!(crc16(&[0x01, 0x03, 0x00, 0x00, 0x00, 0x0A]), 0xCDC5);
    }

    #[test]
    fn build_and_strip_roundtrip() {
        let frame = build_rtu_frame(1, &[0x03, 0x00, 0x00, 0x00, 0x0A]);
        assert_eq!(frame, vec![0x01, 0x03, 0x00, 0x00, 0x00, 0x0A, 0xC5, 0xCD]);
        let (unit, pdu) = strip_rtu_frame(&frame).unwrap();
        assert_eq!(unit, 1);
        assert_eq!(pdu, vec![0x03, 0x00, 0x00, 0x00, 0x0A]);
    }

    #[test]
    fn strip_rejects_bad_crc() {
        let mut frame = build_rtu_frame(1, &[0x03, 0x00, 0x00, 0x00, 0x0A]);
        frame[3] ^= 0xFF;
        assert!(strip_rtu_frame(&frame).is_err());
    }

    #[test]
    fn strip_rejects_short_frame() {
        assert!(strip_rtu_frame(&[0x01, 0x03]).is_err());
    }

    #[test]
    fn char_time_follows_spec() {
        // ≤19200 波特率取固定 1.75 ms
        assert_eq!(char_time_3_5(9600), Duration::from_micros(1750));
        // 115200：3.5×11/115200 ≈ 334 µs
        let t = char_time_3_5(115200);
        assert!(t.as_micros() >= 330 && t.as_micros() <= 340);
    }

    #[test]
    fn open_missing_port_fails() {
        let s = SerialSettings::master_default("COM_NOT_EXIST_999", 9600);
        assert!(s.open().is_err());
    }
}
