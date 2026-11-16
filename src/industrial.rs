//! 工业级辅助：确定性定时器、看门狗、有界通道。
//!
//! 这些都是“尽量靠近 RTOS 要求”的具体落点，不是装饰：
//! - 定时器：到期后按固定周期推进，落后超过一个周期时直接对齐，不追赶积压，
//!   抖动有界；
//! - 看门狗：连接/串口必须在期限内喂狗，否则视为超时，由调用方决定重连；
//! - 通道：全部有界，内存和积压都有上限。

use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::time::{Duration, Instant};

/// 有界通道（sync_channel 的薄包装）。
/// 容量必须显式给定；满了之后 `send` 会阻塞调用方，所以数据通路的发送方
/// 应该用 `try_send`，或者把通道容量留够。禁止用无界通道。
pub fn bounded_channel<T>(capacity: usize) -> (SyncSender<T>, Receiver<T>) {
    sync_channel(capacity.max(1))
}

/// 确定性间隔定时器。
///
/// - 第一次调用 `due()` 立即返回 true（启动时不用等一个周期）；
/// - 每次到期后按固定周期推进 `next`，不按“现在”重算，长期漂移小；
/// - 如果落后超过一个周期（比如界面卡了一会儿），直接对齐到 now + period，
///   不把积压的到期次数一次补回来——轮询发一万条没有意义。
/// 框架阶段预留：轮询/心跳等业务逻辑接上后使用。
#[allow(dead_code)]
pub struct IntervalTimer {
    period: Duration,
    next: Instant,
}

#[allow(dead_code)]
impl IntervalTimer {
    pub fn new(period: Duration) -> Self {
        Self {
            period,
            next: Instant::now(),
        }
    }

    /// 到期返回 true，并推进到下一个周期；没到期返回 false。
    pub fn due(&mut self) -> bool {
        let now = Instant::now();
        if now >= self.next {
            self.next = if now > self.next + self.period {
                now + self.period
            } else {
                self.next + self.period
            };
            true
        } else {
            false
        }
    }

    pub fn set_period(&mut self, period: Duration) {
        self.period = period;
    }

    pub fn period(&self) -> Duration {
        self.period
    }
}

/// 看门狗：必须在期限内 `feed()`，否则 `expired()` 为 true。
///
/// 用途：TCP/串口连接的心跳、Modbus 响应超时、桥接存活检测。
/// 框架阶段预留：连接看门狗，业务逻辑接上后使用。
#[allow(dead_code)]
pub struct Watchdog {
    deadline: Duration,
    last_feed: Instant,
}

#[allow(dead_code)]
impl Watchdog {
    pub fn new(deadline: Duration) -> Self {
        Self {
            deadline,
            last_feed: Instant::now(),
        }
    }

    /// 喂狗。
    pub fn feed(&mut self) {
        self.last_feed = Instant::now();
    }

    /// 是否已超时。
    pub fn expired(&self) -> bool {
        self.last_feed.elapsed() >= self.deadline
    }

    /// 剩余时间（已超时返回 0）。
    pub fn remaining(&self) -> Duration {
        self.deadline.saturating_sub(self.last_feed.elapsed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_timer_fires_immediately_then_waits() {
        let mut t = IntervalTimer::new(Duration::from_secs(60));
        assert!(t.due(), "第一次调用应立即到期");
        assert!(!t.due(), "刚到期过，60 秒周期内不应再到期");
    }

    #[test]
    fn watchdog_expires_after_deadline() {
        let w = Watchdog::new(Duration::from_millis(0));
        assert!(w.expired(), "0 毫秒期限应立即超时");
        assert_eq!(w.remaining(), Duration::ZERO);
    }

    #[test]
    fn watchdog_feed_resets_deadline() {
        let mut w = Watchdog::new(Duration::from_secs(60));
        w.feed();
        assert!(!w.expired());
    }

    #[test]
    fn bounded_channel_respects_capacity() {
        let (tx, rx) = bounded_channel::<i32>(2);
        assert!(tx.try_send(1).is_ok());
        assert!(tx.try_send(2).is_ok());
        assert!(tx.try_send(3).is_err(), "满了就应拒绝，不阻塞");
        assert_eq!(rx.try_recv().unwrap(), 1);
    }
}
