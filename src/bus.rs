//! 页面与后台任务之间的事件总线。
//!
//! 规则：
//! - 用 std mpsc，不引入额外依赖；
//! - 通道有界（见 industrial::bounded_channel），发送方永不被无限期阻塞——
//!   队列满时 `send` 直接丢弃并返回 false，宁可丢一条提示也不卡住数据通路；
//! - 后台线程只发事件，UI 线程每帧 drain；需要立即重绘时调用方拿到的
//!   `egui::Context` 自己调 `request_repaint()`。

use crate::industrial::bounded_channel;
use crate::log::LogLevel;
use std::sync::mpsc::{Receiver, SyncSender};

/// 总线事件。以后加新事件类型只在这里加变体，发送方和接收方都不用改结构。
/// 框架阶段预留：业务逻辑接上后由后台任务发送，暂时没有发送方。
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum Event {
    /// 界面右下角提示
    Toast(String),
    /// 追加一条日志
    Log(LogLevel, String),
}

/// 框架阶段预留：业务逻辑接上后由后台任务发送，暂时没有发送方。
#[allow(dead_code)]
pub struct Bus {
    tx: SyncSender<Event>,
    rx: Receiver<Event>,
}

#[allow(dead_code)]
impl Bus {
    /// 默认容量 256 条事件，够用且有界。
    pub fn new() -> Self {
        Self::with_capacity(256)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        let (tx, rx) = bounded_channel(capacity);
        Self { tx, rx }
    }

    /// 发事件。队列满返回 false（事件被丢弃），不阻塞、不 panic。
    pub fn send(&self, event: Event) -> bool {
        self.tx.try_send(event).is_ok()
    }

    /// 发一条 toast。
    pub fn toast(&self, text: impl Into<String>) -> bool {
        self.send(Event::Toast(text.into()))
    }

    /// 发一条日志。
    pub fn log(&self, level: LogLevel, text: impl Into<String>) -> bool {
        self.send(Event::Log(level, text.into()))
    }

    /// 取出当前积压的全部事件（非阻塞）。
    pub fn drain(&self) -> Vec<Event> {
        self.rx.try_iter().collect()
    }

    /// 给后台线程用的发送端克隆。
    pub fn sender(&self) -> SyncSender<Event> {
        self.tx.clone()
    }
}

impl Default for Bus {
    fn default() -> Self {
        Self::new()
    }
}
