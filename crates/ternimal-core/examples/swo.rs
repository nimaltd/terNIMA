//! Runs an SWO session for a few seconds and prints what it reports.
//! Run with `cargo run -p ternimal-core --example swo -- <chip> <clock Hz> [baud] [port] [seconds]`.

use std::sync::Arc;
use std::time::Duration;

use ternimal_core::swo::{Event, ProbeRs, Session, SwoConfig};

fn main() {
    let mut args = std::env::args().skip(1);
    let chip = args.next().unwrap_or_default();
    let mut number = |default: u32| {
        args.next()
            .and_then(|text| text.parse().ok())
            .unwrap_or(default)
    };
    let config = SwoConfig {
        chip,
        clock_hz: number(0),
        baud: number(2_000_000),
        port: number(0) as u8,
        ..SwoConfig::default()
    };
    let seconds = number(4);

    let session = Session::start(Arc::new(ProbeRs), config, |event| match event {
        Event::Data(bytes) => print!("{}", String::from_utf8_lossy(&bytes)),
        other => println!("{other:?}"),
    });
    std::thread::sleep(Duration::from_secs(seconds.into()));
    drop(session);
    println!("closed");
}
