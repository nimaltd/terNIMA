//! Runs an RTT session for a few seconds and prints what it reports.
//! Run with `cargo run -p ternima-core --example rtt -- <chip> [seconds]`.

use std::sync::Arc;
use std::time::Duration;

use ternima_core::rtt::{Event, ProbeRs, RttConfig, Session};

fn main() {
    let mut args = std::env::args().skip(1);
    let chip = args.next().unwrap_or_default();
    let seconds = args.next().and_then(|text| text.parse().ok()).unwrap_or(4);
    let config = RttConfig {
        chip,
        ..RttConfig::default()
    };

    let session = Session::start(Arc::new(ProbeRs), config, |event| match event {
        Event::Data(bytes) => print!("{}", String::from_utf8_lossy(&bytes)),
        other => println!("{other:?}"),
    });
    std::thread::sleep(Duration::from_secs(seconds));
    drop(session);
    println!("closed");
}
