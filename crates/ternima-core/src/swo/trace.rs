//! Which task runs and the interrupts, traced over SWO: every switch and
//! every exception, timed by the chip itself.
//!
//! A DWT comparator watches the kernel's pointer to the running task and
//! sends each value written to it; the DWT's exception trace sends each
//! exception entered, left and returned to; the ITM's local timestamps,
//! counted in core clock cycles, tell when. Nothing in the firmware changes.
//! It needs a Cortex-M3, M4, M7 or M33 and its SWO pin, as an SWO tab has.

use std::collections::HashSet;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use probe_rs::{Core, CoreType, MemoryInterface};
use serde::Serialize;

use super::itm::{ExceptionEvent, ItmPacket};
use crate::rtos::{HALTED, Running};

const DWT_CTRL: u64 = 0xE000_1000;
/// Exception trace, in DWT_CTRL.
const EXCTRCENA: u32 = 1 << 16;
const DWT_COMP0: u64 = 0xE000_1020;
const DWT_MASK0: u64 = 0xE000_1024;
const DWT_FUNCTION0: u64 = 0xE000_1028;
/// ARMv8-M: MATCH 0b1101, a data address with its value, on writes; ACTION
/// 0b10, a Data Trace Data Value packet; DATAVSIZE 0b10, a word.
const FUNCTION_V8M: u32 = (0b10 << 10) | (0b10 << 4) | 0b1101;
/// ARMv7-M: FUNCTION 0b1101, the data of write transfers; DATAVSIZE 0b10, a word.
const FUNCTION_V7M: u32 = (0b10 << 10) | 0b1101;

/// Has DWT comparator 0 send each value written to `address`, a word, or
/// stops it with `None`. Says whether this core can: not a Cortex-M0.
pub fn trace_writes(core: &mut Core, address: Option<u64>) -> Result<bool, probe_rs::Error> {
    let function = match core.core_type() {
        CoreType::Armv8m => FUNCTION_V8M,
        CoreType::Armv7m | CoreType::Armv7em => FUNCTION_V7M,
        _ => return Ok(false),
    };
    let Some(address) = address else {
        core.write_word_32(DWT_FUNCTION0, 0)?;
        return Ok(true);
    };
    core.write_word_32(DWT_FUNCTION0, 0)?;
    core.write_word_32(DWT_COMP0, address as u32)?;
    if function == FUNCTION_V7M {
        core.write_word_32(DWT_MASK0, 0)?;
    }
    core.write_word_32(DWT_FUNCTION0, function)?;
    Ok(true)
}

/// Turns the DWT's exception trace on or off. Off, SWO has more room: each
/// task switch is an exception too, as Zephyr's PendSV.
pub fn trace_exceptions(core: &mut Core, on: bool) -> Result<(), probe_rs::Error> {
    let ctrl = core.read_word_32(DWT_CTRL)?;
    let ctrl = if on {
        ctrl | EXCTRCENA
    } else {
        ctrl & !EXCTRCENA
    };
    core.write_word_32(DWT_CTRL, ctrl)
}

/// An exception entered, left or returned to, at a time in ms after the
/// trace's base.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Interrupt {
    pub at: f64,
    /// The exception's number: 15 is SysTick, 16 and on are the IRQs, 0 the thread.
    pub number: u16,
    pub event: Event,
}

/// What happened to an exception, as the page reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Event {
    Entered,
    Exited,
    Returned,
    /// Some of the trace was lost here, of exception 0: an exception entered
    /// before may have been left unseen.
    Lost,
}

impl From<ExceptionEvent> for Event {
    fn from(event: ExceptionEvent) -> Self {
        match event {
            ExceptionEvent::Entered => Event::Entered,
            ExceptionEvent::Exited => Event::Exited,
            ExceptionEvent::Returned => Event::Returned,
        }
    }
}

/// What tracing saw since it last told.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Traced {
    /// The whole ms since 1970 the times count from: when tracing began.
    pub base: f64,
    /// The task switches, exact.
    pub running: Option<Running>,
    pub interrupts: Vec<Interrupt>,
}

/// How often what was traced is told.
const TELL_EVERY: Duration = Duration::from_millis(100);
/// How far the chip's time may fall behind the computer's before it is set
/// again: time is lost with SWO's bytes, as when the chip's trace FIFO
/// overflows, or the probe loses them while it reads memory. More than the
/// time bytes take to reach terNIMA, which varies.
const BEHIND_MAX_MS: f64 = 30.0;
/// How far ahead of the computer's it may go, which only a clock set wrong does.
const AHEAD_MAX_MS: f64 = 250.0;
/// How long the least lag of the computer's clock over the chip's is kept:
/// over two of these.
const LAG_WINDOW_MS: f64 = 250.0;
/// How long a check of a read of memory waits for SWO's bytes as they are
/// sent again, after those the probe kept through the read.
const CHECK_MS: f64 = 15.0;
/// The longest a check waits for SWO's bytes at all: a quiet chip sends none.
const CHECK_MAX_MS: f64 = 200.0;
/// Less than this behind, the chip's time only wavered with how long bytes
/// take to come: nothing was lost.
const LOST_MIN_MS: f64 = 0.5;

/// The least the computer's clock was ahead of the chip's lately, over the
/// last half second or so: how long SWO's bytes took to be read, at best.
#[derive(Clone, Copy, Debug)]
struct Lag {
    current: f64,
    previous: f64,
    /// When the current window began, by the computer's clock.
    since: f64,
}

impl Lag {
    fn new(clock: f64) -> Self {
        Self {
            current: f64::INFINITY,
            previous: f64::INFINITY,
            since: clock,
        }
    }

    fn note(&mut self, lag: f64, clock: f64) {
        if clock - self.since > LAG_WINDOW_MS {
            self.previous = self.current;
            self.current = f64::INFINITY;
            self.since = clock;
        }
        self.current = self.current.min(lag);
    }

    /// Infinite while none is known.
    fn least(&self) -> f64 {
        self.current.min(self.previous)
    }
}

/// A read of memory being checked: the probe may have lost some of SWO's
/// bytes through it, and with them the time they held.
#[derive(Debug)]
struct Check {
    /// When it began, by the computer's clock, and the chip's time then.
    began: f64,
    from: f64,
    /// The least lag before it.
    lag: f64,
    /// How much had been traced when it began.
    switches: usize,
    interrupts: usize,
    /// How much had once the first bytes after it were in: those the probe
    /// kept through it, which came late.
    after: Option<(usize, usize)>,
    /// The least lag since, of the bytes as they were sent again.
    lag_after: f64,
    /// When to decide, by the computer's clock.
    until: f64,
}

/// A packet waiting for the timestamp after it, which tells its time.
enum Waiting {
    Switch(u64),
    Exception(u16, ExceptionEvent),
    /// The chip dropped packets here, its trace FIFO full.
    Overflow,
}

/// Turns the ITM's packets into timed task switches and exceptions.
pub struct Tracer {
    /// The clock the timestamps count, in Hz: the core's.
    clock_hz: f64,
    /// Ticks counted since tracing began.
    ticks: u64,
    /// The whole ms since 1970 the times told count from: when tracing
    /// began. Counted from it, a time keeps its nanoseconds.
    base: f64,
    /// The time, in ms after `base`, that `ticks` 0 stands for.
    start: Option<f64>,
    /// The last time told: times only go on, also when the clock is set again.
    last: f64,
    waiting: Vec<Waiting>,
    /// Whether task switches are traced, which `running` then has.
    switches: bool,
    /// The tasks' addresses, as last read: a value that is none of them came
    /// garbled, as when SWO lost a byte, and is left out, as all are before
    /// the tasks are first read.
    known: HashSet<u64>,
    running: Running,
    last_task: Option<u64>,
    interrupts: Vec<Interrupt>,
    told: Instant,
    lag: Lag,
    check: Option<Check>,
    /// While the chip is halted: the task that ran when it was, to run on after.
    halted: Option<Option<u64>>,
    /// Whether the next timestamp sets the chip's time again, as after a halt.
    resync: bool,
}

impl Tracer {
    pub fn new(clock_hz: u32) -> Self {
        let base = now_ms().floor();
        Self {
            clock_hz: f64::from(clock_hz.max(1)),
            ticks: 0,
            base,
            start: None,
            last: 0.0,
            waiting: Vec::new(),
            switches: false,
            known: HashSet::new(),
            running: Self::fresh(base),
            last_task: None,
            interrupts: Vec::new(),
            told: Instant::now(),
            lag: Lag::new(0.0),
            check: None,
            halted: None,
            resync: false,
        }
    }

    /// Nothing traced yet: every switch, counted from `base`.
    fn fresh(base: f64) -> Running {
        Running {
            base,
            exact: true,
            ..Running::default()
        }
    }

    /// Whether task switches are traced.
    pub fn traces_switches(&self) -> bool {
        self.switches
    }

    /// Whether task switches are traced from now on.
    pub fn trace_switches(&mut self, on: bool) {
        self.switches = on;
        self.last_task = None;
    }

    /// Learns the tasks there are, from a read of them.
    pub fn know(&mut self, tasks: impl IntoIterator<Item = u64>) {
        let known: HashSet<u64> = tasks.into_iter().collect();
        if !known.is_empty() {
            self.known = known;
        }
    }

    /// Learns one more task, as SystemView names them.
    pub fn know_task(&mut self, task: u64) {
        self.known.insert(task);
    }

    /// Takes in a packet: a switch or an exception waits for the timestamp
    /// after it.
    pub fn feed(&mut self, packet: &ItmPacket) {
        self.feed_at(packet, now_ms() - self.base);
    }

    /// As `feed`, read when the computer's clock said `clock`, in ms after `base`.
    fn feed_at(&mut self, packet: &ItmPacket, clock: f64) {
        match *packet {
            // Sent before the chip halted, and read after: it ran when it halted.
            ItmPacket::Value {
                comparator: 0,
                write: true,
                value,
            } if self.switches && self.halted.is_some() => {
                if self.known.contains(&u64::from(value)) {
                    self.halted = Some(Some(u64::from(value)));
                }
            }
            ItmPacket::Value {
                comparator: 0,
                write: true,
                value,
            } if self.switches => self.waiting.push(Waiting::Switch(u64::from(value))),
            ItmPacket::Exception { .. } if self.halted.is_some() => {}
            ItmPacket::Exception { number, event } => {
                self.waiting.push(Waiting::Exception(number, event));
            }
            ItmPacket::Time(delta) => {
                let before = self.last;
                self.ticks += u64::from(delta);
                // Run again after a halt, whether the timestamps counted it or
                // not: the chip's time goes on from the computer's.
                if std::mem::take(&mut self.resync) {
                    let elapsed = self.ticks as f64 * 1000.0 / self.clock_hz;
                    self.start = Some(self.estimate(clock) - elapsed);
                }
                let (at, behind) = self.now_at(clock);
                let waiting = std::mem::take(&mut self.waiting);
                // The chip dropped packets since the last timestamp, and held
                // the timestamps back, as when switches come microseconds
                // apart with the interrupts traced: when what came since
                // happened is not known, nor what ran. A switch after the
                // last packets dropped says what runs by now, though.
                if let Some(dropped) = waiting
                    .iter()
                    .rposition(|item| matches!(item, Waiting::Overflow))
                {
                    self.unknown_from(before);
                    let after = waiting[dropped..].iter().rev().find_map(|item| match item {
                        Waiting::Switch(task) if self.known.contains(task) => Some(*task),
                        _ => None,
                    });
                    if let Some(task) = after {
                        self.running.switches.push((at, task));
                        self.running.until = self.running.until.max(at);
                        self.last_task = Some(task);
                    }
                    return;
                }
                // Time lost with SWO's bytes: what ran until the clock was
                // set again is not known; the task most likely ran on.
                if let Some(was) = behind {
                    if self.switches
                        && let Some(task) = self.last_task
                    {
                        self.running.switches.push((was, 0));
                        self.running.switches.push((at, task));
                    }
                    self.interrupts.push(Interrupt {
                        at: was,
                        number: 0,
                        event: Event::Lost,
                    });
                }
                for item in waiting {
                    self.add(item, at);
                }
            }
            // Told at the timestamp after it.
            ItmPacket::Overflow => self.waiting.push(Waiting::Overflow),
            _ => {}
        }
    }

    /// Marks the trace from the last packet on as not known, as SWO's bytes
    /// were lost: the probe loses them while it reads memory for long. What
    /// ran until the next switch, and when an interrupt entered before left,
    /// is not known, so a run is not drawn on across what was lost.
    pub fn lost(&mut self) {
        self.unknown_from(self.last);
    }

    /// What was traced from `at` on is not known: the packets waiting for
    /// their time lost it, the task that ran is not known until the next
    /// switch, and when an interrupt entered before left is not known.
    fn unknown_from(&mut self, at: f64) {
        self.waiting.clear();
        if self.switches && self.last_task.take().is_some() {
            self.running.switches.push((at, 0));
        }
        if self
            .interrupts
            .last()
            .is_none_or(|last| last.event != Event::Lost)
        {
            self.interrupts.push(Interrupt {
                at,
                number: 0,
                event: Event::Lost,
            });
        }
    }

    /// The chip's time now as the computer's clock tells, `clock`, less how
    /// long SWO's bytes take to come at best; never before the last told.
    fn estimate(&self, clock: f64) -> f64 {
        let lag = self.lag.least();
        (clock - if lag.is_finite() { lag } else { 0.0 }).max(self.last)
    }

    /// The chip was halted, or runs again, by the Live view's Pause. Halted,
    /// the task that ran stays marked halted until it runs again, from when
    /// it runs on. The probe reads what SWO still had first.
    pub fn halt(&mut self, halted: bool) {
        self.halt_at(halted, now_ms() - self.base);
    }

    fn halt_at(&mut self, halted: bool, clock: f64) {
        self.decide(f64::INFINITY);
        let at = self.estimate(clock);
        if halted && self.halted.is_none() {
            self.halted = Some(self.last_task.filter(|&task| task != HALTED));
            self.waiting.clear();
            if self.switches {
                self.running.switches.push((at, HALTED));
                self.last_task = Some(HALTED);
            }
        } else if !halted && let Some(task) = self.halted.take() {
            if self.switches {
                self.running.switches.push((at, task.unwrap_or(0)));
                self.last_task = task;
            }
            self.resync = true;
        } else {
            return;
        }
        self.last = at;
        self.running.until = self.running.until.max(at);
    }

    /// The probe read memory for long: it may have lost some of SWO's bytes
    /// meanwhile, and the time they held, as an ST-Link does. Checked once
    /// the bytes come as they are sent again: if the chip's time fell behind
    /// the computer's through the read, what came of it is not known, and
    /// the time goes on that much later.
    pub fn may_have_lost(&mut self) {
        self.may_have_lost_at(now_ms() - self.base);
    }

    fn may_have_lost_at(&mut self, clock: f64) {
        // One checked before is decided with what it has.
        self.decide(f64::INFINITY);
        let lag = self.lag.least();
        if lag.is_finite() {
            self.check = Some(Check {
                began: clock,
                from: self.last,
                lag,
                switches: self.running.switches.len(),
                interrupts: self.interrupts.len(),
                after: None,
                lag_after: f64::INFINITY,
                until: f64::INFINITY,
            });
        }
    }

    /// A read of SWO's bytes was fed in: the first after a read of memory
    /// has those the probe kept through it.
    pub fn chunk_fed(&mut self) {
        self.chunk_fed_at(now_ms() - self.base);
    }

    fn chunk_fed_at(&mut self, clock: f64) {
        if let Some(check) = &mut self.check
            && check.after.is_none()
        {
            check.after = Some((self.running.switches.len(), self.interrupts.len()));
            check.until = clock + CHECK_MS;
        }
        self.decide(clock);
    }

    /// Decides a read being checked, once it is time. The chip's time
    /// behind the computer's by more than before, SWO's bytes were lost
    /// through the read, with that much time: what the probe kept through it
    /// is left out, as when it was sent is not known; what came after is set
    /// that much later; and the read leaves a gap.
    fn decide(&mut self, clock: f64) {
        let Some(check) = &self.check else {
            return;
        };
        let due = match check.after {
            Some(_) => clock >= check.until,
            None => clock - check.began >= CHECK_MAX_MS,
        };
        if !due {
            return;
        }
        let Some(check) = self.check.take() else {
            return;
        };
        let Some((switches, interrupts)) = check.after else {
            return;
        };
        let lost = check.lag_after - check.lag;
        if !lost.is_finite() || lost < LOST_MIN_MS {
            return;
        }
        self.running.switches.drain(check.switches..switches);
        self.interrupts.drain(check.interrupts..interrupts);
        for switch in &mut self.running.switches[check.switches..] {
            switch.0 += lost;
        }
        for interrupt in &mut self.interrupts[check.interrupts..] {
            interrupt.at += lost;
        }
        self.running
            .switches
            .insert(check.switches, (check.from, 0));
        self.interrupts.insert(
            check.interrupts,
            Interrupt {
                at: check.from,
                number: 0,
                event: Event::Lost,
            },
        );
        if let Some(start) = &mut self.start {
            *start += lost;
        }
        self.last += lost;
        self.running.until = (self.running.until + lost).max(check.from);
        self.last_task = self
            .running
            .switches
            .last()
            .map(|&(_, task)| task)
            .filter(|&task| task != 0);
        // Set on, the chip's time lags as it did before.
        self.lag = Lag {
            current: check.lag,
            previous: check.lag,
            since: clock,
        };
    }

    /// The chip's time now, in ms after `base`: set from the computer's
    /// clock at the start, and again when the two drift apart; with the time
    /// it would have been, when it fell behind and was set again.
    fn now_at(&mut self, clock: f64) -> (f64, Option<f64>) {
        let elapsed = self.ticks as f64 * 1000.0 / self.clock_hz;
        let start = *self.start.get_or_insert(clock - elapsed);
        let at = start + elapsed;
        let (at, behind) = if clock - at > BEHIND_MAX_MS {
            self.anchor(clock - elapsed, clock);
            (clock, Some(at.max(self.last)))
        } else if at - clock > AHEAD_MAX_MS {
            self.anchor(clock - elapsed, clock);
            (clock, None)
        } else {
            match &mut self.check {
                // Those the probe kept through a read came late: their lag says nothing.
                Some(Check { after: None, .. }) => {}
                Some(check) => check.lag_after = check.lag_after.min(clock - at),
                None => self.lag.note(clock - at, clock),
            }
            (at, None)
        };
        self.last = self.last.max(at);
        (self.last, behind)
    }

    /// Sets the chip's time again: the lag known, and a read being checked,
    /// were of the time before.
    fn anchor(&mut self, start: f64, clock: f64) {
        self.start = Some(start);
        self.lag = Lag::new(clock);
        self.check = None;
    }

    fn add(&mut self, waiting: Waiting, at: f64) {
        match waiting {
            Waiting::Switch(task) => {
                // Until the tasks are first read, no value can be told from a garbled one.
                if !self.known.contains(&task) {
                    return;
                }
                if self.last_task != Some(task) {
                    self.running.switches.push((at, task));
                    self.last_task = Some(task);
                }
                self.running.samples += 1;
            }
            Waiting::Exception(number, event) => self.interrupts.push(Interrupt {
                at,
                number,
                event: event.into(),
            }),
            // Taken before, at the timestamp.
            Waiting::Overflow => {}
        }
        self.running.until = self.running.until.max(at);
    }

    /// What was traced since the last time, every so often.
    pub fn take_if_due(&mut self) -> Option<Traced> {
        self.take_if_due_at(now_ms() - self.base)
    }

    fn take_if_due_at(&mut self, clock: f64) -> Option<Traced> {
        if self.told.elapsed() < TELL_EVERY {
            return None;
        }
        // Held while a read is checked: what came after it may move.
        self.decide(clock);
        if self.check.is_some() {
            return None;
        }
        self.told = Instant::now();
        // The chip's time now, as far as the computer's clock tells: the
        // last task runs on until then, as the trace has no switch since.
        let now = self.estimate(clock);
        // Switches traced before tracing them stopped are still told.
        let running = (self.switches || !self.running.switches.is_empty()).then(|| {
            let mut running = std::mem::replace(&mut self.running, Self::fresh(self.base));
            if self.last_task.is_some() {
                running.until = running.until.max(now);
            }
            running
        });
        let interrupts = std::mem::take(&mut self.interrupts);
        (running.is_some() || !interrupts.is_empty()).then_some(Traced {
            base: self.base,
            running,
            interrupts,
        })
    }
}

/// Now, in ms since 1970, as the app times what sessions read.
fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64() * 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switches_and_exceptions_take_the_time_after_them() {
        let mut tracer = Tracer::new(1_000_000);
        tracer.trace_switches(true);
        tracer.know([0x2000_0168, 0x2000_00A8]);
        tracer.feed(&ItmPacket::Time(0));
        let start = tracer.running.until;
        tracer.feed(&ItmPacket::Value {
            comparator: 0,
            write: true,
            value: 0x2000_0168,
        });
        // 5 µs at 1 MHz.
        tracer.feed(&ItmPacket::Time(5));
        tracer.feed(&ItmPacket::Exception {
            number: 66,
            event: ExceptionEvent::Entered,
        });
        tracer.feed(&ItmPacket::Value {
            comparator: 0,
            write: true,
            value: 0x2000_00A8,
        });
        tracer.feed(&ItmPacket::Time(3));
        let switches = &tracer.running.switches;
        assert_eq!(switches.len(), 2);
        assert_eq!(switches[0].1, 0x2000_0168);
        assert_eq!(switches[1].1, 0x2000_00A8);
        assert!((switches[1].0 - switches[0].0 - 0.003).abs() < 1e-12);
        assert_eq!(tracer.interrupts.len(), 1);
        assert_eq!(tracer.interrupts[0].number, 66);
        assert_eq!(tracer.interrupts[0].event, Event::Entered);
        assert!((tracer.interrupts[0].at - switches[1].0).abs() < 1e-12);
        assert!(start <= switches[0].0);
    }

    #[test]
    fn a_clock_cycle_apart_is_told_apart() {
        let mut tracer = Tracer::new(240_000_000);
        tracer.trace_switches(true);
        tracer.know([0x2000_0168, 0x2000_00A8]);
        for (task, ticks) in [(0x2000_0168, 1_000), (0x2000_00A8, 1)] {
            tracer.feed(&ItmPacket::Value {
                comparator: 0,
                write: true,
                value: task,
            });
            tracer.feed(&ItmPacket::Time(ticks));
        }
        let switches = &tracer.running.switches;
        // One cycle at 240 MHz: ms since 1970 could not tell it.
        let cycle = 1.0 / 240_000.0;
        assert!((switches[1].0 - switches[0].0 - cycle).abs() < 1e-12);
        // Told with the whole ms since 1970 the times count from.
        tracer.told = Instant::now() - TELL_EVERY;
        let traced = tracer.take_if_due().unwrap();
        let running = traced.running.unwrap();
        assert_eq!(traced.base, tracer.base);
        assert_eq!(running.base, tracer.base);
        assert_eq!(running.base.fract(), 0.0);
        assert!(running.base > 1.7e12);
        assert!(running.exact);
        assert_eq!(running.switches.len(), 2);
        assert!(tracer.running.switches.is_empty());
        assert_eq!(tracer.running.base, tracer.base);
    }

    #[test]
    fn the_same_task_written_again_is_no_switch() {
        let mut tracer = Tracer::new(1_000_000);
        tracer.trace_switches(true);
        tracer.know([0x2000_0168, 0x2000_00A8]);
        for task in [0x2000_0168, 0x2000_0168, 0x2000_00A8] {
            tracer.feed(&ItmPacket::Value {
                comparator: 0,
                write: true,
                value: task,
            });
            tracer.feed(&ItmPacket::Time(10));
        }
        assert_eq!(tracer.running.switches.len(), 2);
        assert_eq!(tracer.running.samples, 3);
    }

    #[test]
    fn an_overflow_leaves_a_gap_and_switches_untraced_are_left_out() {
        let mut tracer = Tracer::new(1_000_000);
        tracer.feed(&ItmPacket::Value {
            comparator: 0,
            write: true,
            value: 0x2000_0168,
        });
        tracer.feed(&ItmPacket::Time(10));
        assert!(tracer.running.switches.is_empty());
        tracer.trace_switches(true);
        tracer.know([0x2000_0168, 0x2000_00A8]);
        tracer.feed(&ItmPacket::Value {
            comparator: 0,
            write: true,
            value: 0x2000_0168,
        });
        tracer.feed(&ItmPacket::Time(10));
        let known = tracer.last;
        // Dropped packets, and the timestamps held back: what came with them
        // has no time, as in a burst of switches with the interrupts traced.
        tracer.feed(&ItmPacket::Exception {
            number: 14,
            event: ExceptionEvent::Entered,
        });
        tracer.feed(&ItmPacket::Overflow);
        tracer.feed(&ItmPacket::Time(500));
        let tasks: Vec<u64> = tracer
            .running
            .switches
            .iter()
            .map(|&(_, task)| task)
            .collect();
        assert_eq!(tasks, [0x2000_0168, 0]);
        // Not known from the last time known on.
        assert_eq!(tracer.running.switches[1].0, known);
        let events: Vec<Event> = tracer
            .interrupts
            .iter()
            .map(|interrupt| interrupt.event)
            .collect();
        assert_eq!(events, [Event::Lost]);
        // A switch after the last packets dropped: it runs by the timestamp.
        tracer.feed(&ItmPacket::Overflow);
        tracer.feed(&ItmPacket::Value {
            comparator: 0,
            write: true,
            value: 0x2000_00A8,
        });
        tracer.feed(&ItmPacket::Time(500));
        let switches = &tracer.running.switches;
        assert_eq!(switches.len(), 3);
        assert_eq!(switches[2], (tracer.last, 0x2000_00A8));
        // Timed again, the next switch is known when it was.
        tracer.feed(&ItmPacket::Value {
            comparator: 0,
            write: true,
            value: 0x2000_0168,
        });
        tracer.feed(&ItmPacket::Time(10));
        assert_eq!(tracer.running.switches.len(), 4);
    }

    #[test]
    fn what_was_lost_is_a_gap_and_ends_the_interrupts_entered() {
        let mut tracer = Tracer::new(1_000_000);
        tracer.trace_switches(true);
        tracer.know([0x2000_0168]);
        tracer.feed(&ItmPacket::Exception {
            number: 14,
            event: ExceptionEvent::Entered,
        });
        tracer.feed(&ItmPacket::Value {
            comparator: 0,
            write: true,
            value: 0x2000_0168,
        });
        tracer.feed(&ItmPacket::Time(10));
        // A packet still waiting for its time when the bytes were lost.
        tracer.feed(&ItmPacket::Exception {
            number: 14,
            event: ExceptionEvent::Exited,
        });
        tracer.lost();
        tracer.lost();
        let tasks: Vec<u64> = tracer
            .running
            .switches
            .iter()
            .map(|&(_, task)| task)
            .collect();
        assert_eq!(tasks, [0x2000_0168, 0]);
        let events: Vec<Event> = tracer
            .interrupts
            .iter()
            .map(|interrupt| interrupt.event)
            .collect();
        assert_eq!(events, [Event::Entered, Event::Lost]);
        assert_eq!(tracer.interrupts[1].number, 0);
        assert!(tracer.waiting.is_empty());
    }

    #[test]
    fn switches_traced_before_tracing_stopped_are_still_told() {
        let mut tracer = Tracer::new(1_000_000);
        tracer.trace_switches(true);
        tracer.know([0x2000_0168]);
        tracer.feed(&ItmPacket::Value {
            comparator: 0,
            write: true,
            value: 0x2000_0168,
        });
        tracer.feed(&ItmPacket::Time(10));
        tracer.trace_switches(false);
        tracer.told = Instant::now() - TELL_EVERY;
        let running = tracer.take_if_due().unwrap().running.unwrap();
        assert_eq!(running.switches.len(), 1);
        tracer.told = Instant::now() - TELL_EVERY;
        assert!(tracer.take_if_due().is_none());
    }

    const A: u32 = 0x2000_0168;
    const B: u32 = 0x2000_00A8;

    fn traced_at_a_megahertz() -> Tracer {
        let mut tracer = Tracer::new(1_000_000);
        tracer.trace_switches(true);
        tracer.know([u64::from(A), u64::from(B)]);
        tracer
    }

    /// A switch to `task`, and its timestamp `ticks` after the last, read
    /// when the computer's clock said `clock`. A tick is a microsecond.
    fn switch_at(tracer: &mut Tracer, task: u32, ticks: u32, clock: f64) {
        tracer.feed_at(
            &ItmPacket::Value {
                comparator: 0,
                write: true,
                value: task,
            },
            clock,
        );
        tracer.feed_at(&ItmPacket::Time(ticks), clock);
    }

    fn tasks(tracer: &Tracer) -> Vec<u64> {
        tracer
            .running
            .switches
            .iter()
            .map(|&(_, task)| task)
            .collect()
    }

    #[test]
    fn a_read_through_which_nothing_was_lost_leaves_no_gap() {
        let mut tracer = traced_at_a_megahertz();
        switch_at(&mut tracer, A, 1000, 10.0);
        switch_at(&mut tracer, B, 1000, 11.1);
        switch_at(&mut tracer, A, 1000, 12.0);
        tracer.may_have_lost_at(12.5);
        // Kept by the probe through the read, and read late.
        switch_at(&mut tracer, B, 3000, 17.6);
        tracer.chunk_fed_at(17.6);
        // As sent again: the chip's time lags as before.
        switch_at(&mut tracer, A, 3000, 18.1);
        tracer.chunk_fed_at(40.0);
        let (a, b) = (u64::from(A), u64::from(B));
        assert_eq!(tasks(&tracer), [a, b, a, b, a]);
        assert_eq!(tracer.running.switches[3].0, 15.0);
        assert_eq!(tracer.running.switches[4].0, 18.0);
        assert!(tracer.check.is_none());
    }

    #[test]
    fn a_read_through_which_time_was_lost_leaves_a_gap_and_sets_the_time_on() {
        let mut tracer = traced_at_a_megahertz();
        switch_at(&mut tracer, A, 1000, 10.0);
        switch_at(&mut tracer, B, 1000, 11.1);
        switch_at(&mut tracer, A, 1000, 12.0);
        tracer.may_have_lost_at(12.5);
        // Kept through the read: when it was sent is not known.
        switch_at(&mut tracer, B, 1000, 17.6);
        tracer.feed_at(
            &ItmPacket::Exception {
                number: 66,
                event: ExceptionEvent::Entered,
            },
            17.6,
        );
        tracer.feed_at(&ItmPacket::Time(10), 17.6);
        tracer.chunk_fed_at(17.6);
        // 3 ms of bytes lost: the chip's 18.0 counts as 15.0.
        switch_at(&mut tracer, A, 1990, 18.1);
        tracer.feed_at(
            &ItmPacket::Exception {
                number: 66,
                event: ExceptionEvent::Exited,
            },
            18.2,
        );
        tracer.feed_at(&ItmPacket::Time(100), 18.2);
        tracer.chunk_fed_at(40.0);
        let (a, b) = (u64::from(A), u64::from(B));
        assert_eq!(tasks(&tracer), [a, b, a, 0, a]);
        let switches = &tracer.running.switches;
        assert_eq!(switches[3].0, 12.0);
        assert!((switches[4].0 - 18.1).abs() < 1e-9);
        let events: Vec<(f64, Event)> = tracer
            .interrupts
            .iter()
            .map(|interrupt| (interrupt.at, interrupt.event))
            .collect();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0], (12.0, Event::Lost));
        assert!((events[1].0 - 18.2).abs() < 1e-9);
        // Timed on from there, as it lags again.
        switch_at(&mut tracer, B, 1000, 19.3);
        assert!((tracer.running.switches[5].0 - 19.2).abs() < 1e-9);
    }

    #[test]
    fn what_was_traced_is_held_while_a_read_is_checked() {
        let mut tracer = traced_at_a_megahertz();
        switch_at(&mut tracer, A, 1000, 10.0);
        tracer.may_have_lost_at(10.5);
        tracer.told = Instant::now() - TELL_EVERY;
        assert!(tracer.take_if_due_at(11.0).is_none());
        // No bytes came for long: nothing is known lost.
        let running = tracer
            .take_if_due_at(10.5 + CHECK_MAX_MS)
            .unwrap()
            .running
            .unwrap();
        assert_eq!(running.switches, [(10.0, u64::from(A))]);
    }

    #[test]
    fn a_quiet_chip_keeps_its_time() {
        let mut tracer = traced_at_a_megahertz();
        switch_at(&mut tracer, A, 1000, 10.0);
        tracer.told = Instant::now() - TELL_EVERY;
        // Nothing comes for 100 ms: the task ran on until now.
        let running = tracer.take_if_due_at(110.0).unwrap().running.unwrap();
        assert_eq!(running.until, 110.0);
        // The next switch is where the chip timed it.
        switch_at(&mut tracer, B, 150_000, 160.1);
        assert_eq!(tracer.running.switches, [(160.0, u64::from(B))]);
    }

    #[test]
    fn a_halt_is_marked_and_the_task_runs_on_after_it() {
        let mut tracer = traced_at_a_megahertz();
        switch_at(&mut tracer, A, 1000, 10.0);
        switch_at(&mut tracer, B, 1000, 11.0);
        tracer.halt_at(true, 11.5);
        // Sent before the halt and read after it: A ran when it halted, not B.
        switch_at(&mut tracer, A, 10, 11.6);
        tracer.halt_at(true, 12.0);
        tracer.told = Instant::now() - TELL_EVERY;
        let running = tracer.take_if_due_at(500.0).unwrap().running.unwrap();
        let (a, b) = (u64::from(A), u64::from(B));
        assert_eq!(running.switches, [(10.0, a), (11.0, b), (11.5, HALTED)]);
        // Halted still, until now.
        assert_eq!(running.until, 500.0);
        tracer.halt_at(false, 2011.5);
        assert_eq!(tracer.running.switches, [(2011.5, a)]);
        // The chip's time goes on from the computer's, whatever the timestamp counted.
        switch_at(&mut tracer, B, 5, 2012.0);
        assert_eq!(tracer.running.switches[1], (2012.0, b));
    }

    #[test]
    fn a_value_that_is_no_task_is_left_out() {
        let mut tracer = Tracer::new(1_000_000);
        tracer.trace_switches(true);
        tracer.know([0x2000_0168, 0x2000_00A8]);
        for task in [0x2000_0168, 0x0E0E_2000, 0x2000_00A8] {
            tracer.feed(&ItmPacket::Value {
                comparator: 0,
                write: true,
                value: task,
            });
            tracer.feed(&ItmPacket::Time(10));
        }
        let tasks: Vec<u64> = tracer
            .running
            .switches
            .iter()
            .map(|&(_, task)| task)
            .collect();
        assert_eq!(tasks, [0x2000_0168, 0x2000_00A8]);
    }

    #[test]
    fn the_encodings_are_as_the_manuals_say() {
        assert_eq!(FUNCTION_V8M, 0x82D);
        assert_eq!(FUNCTION_V7M, 0x80D);
    }
}
