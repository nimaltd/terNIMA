//! Lists the variables in an ELF file that the Watch view can read: each
//! one's address, kind and type, and how many there are.
//! Run with `cargo run -p ternima-core --example variables -- <file.elf> [part of a name]`.

use ternima_core::elf;

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        return println!("Usage: variables <file.elf> [part of a name]");
    };
    let filter = args.next().unwrap_or_default().to_lowercase();
    let data = match std::fs::read(&path) {
        Ok(data) => data,
        Err(error) => return println!("Could not read {path}: {error}"),
    };
    let found = match elf::variables(&data) {
        Ok(found) => found,
        Err(problem) => return println!("{problem}"),
    };
    let shown: Vec<_> = found
        .iter()
        .filter(|variable| variable.name.to_lowercase().contains(&filter))
        .collect();
    for variable in &shown {
        println!(
            "{:#010x}  {:<5} {:<16} {}",
            variable.address,
            format!("{:?}", variable.kind).to_lowercase(),
            variable.type_name,
            variable.name
        );
    }
    println!("{} of {} variables", shown.len(), found.len());
}
