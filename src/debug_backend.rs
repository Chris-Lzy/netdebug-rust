//! 调试页后台：TCP Client/Server、UDP、WebSocket 的收发会话。
//!
//! Modbus TCP/RTU 不走这里，界面直接用 modbus.rs 的轮询线程（和主站页一样）。
//! 每种协议一个后台线程，事件发回调度界面：
//! - `Log`：系统/错误行（已是完整文字，界面直接写日志）；
//! - `Data` / `Sent`：收到的 / 已发出的原始字节，界面按 HEX/文本格式化；
//! - `Connected`：连接状态变了（TCP 断线重连时也会反复发）；
//! - `Stopped`：会话彻底结束。
//!
//! TCP Client 和 WebSocket 断线自动重连：退避 1 秒 → 2 秒 → 5 秒，
//! 之后一直 5 秒，直到用户点断开。TCP Server 只绑 127.0.0.1（仅本机）；
//! UDP 监听 0.0.0.0（设备要能发进来），发包默认发给最近一次收到
//! 数据的发送方（原版回包发错目标的 bug 就修在这里），没收过数据
//! 时发给界面填的远程地址。

use crate::industrial::bounded_channel;
use crate::log::LogLevel;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 后台发回界面的事件。
#[derive(Debug)]
pub enum DebugEvent {
    /// 系统/错误行（完整文字）。
    Log(LogLevel, String),
    /// 收到原始字节（from：对方地址，没有时是 None）。
    Data {
        from: Option<String>,
        bytes: Vec<u8>,
    },
    /// 已发出原始字节（to：发给谁，没有时是 None）。
    Sent { to: Option<String>, bytes: Vec<u8> },
    /// 连接状态变化。
    Connected(bool),
    /// 会话彻底结束（界面收到后把 session 置空）。
    Stopped,
}

/// 界面发给后台的命令。
#[derive(Debug)]
pub enum DebugCmd {
    Send(Vec<u8>),
}

/// 运行中的会话：停它只要把 stop 置 true（界面还会收到 Stopped）。
pub struct DebugSession {
    pub cmd_tx: std::sync::mpsc::SyncSender<DebugCmd>,
    pub rx: std::sync::mpsc::Receiver<DebugEvent>,
    pub stop: Arc<AtomicBool>,
    /// 实际端口（TCP Server / UDP 传 0 时由系统分配；Client/WS 是 0）。
    /// 正式代码里没人读（后端自己记日志），测试和诊断用。
    #[allow(dead_code)]
    pub port: u16,
}

/// 分段睡，可被 stop 提前打断（重连等待时点断开不用等满）。
fn sleep_interruptible(ms: u64, stop: &Arc<AtomicBool>) {
    let mut left = ms;
    while left > 0 {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let step = left.min(50);
        std::thread::sleep(Duration::from_millis(step));
        left -= step;
    }
}

/// 下一次重连等待：1 秒 → 2 秒 → 5 秒，之后一直 5 秒。
fn next_backoff(backoff_ms: &mut u64) -> u64 {
    let wait = *backoff_ms;
    *backoff_ms = match *backoff_ms {
        1000 => 2000,
        2000 => 5000,
        _ => 5000,
    };
    wait
}

/// UDP 发包目标选择：优先最近一次收到数据的发送方，其次界面填的
/// 远程地址，都没有就发不出去（调用方记一条错误日志）。
fn choose_udp_target(
    last_sender: Option<std::net::SocketAddr>,
    default_target: Option<std::net::SocketAddr>,
) -> Option<std::net::SocketAddr> {
    last_sender.or(default_target)
}

fn resolve(host: &str, port: u16) -> Result<std::net::SocketAddr, String> {
    (host, port)
        .to_socket_addrs()
        .map_err(|e| format!("解析地址 {host}:{port} 失败：{e}"))?
        .next()
        .ok_or_else(|| format!("解析地址 {host}:{port} 失败：没有结果"))
}

fn new_session() -> (
    std::sync::mpsc::SyncSender<DebugCmd>,
    std::sync::mpsc::Receiver<DebugEvent>,
    std::sync::mpsc::SyncSender<DebugEvent>,
    std::sync::mpsc::Receiver<DebugCmd>,
    Arc<AtomicBool>,
) {
    let (cmd_tx, cmd_rx) = bounded_channel::<DebugCmd>(64);
    let (tx, rx) = bounded_channel::<DebugEvent>(64);
    let stop = Arc::new(AtomicBool::new(false));
    (cmd_tx, rx, tx, cmd_rx, stop)
}

/// TCP Client：5 秒连接超时，断线按 auto_reconnect 自动重连。
pub fn start_tcp_client(
    host: String,
    port: u16,
    auto_reconnect: bool,
) -> Result<DebugSession, String> {
    let addr = resolve(&host, port)?;
    let (cmd_tx, rx, tx, cmd_rx, stop) = new_session();
    let stop2 = stop.clone();
    std::thread::spawn(move || {
        let mut attempt: u32 = 0;
        let mut backoff_ms: u64 = 1000;
        loop {
            if stop2.load(Ordering::Relaxed) {
                break;
            }
            match TcpStream::connect_timeout(&addr, Duration::from_secs(5)) {
                Ok(mut stream) => {
                    attempt = 0;
                    backoff_ms = 1000;
                    let _ = tx.try_send(DebugEvent::Log(
                        LogLevel::Sys,
                        format!("✓ 已连接 {host}:{port}"),
                    ));
                    let _ = tx.try_send(DebugEvent::Connected(true));
                    let _ = stream.set_read_timeout(Some(Duration::from_millis(20)));
                    let mut buf = [0u8; 4096];
                    let mut alive = true;
                    while alive {
                        if stop2.load(Ordering::Relaxed) {
                            break;
                        }
                        while let Ok(DebugCmd::Send(bytes)) = cmd_rx.try_recv() {
                            match stream.write_all(&bytes) {
                                Ok(()) => {
                                    let _ = tx.try_send(DebugEvent::Sent {
                                        to: Some(format!("{host}:{port}")),
                                        bytes,
                                    });
                                }
                                Err(e) => {
                                    let _ = tx.try_send(DebugEvent::Log(
                                        LogLevel::Err,
                                        format!("✗ 发送失败：{e}"),
                                    ));
                                    alive = false;
                                    break;
                                }
                            }
                        }
                        if !alive {
                            break;
                        }
                        match stream.read(&mut buf) {
                            Ok(0) => alive = false,
                            Ok(n) => {
                                let _ = tx.try_send(DebugEvent::Data {
                                    from: Some(format!("{host}:{port}")),
                                    bytes: buf[..n].to_vec(),
                                });
                            }
                            Err(ref e)
                                if e.kind() == std::io::ErrorKind::TimedOut
                                    || e.kind() == std::io::ErrorKind::WouldBlock => {}
                            Err(_) => alive = false,
                        }
                    }
                    let _ = tx.try_send(DebugEvent::Connected(false));
                    if stop2.load(Ordering::Relaxed) {
                        break;
                    }
                    if !auto_reconnect {
                        break;
                    }
                    attempt += 1;
                    let wait = next_backoff(&mut backoff_ms);
                    let _ = tx.try_send(DebugEvent::Log(
                        LogLevel::Sys,
                        format!("✗ 已断开，{wait} ms 后第 {attempt} 次重连…"),
                    ));
                    sleep_interruptible(wait, &stop2);
                }
                Err(e) => {
                    if stop2.load(Ordering::Relaxed) {
                        break;
                    }
                    if !auto_reconnect {
                        let _ =
                            tx.try_send(DebugEvent::Log(LogLevel::Err, format!("✗ 连接失败：{e}")));
                        break;
                    }
                    attempt += 1;
                    let wait = next_backoff(&mut backoff_ms);
                    let _ = tx.try_send(DebugEvent::Log(
                        LogLevel::Err,
                        format!("✗ 连接失败：{e}，{wait} ms 后第 {attempt} 次重连…"),
                    ));
                    sleep_interruptible(wait, &stop2);
                }
            }
        }
        let _ = tx.try_send(DebugEvent::Stopped);
    });
    Ok(DebugSession {
        cmd_tx,
        rx,
        stop,
        port: 0,
    })
}

/// TCP Server：只绑 127.0.0.1（仅本机），多客户端，收到的数据带客户端地址记日志。
pub fn start_tcp_server(port: u16) -> Result<DebugSession, String> {
    let listener = TcpListener::bind(("127.0.0.1", port))
        .map_err(|e| format!("监听端口 {port} 失败：{e}（可能被占用）"))?;
    let actual_port = listener.local_addr().map(|a| a.port()).unwrap_or(port);
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("监听端口 {port} 失败：{e}"))?;
    let (cmd_tx, rx, tx, cmd_rx, stop) = new_session();
    let stop2 = stop.clone();
    let clients: Arc<Mutex<Vec<(String, TcpStream)>>> = Arc::new(Mutex::new(Vec::new()));
    let _ = tx.try_send(DebugEvent::Log(
        LogLevel::Sys,
        format!("✓ TCP Server 已启动：127.0.0.1:{actual_port}（仅本机）"),
    ));
    let _ = tx.try_send(DebugEvent::Connected(true));
    std::thread::spawn(move || {
        loop {
            if stop2.load(Ordering::Relaxed) {
                break;
            }
            while let Ok(DebugCmd::Send(bytes)) = cmd_rx.try_recv() {
                let mut dead = Vec::new();
                let mut sent = 0;
                {
                    let mut guard = clients.lock().unwrap_or_else(|e| e.into_inner());
                    for (i, (addr, stream)) in guard.iter_mut().enumerate() {
                        match stream.write_all(&bytes) {
                            Ok(()) => sent += 1,
                            Err(_) => dead.push(i),
                        }
                        let _ = addr;
                    }
                    for i in dead.into_iter().rev() {
                        guard.remove(i);
                    }
                }
                let _ = tx.try_send(DebugEvent::Sent {
                    to: Some(format!("{sent} 个客户端")),
                    bytes,
                });
            }
            match listener.accept() {
                Ok((stream, addr)) => {
                    let addr_str = addr.to_string();
                    let _ = tx.try_send(DebugEvent::Log(
                        LogLevel::Sys,
                        format!("→ 客户端 {addr_str} 已连接"),
                    ));
                    let _ = stream.set_read_timeout(Some(Duration::from_millis(20)));
                    let read_stream = match stream.try_clone() {
                        Ok(s) => s,
                        Err(_) => continue,
                    };
                    clients
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push((addr_str.clone(), stream));
                    let txc = tx.clone();
                    let stopc = stop2.clone();
                    let clientsc = clients.clone();
                    std::thread::spawn(move || {
                        let mut stream = read_stream;
                        let mut buf = [0u8; 4096];
                        loop {
                            if stopc.load(Ordering::Relaxed) {
                                break;
                            }
                            match stream.read(&mut buf) {
                                Ok(0) => break,
                                Ok(n) => {
                                    let _ = txc.try_send(DebugEvent::Data {
                                        from: Some(addr_str.clone()),
                                        bytes: buf[..n].to_vec(),
                                    });
                                }
                                Err(ref e)
                                    if e.kind() == std::io::ErrorKind::TimedOut
                                        || e.kind() == std::io::ErrorKind::WouldBlock => {}
                                Err(_) => break,
                            }
                        }
                        clientsc
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .retain(|(a, _)| *a != addr_str);
                        let _ = txc.try_send(DebugEvent::Log(
                            LogLevel::Sys,
                            format!("→ 客户端 {addr_str} 已断开"),
                        ));
                    });
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(e) => {
                    let _ = tx.try_send(DebugEvent::Log(LogLevel::Err, format!("✗ 监听出错：{e}")));
                    break;
                }
            }
        }
        let _ = tx.try_send(DebugEvent::Stopped);
    });
    Ok(DebugSession {
        cmd_tx,
        rx,
        stop,
        port: actual_port,
    })
}

/// UDP：监听 0.0.0.0:listen_port；发包默认发给最近一次收到数据的
/// 发送方，没收过数据时发给 default_target（界面填的远程地址）。
pub fn start_udp(
    listen_port: u16,
    default_target: Option<std::net::SocketAddr>,
) -> Result<DebugSession, String> {
    let socket = UdpSocket::bind(("0.0.0.0", listen_port))
        .map_err(|e| format!("监听端口 {listen_port} 失败：{e}（可能被占用）"))?;
    let actual_port = socket.local_addr().map(|a| a.port()).unwrap_or(listen_port);
    socket
        .set_read_timeout(Some(Duration::from_millis(20)))
        .map_err(|e| format!("监听端口 {listen_port} 失败：{e}"))?;
    let socket = Arc::new(socket);
    let last_sender: Arc<Mutex<Option<std::net::SocketAddr>>> = Arc::new(Mutex::new(None));
    let (cmd_tx, rx, tx, cmd_rx, stop) = new_session();
    let stop2 = stop.clone();
    let _ = tx.try_send(DebugEvent::Log(
        LogLevel::Sys,
        format!("✓ UDP 已启动：监听 0.0.0.0:{actual_port}"),
    ));
    let _ = tx.try_send(DebugEvent::Connected(true));
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            if stop2.load(Ordering::Relaxed) {
                break;
            }
            while let Ok(DebugCmd::Send(bytes)) = cmd_rx.try_recv() {
                let target = *last_sender.lock().unwrap_or_else(|e| e.into_inner());
                let target = choose_udp_target(target, default_target);
                match target {
                    Some(addr) => match socket.send_to(&bytes, addr) {
                        Ok(_) => {
                            let _ = tx.try_send(DebugEvent::Sent {
                                to: Some(addr.to_string()),
                                bytes,
                            });
                        }
                        Err(e) => {
                            let _ = tx.try_send(DebugEvent::Log(
                                LogLevel::Err,
                                format!("✗ 发送失败：{e}"),
                            ));
                        }
                    },
                    None => {
                        let _ = tx.try_send(DebugEvent::Log(
                            LogLevel::Err,
                            "✗ 还没有收到过数据，不知道发给谁（先在“远程地址”填目标再发）".into(),
                        ));
                    }
                }
            }
            match socket.recv_from(&mut buf) {
                Ok((n, from)) => {
                    *last_sender.lock().unwrap_or_else(|e| e.into_inner()) = Some(from);
                    let _ = tx.try_send(DebugEvent::Data {
                        from: Some(from.to_string()),
                        bytes: buf[..n].to_vec(),
                    });
                }
                Err(ref e)
                    if e.kind() == std::io::ErrorKind::TimedOut
                        || e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => {
                    let _ = tx.try_send(DebugEvent::Log(
                        LogLevel::Err,
                        format!("✗ UDP 接收出错：{e}"),
                    ));
                    break;
                }
            }
        }
        let _ = tx.try_send(DebugEvent::Stopped);
    });
    Ok(DebugSession {
        cmd_tx,
        rx,
        stop,
        port: actual_port,
    })
}

/// WebSocket：只支持 ws://（没开 TLS，wss:// 会握手失败）。
/// 5 秒连接超时，断线按 auto_reconnect 自动重连（和 TCP Client 一样）。
pub fn start_websocket(url: String, auto_reconnect: bool) -> Result<DebugSession, String> {
    let parsed = url::Url::parse(&url).map_err(|e| format!("WS 地址格式不对：{e}"))?;
    if parsed.scheme() != "ws" {
        return Err("只支持 ws://，不支持 wss://（没开 TLS）".into());
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| "WS 地址没有主机名".to_string())?
        .to_string();
    let port = parsed.port_or_known_default().unwrap_or(80);
    let addr = resolve(&host, port)?;
    let (cmd_tx, rx, tx, cmd_rx, stop) = new_session();
    let stop2 = stop.clone();
    std::thread::spawn(move || {
        let mut attempt: u32 = 0;
        let mut backoff_ms: u64 = 1000;
        loop {
            if stop2.load(Ordering::Relaxed) {
                break;
            }
            let dial = (|| -> Result<tungstenite::WebSocket<TcpStream>, String> {
                let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5))
                    .map_err(|e| format!("连接 {host}:{port} 失败：{e}"))?;
                let (mut socket, _) = tungstenite::client(url.as_str(), stream)
                    .map_err(|e| format!("WebSocket 握手失败：{e}"))?;
                socket
                    .get_mut()
                    .set_read_timeout(Some(Duration::from_millis(20)))
                    .map_err(|e| format!("设置读超时失败：{e}"))?;
                Ok(socket)
            })();
            let mut socket = match dial {
                Ok(s) => {
                    attempt = 0;
                    backoff_ms = 1000;
                    let _ = tx.try_send(DebugEvent::Log(
                        LogLevel::Sys,
                        format!("✓ WebSocket 已连接 {url}"),
                    ));
                    let _ = tx.try_send(DebugEvent::Connected(true));
                    s
                }
                Err(e) => {
                    if stop2.load(Ordering::Relaxed) {
                        break;
                    }
                    if !auto_reconnect {
                        let _ = tx.try_send(DebugEvent::Log(LogLevel::Err, e));
                        break;
                    }
                    attempt += 1;
                    let wait = next_backoff(&mut backoff_ms);
                    let _ = tx.try_send(DebugEvent::Log(
                        LogLevel::Err,
                        format!("✗ {e}，{wait} ms 后第 {attempt} 次重连…"),
                    ));
                    sleep_interruptible(wait, &stop2);
                    continue;
                }
            };
            let mut alive = true;
            while alive {
                if stop2.load(Ordering::Relaxed) {
                    break;
                }
                while let Ok(DebugCmd::Send(bytes)) = cmd_rx.try_recv() {
                    match socket.write(tungstenite::Message::Binary(bytes.clone().into())) {
                        Ok(()) => {
                            // tungstenite 的 write 只进写缓冲，必须 flush 才真正发出去。
                            let _ = socket.flush();
                            let _ = tx.try_send(DebugEvent::Sent {
                                to: Some(url.clone()),
                                bytes,
                            });
                        }
                        Err(e) => {
                            let _ = tx.try_send(DebugEvent::Log(
                                LogLevel::Err,
                                format!("✗ 发送失败：{e}"),
                            ));
                            alive = false;
                            break;
                        }
                    }
                }
                if !alive {
                    break;
                }
                match socket.read() {
                    Ok(tungstenite::Message::Text(text)) => {
                        let _ = tx.try_send(DebugEvent::Data {
                            from: Some(url.clone()),
                            bytes: text.as_bytes().to_vec(),
                        });
                    }
                    Ok(tungstenite::Message::Binary(bytes)) => {
                        let _ = tx.try_send(DebugEvent::Data {
                            from: Some(url.clone()),
                            bytes: bytes.to_vec(),
                        });
                    }
                    Ok(tungstenite::Message::Ping(data)) => {
                        let _ = socket.write(tungstenite::Message::Pong(data));
                        let _ = socket.flush();
                    }
                    Ok(tungstenite::Message::Pong(_)) | Ok(tungstenite::Message::Frame(_)) => {}
                    Ok(tungstenite::Message::Close(_)) => alive = false,
                    Err(tungstenite::Error::Io(ref e))
                        if e.kind() == std::io::ErrorKind::TimedOut
                            || e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(_) => alive = false,
                }
            }
            let _ = tx.try_send(DebugEvent::Connected(false));
            if stop2.load(Ordering::Relaxed) {
                break;
            }
            if !auto_reconnect {
                break;
            }
            attempt += 1;
            let wait = next_backoff(&mut backoff_ms);
            let _ = tx.try_send(DebugEvent::Log(
                LogLevel::Sys,
                format!("✗ WebSocket 已断开，{wait} ms 后第 {attempt} 次重连…"),
            ));
            sleep_interruptible(wait, &stop2);
        }
        let _ = tx.try_send(DebugEvent::Stopped);
    });
    Ok(DebugSession {
        cmd_tx,
        rx,
        stop,
        port: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::Receiver;
    use std::time::Instant;

    /// 等事件，满足条件返回 true，超时返回 false（消费掉等到的事件）。
    fn wait_for(
        rx: &Receiver<DebugEvent>,
        mut pred: impl FnMut(&DebugEvent) -> bool,
        timeout_ms: u64,
    ) -> bool {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        while Instant::now() < deadline {
            if let Ok(ev) = rx.try_recv() {
                if pred(&ev) {
                    return true;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn tcp_server_client_roundtrip() {
        let server = start_tcp_server(0).unwrap();
        let port = server.port;
        assert!(port > 0);
        let client = start_tcp_client("127.0.0.1".into(), port, false).unwrap();
        assert!(wait_for(
            &client.rx,
            |ev| matches!(ev, DebugEvent::Connected(true)),
            3000
        ));
        client
            .cmd_tx
            .send(DebugCmd::Send(b"hello".to_vec()))
            .unwrap();
        assert!(wait_for(
            &server.rx,
            |ev| matches!(ev, DebugEvent::Data { bytes, .. } if bytes == b"hello"),
            3000
        ));
        server
            .cmd_tx
            .send(DebugCmd::Send(b"world".to_vec()))
            .unwrap();
        assert!(wait_for(
            &client.rx,
            |ev| matches!(ev, DebugEvent::Data { bytes, .. } if bytes == b"world"),
            3000
        ));
        client.stop.store(true, Ordering::Relaxed);
        server.stop.store(true, Ordering::Relaxed);
    }

    #[test]
    fn udp_target_prefers_last_sender() {
        let last: std::net::SocketAddr = "192.168.1.5:1234".parse().unwrap();
        let def: std::net::SocketAddr = "127.0.0.1:502".parse().unwrap();
        // 收过数据就回给最后那个发送方（原版回包发错目标的 bug 修在这里）。
        assert_eq!(choose_udp_target(Some(last), Some(def)), Some(last));
        // 没收过数据就发给界面填的远程地址。
        assert_eq!(choose_udp_target(None, Some(def)), Some(def));
        // 两个都没有就发不出去。
        assert_eq!(choose_udp_target(None, None), None);
    }

    #[test]
    fn udp_session_binds() {
        // 注意：本机沙箱禁了 UDP 收包，完整的收发回环只能在 Windows 真机上验；
        // 这里只验监听能起来、端口分配正常。
        let session = start_udp(0, None).unwrap();
        assert!(session.port > 0);
        session.stop.store(true, Ordering::Relaxed);
    }

    #[test]
    fn websocket_roundtrip() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut ws = tungstenite::accept(stream).unwrap();
            let msg = ws.read().unwrap();
            assert_eq!(msg.into_data().as_ref(), b"hello");
            ws.write(tungstenite::Message::Text("world".into()))
                .unwrap();
            // write 只进写缓冲，必须 flush 才真正发出去。
            ws.flush().unwrap();
        });
        let session = start_websocket(format!("ws://127.0.0.1:{port}/"), false).unwrap();
        assert!(wait_for(
            &session.rx,
            |ev| matches!(ev, DebugEvent::Connected(true)),
            3000
        ));
        session
            .cmd_tx
            .send(DebugCmd::Send(b"hello".to_vec()))
            .unwrap();
        assert!(wait_for(
            &session.rx,
            |ev| matches!(ev, DebugEvent::Data { bytes, .. } if bytes == b"world"),
            3000
        ));
        session.stop.store(true, Ordering::Relaxed);
        server.join().unwrap();
    }
}
