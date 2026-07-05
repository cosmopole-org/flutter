//! Dart's event loop: the native scheduling substrate under `dart:async`.
//!
//! `Future`, `Stream`, `Completer`, and `async`/`await` are **Dart source** in
//! the real SDK — they are built on top of just two native hooks:
//! `scheduleMicrotask` (`_scheduleImmediate`) and `Timer` (`Timer._createTimer`).
//! So the runtime layer only has to provide those two primitives and, crucially,
//! reproduce Dart's **exact ordering rules**:
//!
//! * the microtask queue is drained **completely** before any timer/event runs;
//! * a callback that schedules more microtasks has them run before returning to
//!   timers;
//! * timers fire in non-decreasing due-time order, FIFO among equal due times.
//!
//! Getting this ordering right is what makes framework and app async code behave
//! identically to stock Dart. This module is the pure scheduler; the runtime
//! drives the actual guest-callback invocations it hands back.

use std::collections::VecDeque;

/// An opaque guest callback id. The guest owns the closure table; the loop only
/// tracks *which* callback is due, never the closure itself.
pub type CallbackId = u64;

#[derive(Debug, Clone, Copy)]
struct Timer {
    id: u64,
    cb: CallbackId,
    due: u64,
    seq: u64,
    cancelled: bool,
    /// `Some(interval)` for a repeating `Timer.periodic`; `None` for one-shot.
    period: Option<u64>,
}

/// A callback the runtime should now invoke on the guest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DueTask {
    pub cb: CallbackId,
    /// True for a timer firing (vs a microtask) — diagnostic only.
    pub is_timer: bool,
}

/// The event loop: a microtask FIFO and a set of timers over a virtual clock.
#[derive(Debug, Default)]
pub struct EventLoop {
    microtasks: VecDeque<CallbackId>,
    timers: Vec<Timer>,
    now: u64,
    next_timer_id: u64,
    seq: u64,
}

impl EventLoop {
    pub fn new() -> Self {
        EventLoop::default()
    }

    /// `scheduleMicrotask(cb)` — enqueue a microtask (runs before any timer).
    pub fn schedule_microtask(&mut self, cb: CallbackId) {
        self.microtasks.push_back(cb);
    }

    /// `Timer(delayMs, cb)` — schedule a one-shot timer; returns its id.
    pub fn schedule_timer(&mut self, cb: CallbackId, delay_ms: u64) -> u64 {
        let id = self.next_timer_id;
        self.next_timer_id += 1;
        let seq = self.seq;
        self.seq += 1;
        self.timers.push(Timer {
            id,
            cb,
            due: self.now.saturating_add(delay_ms),
            seq,
            cancelled: false,
            period: None,
        });
        id
    }

    /// `Timer.periodic(interval, cb)` — a repeating timer. Fires every
    /// `interval` ms until cancelled; returns its id.
    pub fn schedule_periodic(&mut self, cb: CallbackId, interval_ms: u64) -> u64 {
        let id = self.next_timer_id;
        self.next_timer_id += 1;
        let seq = self.seq;
        self.seq += 1;
        self.timers.push(Timer {
            id,
            cb,
            due: self.now.saturating_add(interval_ms),
            seq,
            cancelled: false,
            period: Some(interval_ms.max(1)),
        });
        id
    }

    /// `Timer.cancel()` — a cancelled timer never fires.
    pub fn cancel_timer(&mut self, id: u64) -> bool {
        if let Some(t) = self.timers.iter_mut().find(|t| t.id == id) {
            let was = !t.cancelled;
            t.cancelled = true;
            was
        } else {
            false
        }
    }

    pub fn has_pending(&self) -> bool {
        !self.microtasks.is_empty() || self.timers.iter().any(|t| !t.cancelled)
    }

    /// Current virtual time (ms). Advances as timers fire.
    pub fn now(&self) -> u64 {
        self.now
    }

    /// Return the next task to run, honoring Dart ordering: any pending
    /// microtask first; otherwise advance the virtual clock to the earliest live
    /// timer (FIFO among equal due times) and fire it. `None` when idle.
    pub fn next_task(&mut self) -> Option<DueTask> {
        if let Some(cb) = self.microtasks.pop_front() {
            return Some(DueTask { cb, is_timer: false });
        }
        // Pick the earliest live timer by (due, seq).
        let mut best: Option<usize> = None;
        for (i, t) in self.timers.iter().enumerate() {
            if t.cancelled {
                continue;
            }
            match best {
                Some(b) => {
                    let bt = &self.timers[b];
                    if (t.due, t.seq) < (bt.due, bt.seq) {
                        best = Some(i);
                    }
                }
                None => best = Some(i),
            }
        }
        let idx = best?;
        let t = self.timers.remove(idx);
        self.now = self.now.max(t.due);
        // A periodic timer reschedules itself for the next interval.
        if let Some(interval) = t.period {
            let seq = self.seq;
            self.seq += 1;
            self.timers.push(Timer {
                id: t.id,
                cb: t.cb,
                due: t.due.saturating_add(interval),
                seq,
                cancelled: false,
                period: Some(interval),
            });
        }
        Some(DueTask { cb: t.cb, is_timer: true })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drain(loop_: &mut EventLoop) -> Vec<DueTask> {
        let mut out = Vec::new();
        while let Some(t) = loop_.next_task() {
            out.push(t);
        }
        out
    }

    #[test]
    fn microtasks_run_before_timers() {
        let mut l = EventLoop::new();
        l.schedule_timer(100, 0); // cb 100, delay 0
        l.schedule_microtask(1);
        l.schedule_microtask(2);
        let order: Vec<CallbackId> = drain(&mut l).iter().map(|t| t.cb).collect();
        // Both microtasks (FIFO) before the zero-delay timer.
        assert_eq!(order, vec![1, 2, 100]);
    }

    #[test]
    fn timers_fire_in_due_order_then_fifo() {
        let mut l = EventLoop::new();
        l.schedule_timer(30, 30);
        l.schedule_timer(10, 10);
        let a = l.schedule_timer(200, 20);
        let _ = a;
        l.schedule_timer(201, 20); // same due as previous -> FIFO by insertion
        let order: Vec<CallbackId> = drain(&mut l).iter().map(|t| t.cb).collect();
        assert_eq!(order, vec![10, 200, 201, 30]);
    }

    #[test]
    fn cancelled_timer_does_not_fire() {
        let mut l = EventLoop::new();
        let id = l.schedule_timer(7, 5);
        l.schedule_timer(8, 5);
        assert!(l.cancel_timer(id));
        let order: Vec<CallbackId> = drain(&mut l).iter().map(|t| t.cb).collect();
        assert_eq!(order, vec![8]);
    }

    #[test]
    fn periodic_timer_refires_until_cancelled() {
        let mut l = EventLoop::new();
        let id = l.schedule_periodic(9, 10);
        let mut fires = 0;
        // Drain a bounded number of ticks, cancelling after 3.
        while let Some(t) = l.next_task() {
            assert_eq!(t.cb, 9);
            fires += 1;
            if fires == 3 {
                l.cancel_timer(id);
            }
            if fires > 10 {
                break; // safety
            }
        }
        assert_eq!(fires, 3, "periodic should fire exactly 3 times then stop");
    }

    #[test]
    fn virtual_clock_advances_to_fired_timer() {
        let mut l = EventLoop::new();
        l.schedule_timer(1, 500);
        let _ = drain(&mut l);
        assert_eq!(l.now(), 500);
    }
}
