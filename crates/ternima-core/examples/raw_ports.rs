//! Prints what the serialport crate reports for every port, unchanged.
//! Run with `cargo run -p ternima-core --example raw_ports`.

fn main() {
    match serialport::available_ports() {
        Ok(ports) if ports.is_empty() => println!("No serial ports found."),
        Ok(ports) => {
            for port in ports {
                println!("{:#?}", port);
            }
        }
        Err(error) => println!("Could not list the ports: {error}"),
    }
}
