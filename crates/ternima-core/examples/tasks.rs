//! Finds the RTOS in an ELF file, and with a chip named, reads its tasks a
//! few times through the only probe plugged in, as the chip runs; then
//! samples which one runs for a second, as the timeline does.
//! Run with `cargo run -p ternima-core --example tasks -- <file.elf> [chip]`.

use std::collections::HashMap;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use probe_rs::probe::list::Lister;
use probe_rs::{MemoryInterface, Permissions};
use ternima_core::awake::Awake;
use ternima_core::rtos::{Layout, Reader, Sampler, Snapshot};

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        return println!("Usage: tasks <file.elf> [chip]");
    };
    let data = match std::fs::read(&path) {
        Ok(data) => data,
        Err(error) => return println!("Could not read {path}: {error}"),
    };
    let found = match Layout::find(&data) {
        Ok(Some(found)) => found,
        Ok(None) => return println!("No RTOS terNIMA reads in {path}."),
        Err(problem) => return println!("{problem}"),
    };
    println!("{:?}: {:#?}", found.layout.kernel(), found.layout);
    for missing in &found.missing {
        println!("missing: {missing}");
    }
    let Some(chip) = args.next() else {
        return;
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
    let mut core = match session.core(0) {
        Ok(core) => core,
        Err(error) => return println!("Could not reach the core: {error}"),
    };
    // As the app does: a sleeping STM32 reads right only with a DMA clock on.
    let mut awake = Awake::new(&chip);
    let layout = Arc::new(found.layout);
    let mut reader = Reader::default();
    reader.set(Some(layout.clone()), Duration::from_millis(500));
    let mut names = HashMap::new();
    for _ in 0..4 {
        awake.check(&mut core);
        if let Some(snapshot) =
            reader.read_if_due(|address, block| core.read(address, block).is_ok())
        {
            print(&snapshot);
            for task in &snapshot.tasks {
                names.insert(task.address, task.name.clone());
            }
        }
        thread::sleep(Duration::from_millis(600));
    }

    let mut sampler = Sampler::default();
    sampler.set(Some(&layout));
    let mut switches = Vec::new();
    let (mut samples, mut until) = (0, 0.0);
    let end = Instant::now() + Duration::from_secs(1);
    while Instant::now() < end {
        awake.check(&mut core);
        if let Some(seen) = sampler.sample(|address, block| core.read(address, block).is_ok()) {
            switches.extend(seen.switches);
            samples += seen.samples;
            until = seen.until;
        }
    }
    let mut ran: HashMap<u64, f64> = HashMap::new();
    for (index, &(at, task)) in switches.iter().enumerate() {
        let next = switches.get(index + 1).map_or(until, |&(next, _)| next);
        *ran.entry(task).or_default() += next - at;
    }
    let total: f64 = ran.values().sum();
    println!();
    println!(
        "{samples} samples in a second, {} switches seen",
        switches.len()
    );
    let mut shares: Vec<_> = ran.into_iter().collect();
    shares.sort_by(|a, b| b.1.total_cmp(&a.1));
    for (task, time) in shares {
        let name = names
            .get(&task)
            .cloned()
            .unwrap_or_else(|| format!("{task:#010x}"));
        println!(
            "{name:<16} {:5.1}% of the samples' time",
            time * 100.0 / total
        );
    }
}

fn print(snapshot: &Snapshot) {
    println!();
    if let Some(problem) = &snapshot.problem {
        println!("{problem}");
    }
    println!(
        "{:<16} {:<10} {:>5} {:>7} {:>7} {:>7} {:>6}",
        "name", "state", "prio", "stack", "used", "free", "cpu %"
    );
    let number =
        |value: Option<u64>| value.map_or_else(|| "-".to_string(), |value| value.to_string());
    for task in &snapshot.tasks {
        println!(
            "{:<16} {:<10} {:>5} {:>7} {:>7} {:>7} {:>6}",
            task.name,
            format!("{:?}", task.state),
            task.priority,
            number(task.stack_size),
            number(task.stack_used),
            number(task.stack_free),
            task.cpu
                .map_or_else(|| "-".to_string(), |cpu| format!("{cpu:.1}")),
        );
    }
    if let Some(heap) = &snapshot.heap {
        println!(
            "heap: {} of {} free, at least {:?}",
            heap.free, heap.size, heap.lowest_free
        );
    }
}
