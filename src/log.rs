//! 有界日志存储（环形缓冲，永不无限增长）。
//!
//! 工业要求：日志是调试工具里最容易失控的部分——跑一晚上不能把内存吃光。
//! 所以这里用固定容量的环形缓冲：满了就丢最旧的，并计数，绝不无限增长。

use std::collections::VecDeque;
use std::time::{SystemTime, UNIX_EPOCH};

/// 日志级别（颜色沿用 NetDebug 原版）。
/// 框架公共 API：Tx/Rx 由收发逻辑写入，业务逻辑接上后使用。
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    /// 发送（蓝）
    Tx,
    /// 接收（绿）
    Rx,
    /// 系统（灰）
    Sys,
    /// 错误（红）
    Err,
}

impl LogLevel {
    pub fn label(self) -> &'static str {
        match self {
            LogLevel::Tx => "TX",
            LogLevel::Rx => "RX",
            LogLevel::Sys => "SYS",
            LogLevel::Err => "ERR",
        }
    }

    pub fn color(self) -> egui::Color32 {
        match self {
            LogLevel::Tx => egui::Color32::from_rgb(0x60, 0xa5, 0xfa),
            LogLevel::Rx => egui::Color32::from_rgb(0x34, 0xd3, 0x99),
            LogLevel::Sys => egui::Color32::from_rgb(0x9c, 0xa3, 0xaf),
            LogLevel::Err => egui::Color32::from_rgb(0xf8, 0x71, 0x71),
        }
    }
}

/// 一条日志。
#[derive(Debug, Clone)]
pub struct LogEntry {
    /// 自 Unix 纪元起的毫秒数
    pub millis: u64,
    pub level: LogLevel,
    pub text: String,
}

impl LogEntry {
    pub fn new(level: LogLevel, text: impl Into<String>) -> Self {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        Self {
            millis,
            level,
            text: text.into(),
        }
    }

    /// HH:MM:SS.mmm（本地时区不做换算，调试日志只求单调可读）
    pub fn time_string(&self) -> String {
        format_millis(self.millis)
    }
}

/// 毫秒时间戳 -> "HH:MM:SS.mmm"（UTC，不依赖 chrono）。
pub fn format_millis(millis: u64) -> String {
    let secs = millis / 1000;
    let ms = millis % 1000;
    let h = (secs / 3600) % 24;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    format!("{h:02}:{m:02}:{s:02}.{ms:03}")
}

/// 有界环形日志。
/// 框架公共 API：`len`/`dropped_count`/`capacity`/`set_capacity` 由界面
/// 和业务逻辑使用，框架阶段暂时没人调。
#[allow(dead_code)]
pub struct LogStore {
    capacity: usize,
    entries: VecDeque<LogEntry>,
    /// 因为满而丢弃的条数（界面可显示，丢了多少心里有数）
    dropped: u64,
}

#[allow(dead_code)]
impl LogStore {
    /// 默认容量 3000（沿用 NetDebug 原版上限）。
    pub fn with_default_capacity() -> Self {
        Self::new(3000)
    }

    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            entries: VecDeque::new(),
            dropped: 0,
        }
    }

    pub fn push(&mut self, level: LogLevel, text: impl Into<String>) {
        if self.entries.len() >= self.capacity {
            self.entries.pop_front();
            self.dropped += 1;
        }
        self.entries.push_back(LogEntry::new(level, text));
    }

    pub fn entries(&self) -> &VecDeque<LogEntry> {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn dropped_count(&self) -> u64 {
        self.dropped
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// 改容量：缩小时从最旧的开始丢。
    pub fn set_capacity(&mut self, capacity: usize) {
        self.capacity = capacity.max(1);
        while self.entries.len() > self.capacity {
            self.entries.pop_front();
            self.dropped += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_is_enforced_and_oldest_dropped_first() {
        let mut log = LogStore::new(3);
        log.push(LogLevel::Tx, "a");
        log.push(LogLevel::Tx, "b");
        log.push(LogLevel::Tx, "c");
        log.push(LogLevel::Tx, "d");
        assert_eq!(log.len(), 3);
        assert_eq!(log.dropped_count(), 1);
        let texts: Vec<&str> = log.entries().iter().map(|e| e.text.as_str()).collect();
        assert_eq!(texts, vec!["b", "c", "d"]);
    }

    #[test]
    fn shrink_capacity_drops_oldest() {
        let mut log = LogStore::new(10);
        for i in 0..5 {
            log.push(LogLevel::Rx, format!("{i}"));
        }
        log.set_capacity(2);
        assert_eq!(log.len(), 2);
        assert_eq!(log.entries()[0].text, "3");
        assert_eq!(log.dropped_count(), 3);
    }

    #[test]
    fn time_format_is_stable() {
        assert_eq!(format_millis(0), "00:00:00.000");
        assert_eq!(format_millis(3_661_001), "01:01:01.001");
    }
}
