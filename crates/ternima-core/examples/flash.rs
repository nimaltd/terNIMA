//! Flashes an ELF file through the only probe plugged in, then resets the chip
//! so the new firmware runs.
//! Run with `cargo run -p ternima-core --example flash -- <chip> <file.elf>`.

use std::time::Instant;

use probe_rs::Permissions;
use probe_rs::flashing::{ElfLoader, ElfOptions, download_file};
use probe_rs::probe::list::Lister;

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(chip), Some(path)) = (args.next(), args.next()) else {
        return println!("Usage: flash <chip> <file.elf>");
    };
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

    let started = Instant::now();
    if let Err(error) = download_file(&mut session, &path, ElfLoader(ElfOptions::default())) {
        return println!("Could not flash {path}: {error}");
    }
    println!("Flashed {path} in {:.1?}", started.elapsed());

    match session.core(0).and_then(|mut core| core.reset()) {
        Ok(()) => println!("Reset: the new firmware runs."),
        Err(error) => println!("Flashed, but the reset failed: {error}"),
    }
}
