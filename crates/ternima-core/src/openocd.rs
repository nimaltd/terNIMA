//! Watched variables read through OpenOCD's Tcl server, while OpenOCD has the
//! probe for a debugger: the chip runs on, and the debugger keeps working.

use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::net;
use crate::watch::{Watched, Watcher};

/// The port OpenOCD's Tcl server listens on, unless told otherwise.
pub const TCL_PORT: u16 = 6666;
/// Ends each command and each answer.
const END: u8 = 0x1a;
/// The longest an answer can take.
const ANSWER_WAIT: Duration = Duration::from_secs(2);
/// Between two tries to reach the server.
const RETRY: Duration = Duration::from_secs(1);

/// What a session tells the app.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// Whether the server can be reached: why not, when it cannot.
    Problem(Option<String>),
    /// The watched variables' values, in their order: `None` for one that
    /// could not be read, or is not in the firmware.
    Watch(Vec<Option<f64>>),
}

enum Command {
    Watch(Vec<Option<Watched>>, Duration),
    Close,
}

/// Reads watched variables through OpenOCD's Tcl server, on its own thread.
/// Dropping it ends the thread.
pub struct Session {
    commands: Sender<Command>,
    thread: Option<JoinHandle<()>>,
}

impl Session {
    /// Starts the thread. It reaches the server once there is something to
    /// read, and again whenever it was lost.
    pub fn start(host: &str, port: u16, on_event: impl FnMut(Event) + Send + 'static) -> Self {
        let (commands, receiver) = mpsc::channel();
        let worker = Worker {
            host: host.trim().to_string(),
            port,
            on_event: Box::new(on_event),
            commands: receiver,
            watcher: Watcher::default(),
            told: None,
        };
        let thread = thread::Builder::new()
            .name("ternima-openocd".into())
            .spawn(move || worker.run())
            .expect("the OS could not start a thread");
        Self {
            commands,
            thread: Some(thread),
        }
    }

    /// Reads these variables every `every`; `None` stands for one not in the
    /// firmware. An empty list stops it.
    pub fn watch(&self, watch: Vec<Option<Watched>>, every: Duration) {
        let _ = self.commands.send(Command::Watch(watch, every));
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Close);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Worker {
    host: String,
    port: u16,
    on_event: Box<dyn FnMut(Event) + Send>,
    commands: Receiver<Command>,
    watcher: Watcher,
    /// The problem last told to the app, so each change is told once.
    told: Option<Option<String>>,
}

impl Worker {
    fn run(mut self) {
        loop {
            // Nothing to read: no need for the server yet.
            if self.watcher.next_in().is_none() {
                match self.commands.recv() {
                    Ok(Command::Watch(watch, every)) => self.watcher.set(watch, every),
                    Ok(Command::Close) | Err(_) => return,
                }
                continue;
            }
            match self.connect() {
                Ok(stream) => {
                    self.tell(None);
                    match self.serve(stream) {
                        Some(problem) => self.tell(Some(problem)),
                        None => return,
                    }
                }
                Err(problem) => self.tell(Some(problem)),
            }
            if !self.wait(RETRY) {
                return;
            }
        }
    }

    fn connect(&self) -> Result<TcpStream, String> {
        let at = format!("{}:{}", self.host, self.port);
        let cannot = |error: io::Error| {
            format!(
                "OpenOCD's Tcl server at {at} does not answer: {error}. Is OpenOCD running, \
                 with its Tcl port at {}?",
                self.port
            )
        };
        let mut last = io::Error::new(io::ErrorKind::NotFound, "the host has no address");
        for address in net::addresses(&self.host, self.port).map_err(cannot)? {
            match TcpStream::connect_timeout(&address, ANSWER_WAIT) {
                Ok(stream) => {
                    stream.set_read_timeout(Some(ANSWER_WAIT)).map_err(cannot)?;
                    let _ = stream.set_nodelay(true);
                    return Ok(stream);
                }
                Err(error) => last = error,
            }
        }
        Err(cannot(last))
    }

    /// Reads the variables when it is their time, until the server is lost,
    /// which it says, or the session ends, which gives `None`.
    fn serve(&mut self, mut stream: TcpStream) -> Option<String> {
        let mut answers = Answers::default();
        loop {
            let left = self.watcher.next_in().unwrap_or(RETRY);
            match self.commands.recv_timeout(left) {
                Err(RecvTimeoutError::Timeout) => {}
                Ok(Command::Watch(watch, every)) => self.watcher.set(watch, every),
                Ok(Command::Close) | Err(RecvTimeoutError::Disconnected) => return None,
            }
            let mut lost = None;
            let read = |address, block: &mut [u8]| {
                if lost.is_some() {
                    return false;
                }
                match read_memory(&mut stream, &mut answers, address, block) {
                    Ok(read) => read,
                    Err(error) => {
                        lost = Some(error);
                        false
                    }
                }
            };
            if let Some(values) = self.watcher.read_if_due(read) {
                if let Some(error) = lost {
                    return Some(format!("Lost OpenOCD's Tcl server: {error}"));
                }
                (self.on_event)(Event::Watch(values));
            }
        }
    }

    /// Waits while the server cannot be reached. Returns false when the session should end.
    fn wait(&mut self, time: Duration) -> bool {
        match self.commands.recv_timeout(time) {
            Err(RecvTimeoutError::Timeout) => true,
            Ok(Command::Watch(watch, every)) => {
                self.watcher.set(watch, every);
                true
            }
            Ok(Command::Close) | Err(RecvTimeoutError::Disconnected) => false,
        }
    }

    fn tell(&mut self, problem: Option<String>) {
        if self.told.as_ref() != Some(&problem) {
            self.told = Some(problem.clone());
            (self.on_event)(Event::Problem(problem));
        }
    }
}

/// What came from the server and has not been taken as an answer yet.
#[derive(Default)]
struct Answers(Vec<u8>);

impl Answers {
    /// The next answer, without its end byte.
    fn next(&mut self, stream: &mut TcpStream) -> io::Result<String> {
        loop {
            if let Some(at) = self.0.iter().position(|&byte| byte == END) {
                let answer = String::from_utf8_lossy(&self.0[..at]).into_owned();
                self.0.drain(..=at);
                return Ok(answer);
            }
            let mut buf = [0; 4096];
            match stream.read(&mut buf)? {
                0 => return Err(io::ErrorKind::UnexpectedEof.into()),
                count => self.0.extend_from_slice(&buf[..count]),
            }
        }
    }
}

/// Asks for `block.len()` bytes from `address` while the chip runs:
/// `read_memory` answers with each one's value in hex. False when the chip
/// could not give them, as at an address with no memory; an error when the
/// server is lost.
fn read_memory(
    stream: &mut TcpStream,
    answers: &mut Answers,
    address: u64,
    block: &mut [u8],
) -> io::Result<bool> {
    let mut command = format!("read_memory {address:#x} 8 {}", block.len()).into_bytes();
    command.push(END);
    stream.write_all(&command)?;
    let answer = answers.next(stream)?;
    let bytes: Option<Vec<u8>> = answer
        .split_whitespace()
        .map(|value| u8::from_str_radix(value.trim_start_matches("0x"), 16).ok())
        .collect();
    match bytes {
        Some(bytes) if bytes.len() == block.len() => {
            block.copy_from_slice(&bytes);
            Ok(true)
        }
        _ => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;
    use std::time::Instant;

    use super::*;
    use crate::elf::Kind;

    /// A pretend OpenOCD on this computer only, for one connection: RAM from
    /// 0x2000_0000 with a u32 of 1000 and an f32 of 21.5, read as OpenOCD
    /// answers `read_memory`.
    fn openocd() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut ram = [0u8; 16];
            ram[0..4].copy_from_slice(&1000u32.to_le_bytes());
            ram[4..8].copy_from_slice(&21.5f32.to_le_bytes());
            let mut pending = Vec::new();
            let mut buf = [0; 1024];
            while let Ok(count) = stream.read(&mut buf) {
                if count == 0 {
                    return;
                }
                pending.extend_from_slice(&buf[..count]);
                while let Some(at) = pending.iter().position(|&byte| byte == END) {
                    let command = String::from_utf8_lossy(&pending[..at]).into_owned();
                    pending.drain(..=at);
                    let words: Vec<&str> = command.split_whitespace().collect();
                    let address =
                        u64::from_str_radix(words[1].trim_start_matches("0x"), 16).unwrap();
                    let count: usize = words[3].parse().unwrap();
                    let mut answer = match address.checked_sub(0x2000_0000) {
                        Some(at) if at as usize + count <= ram.len() => ram
                            [at as usize..at as usize + count]
                            .iter()
                            .map(|byte| format!("{byte:#x}"))
                            .collect::<Vec<_>>()
                            .join(" "),
                        _ => "Failed to read memory at 0x30000000".into(),
                    }
                    .into_bytes();
                    answer.push(END);
                    if stream.write_all(&answer).is_err() {
                        return;
                    }
                }
            }
        });
        port
    }

    fn expect(events: &Receiver<Event>, wanted: impl Fn(&Event) -> bool) -> Event {
        let until = Instant::now() + Duration::from_secs(5);
        while let Some(left) = until.checked_duration_since(Instant::now()) {
            match events.recv_timeout(left) {
                Ok(event) if wanted(&event) => return event,
                Ok(_) => {}
                Err(_) => break,
            }
        }
        panic!("not told within 5 s");
    }

    fn start(port: u16) -> (Session, Receiver<Event>) {
        let (sender, events) = mpsc::channel();
        let session = Session::start("127.0.0.1", port, move |event| {
            let _ = sender.send(event);
        });
        (session, events)
    }

    #[test]
    fn variables_are_read_through_the_tcl_server() {
        let (session, events) = start(openocd());
        session.watch(
            vec![
                Some(Watched {
                    address: 0x2000_0004,
                    kind: Kind::F32,
                }),
                None,
                Some(Watched {
                    address: 0x2000_0000,
                    kind: Kind::U32,
                }),
                Some(Watched {
                    address: 0x3000_0000,
                    kind: Kind::U32,
                }),
            ],
            Duration::from_millis(10),
        );
        assert_eq!(
            expect(&events, |event| matches!(event, Event::Problem(_))),
            Event::Problem(None)
        );
        match expect(&events, |event| matches!(event, Event::Watch(_))) {
            Event::Watch(values) => assert_eq!(values, [Some(21.5), None, Some(1000.0), None]),
            _ => unreachable!(),
        }
    }

    #[test]
    fn a_server_that_does_not_answer_is_told() {
        // A port nothing listens on: taken, then let go.
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let (session, events) = start(port);
        session.watch(
            vec![Some(Watched {
                address: 0x2000_0000,
                kind: Kind::U32,
            })],
            Duration::from_millis(10),
        );
        match expect(&events, |event| matches!(event, Event::Problem(Some(_)))) {
            Event::Problem(Some(problem)) => {
                assert!(problem.contains("does not answer"), "{problem}")
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn nothing_is_reached_until_something_is_watched() {
        let (_session, events) = start(1);
        assert!(events.recv_timeout(Duration::from_millis(100)).is_err());
    }
}
