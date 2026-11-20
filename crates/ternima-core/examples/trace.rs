//! Traces which task runs, and the interrupts, over SWO for a second, as an
//! SWO tab's timeline does: every switch and every exception, timed by the
//! chip. Run with
//! `cargo run -p ternima-core --example trace -- <file.elf> <chip> <clock_hz> [baud] [no-interrupts]`.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use probe_rs::architecture::arm::SwoConfig;
use probe_rs::architecture::arm::component::TraceSink;
use probe_rs::probe::list::Lister;
use probe_rs::{MemoryInterface, Permissions};
use ternima_core::awake::Awake;
use ternima_core::rtos::Layout;
use ternima_core::swo::trace::{Event, Tracer, trace_exceptions, trace_writes};
use ternima_core::swo::{ItmDecoder, ItmPacket};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [elf, chip, clock, rest @ ..] = args.as_slice() else {
        return println!("Usage: trace <file.elf> <chip> <clock_hz> [baud] [no-interrupts]");
    };
    let Ok(clock) = clock.parse::<u32>() else {
        return println!("The clock is a number of Hz, as 240000000.");
    };
    let baud = rest
        .first()
        .and_then(|baud| baud.parse().ok())
        .unwrap_or(2_000_000);
    let interrupts = !rest.iter().any(|word| word == "no-interrupts");
    let data = match std::fs::read(elf) {
        Ok(data) => data,
        Err(error) => return println!("Could not read {elf}: {error}"),
    };
    let found = match Layout::find(&data) {
        Ok(Some(found)) => found,
        Ok(None) => return println!("No RTOS terNIMA reads in {elf}."),
        Err(problem) => return println!("{problem}"),
    };
    let (running, _) = found.layout.running_at();

    let probes = Lister::new().list_all();
    let Some(info) = probes.first() else {
        return println!("No debug probe found.");
    };
    let probe = match info.open() {
        Ok(probe) => probe,
        Err(error) => return println!("Could not open {}: {error}", info.identifier),
    };
    let mut session = match probe.attach(chip.as_str(), Permissions::default()) {
        Ok(session) => session,
        Err(error) => return println!("Could not attach to {chip}: {error}"),
    };
    if let Ok(mut core) = session.core(0) {
        Awake::new(chip).check(&mut core);
    }
    let swo = SwoConfig::new(clock)
        .set_baud(baud)
        .set_mode_uart()
        .set_continuous_formatting(false);
    if let Err(error) = session.setup_tracing(0, TraceSink::Swo(swo)) {
        return println!("Could not turn SWO on: {error}");
    }
    {
        let Ok(mut core) = session.core(0) else {
            return println!("Could not reach the core.");
        };
        // The STM32H5 has its trace pin's switch where probe-rs does not set it.
        if chip.to_uppercase().starts_with("STM32H5")
            && let Ok(cr) = core.read_word_32(0x4402_4004)
        {
            let _ = core.write_word_32(0x4402_4004, cr | 0x30);
        }
        if let Err(error) = trace_exceptions(&mut core, interrupts) {
            return println!("Could not set the exception trace: {error}");
        }
        match trace_writes(&mut core, Some(running)) {
            Ok(true) => {}
            Ok(false) => return println!("This core cannot trace writes."),
            Err(error) => return println!("Could not set the DWT: {error}"),
        }
    }

    let mut decoder = ItmDecoder::new();
    let mut tracer = Tracer::new(clock);
    tracer.trace_switches(true);
    let (mut bytes, mut overflows) = (0, 0);
    let mut switches = Vec::new();
    let mut entered: HashMap<u16, u32> = HashMap::new();
    let end = Instant::now() + Duration::from_secs(1);
    while Instant::now() < end {
        let raw = match session.read_trace_data() {
            Ok(raw) => raw,
            Err(error) => return println!("Could not read SWO: {error}"),
        };
        bytes += raw.len();
        for packet in decoder.feed(&raw) {
            if packet == ItmPacket::Overflow {
                overflows += 1;
            }
            tracer.feed(&packet);
        }
        if let Some(traced) = tracer.take_if_due() {
            if let Some(running) = traced.running {
                switches.extend(running.switches);
            }
            for interrupt in traced.interrupts {
                if interrupt.event == Event::Entered {
                    *entered.entry(interrupt.number).or_default() += 1;
                }
            }
        }
    }
    let _ = trace_writes(&mut session.core(0).unwrap(), None);

    println!(
        "{bytes} bytes over SWO, {} switches, {overflows} overflows",
        switches.len()
    );
    let mut numbers: Vec<_> = entered.into_iter().collect();
    numbers.sort();
    for (number, count) in numbers {
        println!("exception {number:>3} entered {count} times");
    }
    let apart = switches
        .windows(2)
        .filter(|pair| pair[0].1 != 0 && pair[1].1 != 0)
        .map(|pair| pair[1].0 - pair[0].0)
        .fold(f64::INFINITY, f64::min);
    if apart.is_finite() {
        println!("closest switches: {:.2} us apart", apart * 1000.0);
    }
    let start = switches.first().map_or(0.0, |&(at, _)| at);
    for &(at, task) in switches.iter().skip(20).take(12) {
        println!("  {:>12.3} us  {task:#010x}", (at - start) * 1000.0);
    }
}
