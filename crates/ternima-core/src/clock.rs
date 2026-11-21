//! The core's clock, found from the chip, for SWO: the firmware's own
//! `SystemCoreClock`, checked against the core's cycle counter, or else the
//! counter alone.

use std::time::{Duration, Instant};

use probe_rs::{Core, MemoryInterface};
use serde::Serialize;

const DHCSR: u64 = 0xE000_EDF0;
const S_HALT: u32 = 1 << 17;
const S_SLEEP: u32 = 1 << 18;
const DEMCR: u64 = 0xE000_EDFC;
const TRCENA: u32 = 1 << 24;
const DWT_CTRL: u64 = 0xE000_1000;
const CYCCNTENA: u32 = 1;
const NOCYCCNT: u32 = 1 << 25;
const DWT_CYCCNT: u64 = 0xE000_1004;

/// How long the cycle counter is watched.
const COUNT_FOR: Duration = Duration::from_millis(250);
/// The share of the time the core may sleep, and its count surely be its
/// clock: some cores stop counting while they sleep, though most count on
/// while a probe is attached, as the STM32H5 does.
const SLEEPING_MAX: f64 = 0.02;
/// How near a whole MHz a count is to be taken for it, a core's clock being
/// set so: the count is good to about 0.01%.
const WHOLE: f64 = 0.001;
/// How far the firmware's clock and the count may differ and agree.
const AGREE: f64 = 0.02;

/// The core's clock, as found.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Clock {
    pub hz: u32,
    /// Whether it is the firmware's own `SystemCoreClock`, the count agreeing.
    pub from_firmware: bool,
    /// Whether it may be low: counted while the core slept much of the time,
    /// not near a whole MHz.
    pub about: bool,
}

/// What counting the cycles found: cycles a second, while the core ran, and
/// the share of the time it slept.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Counted {
    hz: f64,
    sleeping: f64,
}

fn text(error: probe_rs::Error) -> String {
    error.to_string()
}

/// Finds the core's clock: reads the firmware's `SystemCoreClock` at
/// `system_core_clock`, when its ELF file has one, and counts the core's
/// cycles for a quarter of a second. The chip runs on as it is.
pub fn find(core: &mut Core, system_core_clock: Option<u64>) -> Result<Clock, String> {
    let firmware = system_core_clock
        .and_then(|address| core.read_word_32(address).ok())
        .filter(|hz| (1_000_000..=2_000_000_000).contains(hz));
    let counted = count(core)?;
    Ok(choose(firmware, counted))
}

/// Counts the core's cycles, with its cycle counter turned on for it, and
/// left as it was after.
fn count(core: &mut Core) -> Result<Counted, String> {
    let demcr = core.read_word_32(DEMCR).map_err(text)?;
    if demcr & TRCENA == 0 {
        core.write_word_32(DEMCR, demcr | TRCENA).map_err(text)?;
    }
    let ctrl = core.read_word_32(DWT_CTRL).map_err(text)?;
    if ctrl & NOCYCCNT != 0 {
        return Err("This core has no cycle counter to measure its clock with: enter it.".into());
    }
    if ctrl & CYCCNTENA == 0 {
        core.write_word_32(DWT_CTRL, ctrl | CYCCNTENA)
            .map_err(text)?;
    }
    let counted = watch(core);
    if ctrl & CYCCNTENA == 0 {
        let _ = core.write_word_32(DWT_CTRL, ctrl);
    }
    if demcr & TRCENA == 0 {
        let _ = core.write_word_32(DEMCR, demcr);
    }
    counted
}

/// Reads the counter, then whether the core sleeps, again and again, then
/// the counter again: each read of it timed halfway through.
fn watch(core: &mut Core) -> Result<Counted, String> {
    let stamp = |core: &mut Core| -> Result<(Instant, u32), String> {
        let before = Instant::now();
        let cycles = core.read_word_32(DWT_CYCCNT).map_err(text)?;
        Ok((before + before.elapsed() / 2, cycles))
    };
    let (first_at, first) = stamp(core)?;
    let (mut looks, mut sleeping, mut halted) = (0u32, 0u32, 0u32);
    while first_at.elapsed() < COUNT_FOR {
        let dhcsr = core.read_word_32(DHCSR).map_err(text)?;
        looks += 1;
        sleeping += u32::from(dhcsr & S_SLEEP != 0);
        halted += u32::from(dhcsr & S_HALT != 0);
    }
    let (last_at, last) = stamp(core)?;
    if halted * 2 > looks {
        return Err("The chip is halted: let it run to measure its clock.".into());
    }
    let cycles = f64::from(last.wrapping_sub(first));
    let seconds = last_at.duration_since(first_at).as_secs_f64();
    if cycles <= 0.0 || seconds <= 0.0 {
        return Err("The core's cycle counter did not count: let the chip run.".into());
    }
    Ok(Counted {
        hz: cycles / seconds,
        sleeping: f64::from(sleeping) / f64::from(looks.max(1)),
    })
}

/// The clock from what the firmware says and what was counted. The
/// firmware's is taken when the count agrees, being exact; or when the count
/// is less and the core slept, as a core may stop counting asleep. Else the
/// count is taken, rounded as clocks are set: where the core slept and the
/// count is not near a whole MHz, it may be low.
fn choose(firmware: Option<u32>, counted: Counted) -> Clock {
    let asleep = counted.sleeping >= SLEEPING_MAX;
    if let Some(hz) = firmware {
        let firmware_hz = f64::from(hz);
        let agrees = (counted.hz - firmware_hz).abs() <= firmware_hz * AGREE;
        if agrees || (asleep && counted.hz < firmware_hz) {
            return Clock {
                hz,
                from_firmware: true,
                about: false,
            };
        }
    }
    let whole = ((counted.hz / 1e6).round() * 1e6 - counted.hz).abs() <= counted.hz * WHOLE;
    Clock {
        hz: round(counted.hz),
        from_firmware: false,
        about: asleep && !whole,
    }
}

/// A clock counted, as it is most likely set: in whole MHz when that is near,
/// or else in tenths of one.
fn round(hz: f64) -> u32 {
    let mhz = (hz / 1e6).round() * 1e6;
    let rounded = if (hz - mhz).abs() <= hz * 0.01 {
        mhz
    } else {
        (hz / 1e5).round() * 1e5
    };
    rounded.clamp(0.0, f64::from(u32::MAX)) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counted(hz: f64, sleeping: f64) -> Counted {
        Counted { hz, sleeping }
    }

    #[test]
    fn the_firmware_s_clock_is_taken_when_the_count_agrees() {
        let clock = choose(Some(240_000_000), counted(239_400_000.0, 0.0));
        assert_eq!(
            clock,
            Clock {
                hz: 240_000_000,
                from_firmware: true,
                about: false
            }
        );
        // Asleep much of the time, the count is less: still the firmware's.
        let clock = choose(Some(240_000_000), counted(96_000_000.0, 0.6));
        assert!(clock.from_firmware);
    }

    #[test]
    fn a_firmware_clock_not_brought_up_to_date_gives_way_to_the_count() {
        // SystemCoreClock left at the 64 MHz the chip starts on.
        let clock = choose(Some(64_000_000), counted(240_300_000.0, 0.0));
        assert_eq!(
            clock,
            Clock {
                hz: 240_000_000,
                from_firmware: false,
                about: false
            }
        );
        // Awake, a count far less than the firmware's says it is wrong too.
        let clock = choose(Some(240_000_000), counted(120_100_000.0, 0.0));
        assert_eq!(clock.hz, 120_000_000);
    }

    #[test]
    fn without_the_firmware_s_the_count_is_taken() {
        // As an STM32H5 counts, asleep 60% of the time.
        let clock = choose(None, counted(239_985_000.0, 0.6));
        assert_eq!(clock.hz, 240_000_000);
        assert!(!clock.from_firmware && !clock.about);
        // Not near a whole MHz, and the core slept: it may have stopped counting.
        let clock = choose(None, counted(96_420_000.0, 0.6));
        assert_eq!(clock.hz, 96_000_000);
        assert!(clock.about);
    }

    #[test]
    fn counts_round_as_clocks_are_set() {
        assert_eq!(round(167_700_000.0), 168_000_000);
        assert_eq!(round(2_097_152.0), 2_100_000);
        assert_eq!(round(4_000_000.0), 4_000_000);
    }
}
