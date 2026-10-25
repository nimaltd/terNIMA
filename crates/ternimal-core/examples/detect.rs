//! Finds out which chip is on the board through the only probe plugged in.
//! Run with `cargo run -p ternimal-core --example detect`.

fn main() {
    match ternimal_core::detect::detect(None, 0) {
        Ok(found) => {
            println!("found: {}", found.description);
            println!("chip: {:?}", found.chip);
            println!("candidates: {:?}", found.candidates);
        }
        Err(problem) => println!("{problem}"),
    }
}
