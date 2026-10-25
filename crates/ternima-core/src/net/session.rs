//! One TCP session: a connection kept open on its own thread, and made again
//! when the other end starts later or goes away.

use std::io::{self, ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde::Serialize;

use super::config::{Protocol, TcpConfig};
use super::telnet::Telnet;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum NetState {
    /// Not connected. `problem` says why when something is wrong, such as a
    /// host name that cannot be found; without it, nothing answers yet.
    Waiting { problem: Option<String> },
    /// Connected to `address`, the host and port as set, such as "localhost:19021".
    Connected { address: String, protocol: Protocol },
}

/// What a session tells the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// Bytes that arrived, in the order they came, without Telnet's option talk.
    Data(Vec<u8>),
    State(NetState),
}

enum Command {
    Write(Vec<u8>),
    Reconfigure(TcpConfig),
    Close,
}

/// How long things take. Tests make the times shorter.
#[derive(Clone, Copy)]
struct Timing {
    /// Between two tries to connect.
    retry: Duration,
    /// The longest one try to connect to one address takes.
    connect: Duration,
    /// The longest a read waits for data, so what is typed goes out quickly.
    poll: Duration,
    /// The longest received bytes wait before they are passed on, so the
    /// screen updates about 60 times a second.
    batch: Duration,
}

const TIMING: Timing = Timing {
    retry: Duration::from_secs(1),
    connect: Duration::from_secs(3),
    poll: Duration::from_millis(10),
    batch: Duration::from_millis(16),
};

/// The most bytes passed on at once.
const BATCH_MAX: usize = 64 * 1024;

/// A TCP session. Dropping it closes the connection.
pub struct Session {
    commands: Sender<Command>,
    thread: Option<JoinHandle<()>>,
}

impl Session {
    /// Starts connecting and keeps the connection until the session is
    /// dropped. Everything that happens comes back through `on_event`, which
    /// runs on the session's own thread.
    pub fn start(config: TcpConfig, on_event: impl FnMut(Event) + Send + 'static) -> Self {
        Self::start_with(config, Box::new(on_event), TIMING)
    }

    fn start_with(
        config: TcpConfig,
        on_event: Box<dyn FnMut(Event) + Send>,
        timing: Timing,
    ) -> Self {
        let (commands, receiver) = mpsc::channel();
        let worker = Worker {
            config,
            on_event,
            commands: receiver,
            timing,
            state: None,
        };
        let thread = thread::Builder::new()
            .name("ternima-tcp".into())
            .spawn(move || worker.run())
            .expect("the OS could not start a thread");
        Self {
            commands,
            thread: Some(thread),
        }
    }

    /// Sends bytes to the other end. They are dropped while not connected.
    pub fn write(&self, data: Vec<u8>) {
        self.send(Command::Write(data));
    }

    /// Connects again with new settings.
    pub fn reconfigure(&self, config: TcpConfig) {
        self.send(Command::Reconfigure(config));
    }

    fn send(&self, command: Command) {
        // The thread only ends when the session is dropped, so this cannot fail before then.
        let _ = self.commands.send(command);
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.send(Command::Close);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Why serving a connection ended.
enum Ended {
    /// The app closed the session.
    Closed,
    /// The other end closed the connection, or it broke.
    Lost,
    /// New settings: connect again.
    Reconnect,
}

struct Worker {
    config: TcpConfig,
    on_event: Box<dyn FnMut(Event) + Send>,
    commands: Receiver<Command>,
    timing: Timing,
    /// The state last told to the app, so each change is told once.
    state: Option<NetState>,
}

impl Worker {
    fn run(mut self) {
        loop {
            match self.connect() {
                Ok(stream) => match self.serve(stream) {
                    Ended::Closed => return,
                    Ended::Reconnect => continue,
                    // A server that closes at once is not asked again straight away.
                    Ended::Lost => {
                        if !self.wait(self.timing.retry) {
                            return;
                        }
                    }
                },
                Err(problem) => {
                    self.set_state(NetState::Waiting { problem });
                    if !self.wait(self.timing.retry) {
                        return;
                    }
                }
            }
        }
    }

    /// Connects, or says why not: `None` when nothing answers yet.
    fn connect(&mut self) -> Result<TcpStream, Option<String>> {
        if let Some(problem) = self.config.problem() {
            return Err(Some(problem));
        }
        let host = self
            .config
            .host
            .trim()
            .trim_start_matches('[')
            .trim_end_matches(']');
        let mut addresses: Vec<SocketAddr> = (host, self.config.port)
            .to_socket_addrs()
            .map_err(|error| Some(format!("Cannot find the host {host}: {error}")))?
            .collect();
        // IPv4 first: J-Link, OpenOCD and most devices listen there, and on
        // Windows a refused connection to localhost's IPv6 address takes seconds.
        addresses.sort_by_key(SocketAddr::is_ipv6);

        let mut last = None;
        for address in addresses {
            match TcpStream::connect_timeout(&address, self.timing.connect) {
                Ok(stream) => return self.ready(stream).map_err(|error| Some(error.to_string())),
                Err(error) => last = Some(error),
            }
        }
        match last {
            Some(error) if !is_not_there(&error) => Err(Some(format!(
                "Could not connect to {}: {error}",
                self.config.address()
            ))),
            _ => Err(None),
        }
    }

    /// Sets up a new connection and says it is connected.
    fn ready(&mut self, mut stream: TcpStream) -> io::Result<TcpStream> {
        // Key presses go out at once instead of waiting to fill a packet.
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(self.timing.poll))?;
        if let Some(greeting) = self.config.greeting() {
            stream.write_all(&greeting)?;
        }
        self.set_state(NetState::Connected {
            address: self.config.address(),
            protocol: self.config.protocol,
        });
        Ok(stream)
    }

    /// Passes data both ways until the session closes, the connection ends
    /// or the settings change.
    fn serve(&mut self, mut stream: TcpStream) -> Ended {
        let mut telnet = (self.config.protocol == Protocol::Telnet).then(Telnet::default);
        let mut buf = vec![0; BATCH_MAX];
        let mut pending = Vec::new();
        let mut reply = Vec::new();
        let mut batch_started: Option<Instant> = None;

        let ended = loop {
            if let Some(ended) = self.do_commands(&mut stream, telnet.as_ref()) {
                break ended;
            }

            match stream.read(&mut buf) {
                Ok(0) => break Ended::Lost,
                Ok(count) => {
                    match &mut telnet {
                        Some(telnet) => telnet.receive(&buf[..count], &mut pending, &mut reply),
                        None => pending.extend_from_slice(&buf[..count]),
                    }
                    if !reply.is_empty() && stream.write_all(&std::mem::take(&mut reply)).is_err() {
                        break Ended::Lost;
                    }
                    if !pending.is_empty() {
                        batch_started.get_or_insert_with(Instant::now);
                    }
                }
                Err(error) if is_quiet(&error) => {}
                Err(_) => break Ended::Lost,
            }

            let batch_due = batch_started.is_some_and(|start| start.elapsed() >= self.timing.batch);
            if batch_due || pending.len() >= BATCH_MAX {
                self.emit(Event::Data(std::mem::take(&mut pending)));
                batch_started = None;
            }
        };

        if !pending.is_empty() {
            self.emit(Event::Data(pending));
        }
        if matches!(ended, Ended::Lost) {
            self.set_state(NetState::Waiting { problem: None });
        }
        ended
    }

    /// Does what the app asked, until no command is left. Returns why serving
    /// should end, if it should.
    fn do_commands(&mut self, stream: &mut TcpStream, telnet: Option<&Telnet>) -> Option<Ended> {
        loop {
            let command = match self.commands.try_recv() {
                Ok(command) => command,
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => return Some(Ended::Closed),
            };
            match command {
                Command::Write(data) => {
                    let sent = match telnet {
                        Some(telnet) => stream.write_all(&telnet.encode(&data)),
                        None => stream.write_all(&data),
                    };
                    if sent.is_err() {
                        return Some(Ended::Lost);
                    }
                }
                Command::Reconfigure(config) => {
                    self.config = config;
                    return Some(Ended::Reconnect);
                }
                Command::Close => return Some(Ended::Closed),
            }
        }
    }

    /// Waits while not connected, still doing what the app asks. Returns
    /// false when the session should end.
    fn wait(&mut self, time: Duration) -> bool {
        let until = Instant::now() + time;
        loop {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return true;
            }
            match self.commands.recv_timeout(left) {
                Err(RecvTimeoutError::Timeout) => return true,
                Ok(Command::Close) | Err(RecvTimeoutError::Disconnected) => return false,
                // Try again at once with the new settings.
                Ok(Command::Reconfigure(config)) => {
                    self.config = config;
                    return true;
                }
                // Nothing to send it to.
                Ok(Command::Write(_)) => {}
            }
        }
    }

    fn set_state(&mut self, state: NetState) {
        if self.state.as_ref() != Some(&state) {
            self.state = Some(state.clone());
            self.emit(Event::State(state));
        }
    }

    fn emit(&mut self, event: Event) {
        (self.on_event)(event);
    }
}

/// Errors that only mean nothing arrived during the read.
fn is_quiet(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        ErrorKind::TimedOut | ErrorKind::WouldBlock | ErrorKind::Interrupted
    )
}

/// Errors that only mean nothing listens there yet, which is waited out.
fn is_not_there(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        ErrorKind::ConnectionRefused
            | ErrorKind::TimedOut
            | ErrorKind::WouldBlock
            | ErrorKind::ConnectionReset
            | ErrorKind::ConnectionAborted
    )
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;

    use super::*;

    const FAST: Timing = Timing {
        retry: Duration::from_millis(20),
        connect: Duration::from_millis(500),
        poll: Duration::from_millis(5),
        batch: Duration::from_millis(5),
    };

    /// A server on a free port of this machine.
    fn server() -> (TcpListener, u16) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        (listener, port)
    }

    fn config(port: u16, protocol: Protocol) -> TcpConfig {
        TcpConfig {
            host: "127.0.0.1".into(),
            port,
            protocol,
            channel: 0,
        }
    }

    /// Starts a session and gives back what it reports.
    fn start(config: TcpConfig) -> (Session, Receiver<Event>) {
        let (sender, events) = mpsc::channel();
        let session = Session::start_with(
            config,
            Box::new(move |event| {
                let _ = sender.send(event);
            }),
            FAST,
        );
        (session, events)
    }

    /// Takes the session's connection on the server side.
    fn accept(listener: &TcpListener) -> TcpStream {
        let (stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream
    }

    /// Waits for the first event that `wanted` accepts, skipping the others.
    fn expect(events: &Receiver<Event>, what: &str, wanted: impl Fn(&Event) -> bool) -> Event {
        let until = Instant::now() + Duration::from_secs(5);
        while let Some(left) = until.checked_duration_since(Instant::now()) {
            match events.recv_timeout(left) {
                Ok(event) if wanted(&event) => return event,
                Ok(_) => {}
                Err(_) => break,
            }
        }
        panic!("no {what} within 5 s");
    }

    fn connected(events: &Receiver<Event>) -> String {
        match expect(events, "connect", |event| {
            matches!(event, Event::State(NetState::Connected { .. }))
        }) {
            Event::State(NetState::Connected { address, .. }) => address,
            _ => unreachable!(),
        }
    }

    fn waiting(events: &Receiver<Event>) -> Option<String> {
        match expect(events, "wait", |event| {
            matches!(event, Event::State(NetState::Waiting { .. }))
        }) {
            Event::State(NetState::Waiting { problem }) => problem,
            _ => unreachable!(),
        }
    }

    /// Collects received data until `count` bytes have come.
    fn received(events: &Receiver<Event>, count: usize) -> Vec<u8> {
        let mut data = Vec::new();
        while data.len() < count {
            if let Event::Data(bytes) =
                expect(events, "data", |event| matches!(event, Event::Data(_)))
            {
                data.extend(bytes);
            }
        }
        data
    }

    /// Reads exactly `count` bytes on the server side.
    fn read(stream: &mut TcpStream, count: usize) -> Vec<u8> {
        let mut data = vec![0; count];
        stream.read_exact(&mut data).unwrap();
        data
    }

    #[test]
    fn connects_and_passes_data_both_ways() {
        let (listener, port) = server();
        let (session, events) = start(config(port, Protocol::Raw));
        let mut stream = accept(&listener);
        assert_eq!(connected(&events), format!("127.0.0.1:{port}"));

        stream.write_all(b"hello\r\n").unwrap();
        assert_eq!(received(&events, 7), b"hello\r\n");

        session.write(b"help\r".to_vec());
        assert_eq!(read(&mut stream, 5), b"help\r");
    }

    #[test]
    fn waits_for_a_server_that_starts_later() {
        let (listener, port) = server();
        drop(listener);
        let (_session, events) = start(config(port, Protocol::Raw));
        assert_eq!(waiting(&events), None);

        let listener = TcpListener::bind(("127.0.0.1", port)).unwrap();
        let _stream = accept(&listener);
        connected(&events);
    }

    #[test]
    fn connects_again_after_the_server_closes() {
        let (listener, port) = server();
        let (_session, events) = start(config(port, Protocol::Raw));
        drop(accept(&listener));
        connected(&events);
        assert_eq!(waiting(&events), None);

        let mut stream = accept(&listener);
        connected(&events);
        stream.write_all(b"back").unwrap();
        assert_eq!(received(&events, 4), b"back");
    }

    #[test]
    fn telnet_options_are_answered_and_kept_out() {
        let (listener, port) = server();
        let (session, events) = start(config(port, Protocol::Telnet));
        let mut stream = accept(&listener);
        connected(&events);

        // IAC WILL ECHO, then a prompt.
        stream.write_all(&[255, 251, 1]).unwrap();
        stream.write_all(b"login: ").unwrap();
        assert_eq!(received(&events, 7), b"login: ");
        assert_eq!(read(&mut stream, 3), [255, 253, 1]);

        session.write(b"ls\r".to_vec());
        assert_eq!(read(&mut stream, 4), b"ls\r\0");
    }

    #[test]
    fn jlink_is_told_the_channel_first() {
        let (listener, port) = server();
        let config = TcpConfig {
            channel: 1,
            ..config(port, Protocol::Jlink)
        };
        let (_session, events) = start(config);
        let mut stream = accept(&listener);
        connected(&events);
        let greeting = b"$$SEGGER_TELNET_ConfigStr=RTTCh;1$$";
        assert_eq!(read(&mut stream, greeting.len()), greeting);
    }

    #[test]
    fn missing_settings_say_what_to_fill_in() {
        let (_session, events) = start(TcpConfig::default());
        let problem = waiting(&events).expect("a reason");
        assert!(problem.contains("port"), "{problem}");
    }

    #[test]
    fn new_settings_connect_to_the_new_place() {
        let (first, first_port) = server();
        let (second, second_port) = server();
        let (session, events) = start(config(first_port, Protocol::Raw));
        let _stream = accept(&first);
        connected(&events);

        session.reconfigure(config(second_port, Protocol::Raw));
        let _stream = accept(&second);
        assert_eq!(connected(&events), format!("127.0.0.1:{second_port}"));
    }

    #[test]
    fn dropping_the_session_ends_its_thread() {
        let (listener, port) = server();
        let (session, events) = start(config(port, Protocol::Raw));
        let _stream = accept(&listener);
        connected(&events);

        let before = Instant::now();
        drop(session);
        assert!(before.elapsed() < Duration::from_secs(1));
        while events.try_recv().is_ok() {}
        assert!(matches!(
            events.recv_timeout(Duration::from_millis(50)),
            Err(RecvTimeoutError::Disconnected)
        ));
    }
}
