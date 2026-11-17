//! Finds out which chip is on the board through the only probe plugged in.
//! Run with `cargo run -p ternima-core --example detect`.

fn main() {
    match ternima_core::detect::detect(None, 0) {
        Ok(found) => {
            println!("found: {}", found.description);
            println!("chip: {:?}", found.chip);
            println!("candidates: {:?}", found.candidates);
            println!("under reset: {}", found.under_reset);
        }
        Err(problem) => println!("{problem}"),
    }
}
