//! Keeps an STM32's memory readable through the probe while its core sleeps.
//!
//! In Sleep mode, when the firmware waits in WFI as an RTOS's idle thread
//! does, an STM32 stops the clock of its bus matrix if no other master in it
//! has a clock. The probe then reads zeros from RAM and the peripherals with
//! no error said: watched variables read 0, an RTOS's tasks vanish, RTT
//! finds nothing. With the clock of a DMA controller on, the matrix keeps
//! running. Zephyr turns it on for RTT the same way, in
//! `soc/st/stm32/common/soc_config.c`.
//!
//! So on an STM32, the first DMA controller's clock is checked once a second
//! and turned on when it is off, as after a reset. The core is halted for
//! the moment that takes, for the register to read right and for the
//! firmware not to change it at the same time.

use std::time::{Duration, Instant};

use probe_rs::{Core, MemoryInterface};

/// The RCC register that turns on the clock of a family's first DMA
/// controller, and its bit there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DmaClock {
    pub register: u64,
    pub bit: u32,
}

const fn clock(register: u64, bit: u32) -> DmaClock {
    DmaClock { register, bit }
}

/// By the start of the chip's name, as probe-rs has it. Read from ST's CMSIS
/// headers, compiled for every chip of each family: RCC's base address (the
/// non-secure one where there is TrustZone), the offset of AHBENR or
/// AHB1ENR in it, and DMA1EN or GPDMA1EN. Longer starts come first. The H7
/// is left out: probe-rs keeps its debug clocks on in Sleep through DBGMCU.
const FAMILIES: [(&str, Option<DmaClock>); 21] = [
    // The STM32WB0 is another chip than the WB, with no DMA1 of this kind.
    ("STM32WB0", None),
    ("STM32WBA", Some(clock(0x4602_0C88, 0))),
    ("STM32WB", Some(clock(0x5800_0048, 0))),
    ("STM32WL", Some(clock(0x5800_0048, 0))),
    ("STM32C0", Some(clock(0x4002_1038, 0))),
    ("STM32F0", Some(clock(0x4002_1014, 0))),
    ("STM32F1", Some(clock(0x4002_1014, 0))),
    ("STM32F2", Some(clock(0x4002_3830, 21))),
    ("STM32F3", Some(clock(0x4002_1014, 0))),
    ("STM32F4", Some(clock(0x4002_3830, 21))),
    ("STM32F7", Some(clock(0x4002_3830, 21))),
    ("STM32G0", Some(clock(0x4002_1038, 0))),
    ("STM32G4", Some(clock(0x4002_1048, 0))),
    ("STM32H5", Some(clock(0x4402_0C88, 0))),
    ("STM32L0", Some(clock(0x4002_1030, 0))),
    ("STM32L1", Some(clock(0x4002_381C, 24))),
    ("STM32L4", Some(clock(0x4002_1048, 0))),
    ("STM32L5", Some(clock(0x4002_1048, 0))),
    ("STM32U0", Some(clock(0x4002_1048, 0))),
    ("STM32U5", Some(clock(0x4602_0C88, 0))),
    ("STM32H7", None),
];

/// The DMA clock to keep on for a chip, as probe-rs names it.
pub fn dma_clock(chip: &str) -> Option<DmaClock> {
    let chip = chip.trim().to_uppercase();
    FAMILIES
        .iter()
        .find(|(start, _)| chip.starts_with(start))
        .and_then(|&(_, clock)| clock)
}

/// How often the clock is checked.
const EVERY: Duration = Duration::from_secs(1);
/// How soon after a reset, once the firmware has set its clocks up.
const AFTER_RESET: Duration = Duration::from_millis(200);
/// The most times the clock is turned on in a minute: if something keeps
/// turning it off, the chip is not halted over and over.
const TIMES_MAX: usize = 3;
const TIMES_WINDOW: Duration = Duration::from_secs(60);

/// Keeps a chip's DMA clock on while a probe session reads its memory.
#[derive(Debug)]
pub struct Awake {
    clock: Option<DmaClock>,
    next: Instant,
    /// When it was last turned on.
    turned_on: Vec<Instant>,
}

impl Awake {
    pub fn new(chip: &str) -> Self {
        Self {
            clock: dma_clock(chip),
            next: Instant::now(),
            turned_on: Vec::new(),
        }
    }

    /// Whether it is time to check the clock: asking probe-rs for the core
    /// takes most of a millisecond, so a session asks only then.
    pub fn due(&self) -> bool {
        self.clock.is_some() && Instant::now() >= self.next
    }

    /// Checks the clock when it is time to, and turns it on if it is off.
    /// Nothing is said when it cannot be: memory is then read as before.
    pub fn check(&mut self, core: &mut Core) {
        let Some(clock) = self.clock else {
            return;
        };
        let now = Instant::now();
        if now < self.next {
            return;
        }
        self.next = now + EVERY;
        let mut word = [0];
        let read = core
            .read_32(clock.register, &mut word)
            .ok()
            .map(|()| word[0]);
        if is_on(read, clock) {
            return;
        }
        self.turned_on
            .retain(|&at| now.duration_since(at) < TIMES_WINDOW);
        if self.turned_on.len() >= TIMES_MAX {
            return;
        }
        match turn_on(core, clock) {
            Ok(Turned::On) => self.turned_on.push(now),
            // It was on: what was read running was not the register, so the
            // clock does not keep this chip readable. It is left alone.
            Ok(Turned::WasOn) => self.clock = None,
            Err(_) => {}
        }
    }

    /// After the chip was reset, which turns the clock off: checked again soon.
    pub fn after_reset(&mut self) {
        self.next = Instant::now() + AFTER_RESET;
    }
}

/// Whether a read of the RCC register, made as the chip runs, shows the
/// clock on. A register of only zeros or only ones is no register read: on
/// a sleeping chip without the clock, reads give that.
fn is_on(read: Option<u32>, clock: DmaClock) -> bool {
    match read {
        Some(0 | u32::MAX) | None => false,
        Some(value) => value & (1 << clock.bit) != 0,
    }
}

enum Turned {
    On,
    WasOn,
}

/// Turns the clock on with the core halted, and lets it run on, unless it
/// was halted already, as at a breakpoint.
fn turn_on(core: &mut Core, clock: DmaClock) -> Result<Turned, probe_rs::Error> {
    let halted = core.core_halted()?;
    if !halted {
        core.halt(Duration::from_millis(100))?;
    }
    let changed: Result<Turned, probe_rs::Error> = (|| {
        let value = core.read_word_32(clock.register)?;
        if value & (1 << clock.bit) != 0 {
            return Ok(Turned::WasOn);
        }
        core.write_word_32(clock.register, value | (1 << clock.bit))?;
        Ok(Turned::On)
    })();
    if !halted {
        core.run()?;
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn families_are_found_by_the_start_of_the_name() {
        assert_eq!(dma_clock("STM32H503RB"), Some(clock(0x4402_0C88, 0)));
        assert_eq!(dma_clock(" stm32f407vgtx "), Some(clock(0x4002_3830, 21)));
        assert_eq!(dma_clock("STM32WBA52CGUx"), Some(clock(0x4602_0C88, 0)));
        assert_eq!(dma_clock("STM32WB55RGVx"), Some(clock(0x5800_0048, 0)));
        assert_eq!(dma_clock("STM32L152RE"), Some(clock(0x4002_381C, 24)));
        assert_eq!(dma_clock("STM32WB05KZVx"), None);
        assert_eq!(dma_clock("STM32H743ZITx"), None);
        assert_eq!(dma_clock("nRF54L15"), None);
    }

    #[test]
    fn longer_starts_come_before_shorter_ones() {
        for (index, (start, _)) in FAMILIES.iter().enumerate() {
            for (longer, _) in &FAMILIES[index + 1..] {
                assert!(
                    !longer.starts_with(start) || longer == start,
                    "{longer} must come before {start}"
                );
            }
        }
    }

    #[test]
    fn only_a_register_read_right_can_show_the_clock_on() {
        let h5 = clock(0x4402_0C88, 0);
        assert!(is_on(Some(0x9000_0101), h5));
        assert!(!is_on(Some(0x9000_0100), h5));
        // A sleeping chip reads zeros, or ones.
        assert!(!is_on(Some(0), h5));
        assert!(!is_on(Some(u32::MAX), h5));
        assert!(!is_on(None, h5));
    }
}
