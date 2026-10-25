//! Lists debug probes, attaches to the first one without halting or resetting
//! the target, and looks for an RTT control block.
//! Run with `cargo run -p ternima-core --example probes [chip]`.

use std::time::{Duration, Instant};

use probe_rs::probe::list::Lister;
use probe_rs::rtt::Rtt;
use probe_rs::{MemoryInterface, Permissions, config::TargetSelector};

fn main() {
    let probes = Lister::new().list_all();
    if probes.is_empty() {
        println!("No debug probes found.");
        return;
    }
    for probe in &probes {
        println!(
            "probe: {} {:04x}:{:04x} serial {:?}",
            probe.identifier, probe.vendor_id, probe.product_id, probe.serial_number
        );
    }

    let chip = std::env::args().nth(1);
    let target = match &chip {
        Some(name) => TargetSelector::Unspecified(name.clone()),
        None => TargetSelector::Auto,
    };
    let probe = match probes[0].open() {
        Ok(probe) => probe,
        Err(error) => return println!("Could not open the probe: {error}"),
    };
    let mut session = match probe.attach(target, Permissions::default()) {
        Ok(session) => session,
        Err(error) => return println!("Could not attach: {error}\n  details: {error:?}"),
    };
    println!("target: {}", session.target().name);
    for region in session.target().memory_map.iter() {
        println!("  memory: {region:x?}");
    }

    let mut core = match session.core(0) {
        Ok(core) => core,
        Err(error) => return println!("No core 0: {error}"),
    };
    println!("core halted: {:?}", core.core_halted());

    // CPUID, then where STM32 families keep DBGMCU_IDCODE: F0/G0/C0/L0, most
    // others, H7, H5/U5 (debug and secure addresses), then the H5 flash size
    // and package words.
    for address in [
        0xE000_ED00u64,
        0x4001_5800,
        0xE004_2000,
        0x5C00_1000,
        0xE004_4000,
        0x4402_4000,
        0x08FF_F80C,
        0x08FF_F80E,
    ] {
        match core.read_word_32(address) {
            Ok(value) => println!(
                "  IDCODE at {address:#x}: {value:#010x} (DEV_ID {:#05x}, REV_ID {:#06x})",
                value & 0xfff,
                value >> 16
            ),
            Err(error) => println!("  IDCODE at {address:#x}: {error}"),
        }
    }

    let started = Instant::now();
    let mut rtt = match Rtt::attach(&mut core) {
        Ok(rtt) => rtt,
        Err(error) => {
            return println!(
                "No RTT control block ({error}), after {:?}",
                started.elapsed()
            );
        }
    };
    println!(
        "RTT control block at {:#x}, found in {:?}",
        rtt.ptr(),
        started.elapsed()
    );
    for channel in rtt.up_channels() {
        println!(
            "  up {}: {:?} {} bytes",
            channel.number(),
            channel.name(),
            channel.buffer_size()
        );
    }
    for channel in rtt.down_channels() {
        println!(
            "  down {}: {:?} {} bytes",
            channel.number(),
            channel.name(),
            channel.buffer_size()
        );
    }

    // Read up channel 0 for two seconds.
    let mut buf = [0u8; 1024];
    let mut received = Vec::new();
    let until = Instant::now() + Duration::from_secs(2);
    while Instant::now() < until {
        if let Some(channel) = rtt.up_channel(0) {
            match channel.read(&mut core, &mut buf) {
                Ok(count) => received.extend_from_slice(&buf[..count]),
                Err(error) => return println!("Read failed: {error}"),
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    println!(
        "{} bytes in 2 s: {:?}",
        received.len(),
        String::from_utf8_lossy(&received)
    );
}
