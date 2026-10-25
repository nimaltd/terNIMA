//! One serial session: a port kept open on its own thread, and found and
//! opened again when the device comes back.

use std::io::{self, ErrorKind};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde::Serialize;

use super::config::{FlowControl, SerialConfig};
use super::matching::{PortId, find_port};
use super::system::{SerialLink, SerialSystem};

/// The modem lines a device drives.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ModemLines {
    pub cts: bool,
    pub dsr: bool,
    pub dcd: bool,
    pub ri: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SessionState {
    /// The port is not open. `problem` says why when the port is there but
    /// could not be opened; without it, the device is not plugged in.
    Waiting { problem: Option<String> },
    /// The port is open. `port` is how it was found this time: its name may
    /// differ from last time.
    Connected { port: PortId },
}

/// What a session tells the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// Bytes that arrived, in the order they came.
    Data(Vec<u8>),
    State(SessionState),
    /// The modem lines changed.
    Lines(ModemLines),
}

enum Command {
    Write(Vec<u8>),
    Dtr(bool),
    Rts(bool),
    Break,
    Reconfigure(SerialConfig),
    Close,
}

/// How often things happen. Tests make the times shorter.
#[derive(Clone, Copy)]
struct Timing {
    /// Between two tries to find and open the port.
    retry: Duration,
    /// The longest received bytes wait before they are passed on, so the
    /// screen updates about 60 times a second.
    batch: Duration,
    /// Between two reads of the modem lines.
    lines: Duration,
    /// Between two checks that the port is still listed, for systems where
    /// reading does not fail when a device is unplugged.
    presence: Duration,
    /// How long a break lasts.
    break_time: Duration,
}

const TIMING: Timing = Timing {
    retry: Duration::from_millis(500),
    batch: Duration::from_millis(16),
    lines: Duration::from_millis(100),
    presence: Duration::from_secs(1),
    break_time: Duration::from_millis(250),
};

/// The most bytes passed on at once.
const BATCH_MAX: usize = 64 * 1024;

/// A serial session. Dropping it closes the port.
pub struct Session {
    commands: Sender<Command>,
    thread: Option<JoinHandle<()>>,
}

impl Session {
    /// Starts looking for the port and keeps it open until the session is
    /// dropped. Everything that happens comes back through `on_event`, which
    /// runs on the session's own thread.
    pub fn start(
        system: Arc<dyn SerialSystem>,
        port: PortId,
        config: SerialConfig,
        on_event: impl FnMut(Event) + Send + 'static,
    ) -> Self {
        Self::start_with(system, port, config, Box::new(on_event), TIMING)
    }

    fn start_with(
        system: Arc<dyn SerialSystem>,
        port: PortId,
        config: SerialConfig,
        on_event: Box<dyn FnMut(Event) + Send>,
        timing: Timing,
    ) -> Self {
        let (commands, receiver) = mpsc::channel();
        let worker = Worker {
            system,
            port,
            config,
            on_event,
            commands: receiver,
            timing,
            state: None,
        };
        let thread = thread::Builder::new()
            .name("ternima-serial".into())
            .spawn(move || worker.run())
            .expect("the OS could not start a thread");
        Self {
            commands,
            thread: Some(thread),
        }
    }

    /// Sends bytes to the device. They are dropped while the port is not open.
    pub fn write(&self, data: Vec<u8>) {
        self.send(Command::Write(data));
    }

    pub fn set_dtr(&self, on: bool) {
        self.send(Command::Dtr(on));
    }

    pub fn set_rts(&self, on: bool) {
        self.send(Command::Rts(on));
    }

    /// Holds the TX line low for a quarter of a second.
    pub fn send_break(&self) {
        self.send(Command::Break);
    }

    /// Opens the port again with new settings.
    pub fn reconfigure(&self, config: SerialConfig) {
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

/// Why serving an open port ended.
enum Ended {
    /// The app closed the session.
    Closed,
    /// The device went away.
    Lost,
    /// New settings: open the port again.
    Reopen,
}

struct Worker {
    system: Arc<dyn SerialSystem>,
    port: PortId,
    config: SerialConfig,
    on_event: Box<dyn FnMut(Event) + Send>,
    commands: Receiver<Command>,
    timing: Timing,
    /// The state last told to the app, so each change is told once.
    state: Option<SessionState>,
}

impl Worker {
    fn run(mut self) {
        loop {
            match self.open() {
                Ok(link) => match self.serve(link) {
                    Ended::Closed => return,
                    Ended::Lost | Ended::Reopen => continue,
                },
                Err(problem) => {
                    self.set_state(SessionState::Waiting { problem });
                    if !self.wait(self.timing.retry) {
                        return;
                    }
                }
            }
        }
    }

    /// Finds the port and opens it, or says why not: `None` when it is not plugged in.
    fn open(&mut self) -> Result<Box<dyn SerialLink>, Option<String>> {
        if let Some(problem) = self.config.problem() {
            return Err(Some(problem));
        }
        let ports = self.system.ports();
        let found = find_port(&self.port, &ports).ok_or(None)?;
        let link = self
            .system
            .open(&found.name, &self.config)
            .map_err(|error| Some(open_problem(&found.name, &error)))?;
        self.port = PortId::from(found);
        self.set_state(SessionState::Connected {
            port: self.port.clone(),
        });
        Ok(link)
    }

    /// Passes data both ways until the session closes, the device goes away
    /// or the settings change.
    fn serve(&mut self, mut link: Box<dyn SerialLink>) -> Ended {
        let mut buf = vec![0; BATCH_MAX];
        let mut pending = Vec::new();
        let mut batch_started: Option<Instant> = None;
        let mut lines: Option<ModemLines> = None;
        let mut lines_read: Option<Instant> = None;
        let mut presence_checked = Instant::now();

        let ended = loop {
            if let Some(ended) = self.do_commands(link.as_mut()) {
                break ended;
            }

            match link.read(&mut buf) {
                Ok(0) => {}
                Ok(count) => {
                    pending.extend_from_slice(&buf[..count]);
                    batch_started.get_or_insert_with(Instant::now);
                }
                Err(error) if is_quiet(&error) => {}
                Err(_) => break Ended::Lost,
            }

            let batch_due = batch_started.is_some_and(|start| start.elapsed() >= self.timing.batch);
            if batch_due || pending.len() >= BATCH_MAX {
                self.emit(Event::Data(std::mem::take(&mut pending)));
                batch_started = None;
            }

            if lines_read.is_none_or(|read| read.elapsed() >= self.timing.lines) {
                lines_read = Some(Instant::now());
                // Some adapters cannot report the lines; they just stay unknown.
                if let Ok(now) = link.modem_lines()
                    && lines != Some(now)
                {
                    lines = Some(now);
                    self.emit(Event::Lines(now));
                }
            }

            if presence_checked.elapsed() >= self.timing.presence {
                presence_checked = Instant::now();
                let name = &self.port.name;
                if !self.system.ports().iter().any(|port| &port.name == name) {
                    break Ended::Lost;
                }
            }
        };

        if !pending.is_empty() {
            self.emit(Event::Data(pending));
        }
        if matches!(ended, Ended::Lost) {
            self.set_state(SessionState::Waiting { problem: None });
        }
        ended
    }

    /// Does what the app asked, until no command is left. Returns why serving
    /// should end, if it should.
    fn do_commands(&mut self, link: &mut dyn SerialLink) -> Option<Ended> {
        loop {
            let command = match self.commands.try_recv() {
                Ok(command) => command,
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => return Some(Ended::Closed),
            };
            match command {
                Command::Write(data) => match link.write_all(&data) {
                    Ok(()) => {}
                    // The device takes nothing, as when flow control holds it:
                    // the rest is dropped, and the port stays open.
                    Err(error) if error.kind() == ErrorKind::TimedOut => {}
                    Err(_) => return Some(Ended::Lost),
                },
                Command::Dtr(on) => {
                    self.config.dtr = on;
                    let _ = link.set_dtr(on);
                }
                Command::Rts(on) => {
                    self.config.rts = on;
                    if self.config.flow != FlowControl::RtsCts {
                        let _ = link.set_rts(on);
                    }
                }
                Command::Break => {
                    let _ = link.set_break(true);
                    thread::sleep(self.timing.break_time);
                    let _ = link.set_break(false);
                }
                Command::Reconfigure(config) => {
                    self.config = config;
                    return Some(Ended::Reopen);
                }
                Command::Close => return Some(Ended::Closed),
            }
        }
    }

    /// Waits while the port is not open, still doing what the app asks.
    /// Returns false when the session should end.
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
                Ok(Command::Dtr(on)) => self.config.dtr = on,
                Ok(Command::Rts(on)) => self.config.rts = on,
                // Try again at once with the new settings.
                Ok(Command::Reconfigure(config)) => {
                    self.config = config;
                    return true;
                }
                // Nothing to send them to.
                Ok(Command::Write(_) | Command::Break) => {}
            }
        }
    }

    fn set_state(&mut self, state: SessionState) {
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

/// Says in plain words why a port that is there could not be opened.
fn open_problem(name: &str, error: &io::Error) -> String {
    match error.kind() {
        ErrorKind::PermissionDenied if cfg!(target_os = "linux") => format!(
            "No permission to open {name}. Add your user to the dialout group, then log out and in again."
        ),
        // Windows says "not found" or "access denied" for a port another program has open.
        ErrorKind::PermissionDenied | ErrorKind::NotFound if cfg!(windows) => {
            format!("{name} is in use by another program.")
        }
        ErrorKind::PermissionDenied => format!("{name} is in use by another program."),
        _ => format!("Could not open {name}: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::super::ports::{PortInfo, UsbId};
    use super::*;

    const FAST: Timing = Timing {
        retry: Duration::from_millis(20),
        batch: Duration::from_millis(5),
        lines: Duration::from_millis(10),
        presence: Duration::from_millis(30),
        break_time: Duration::from_millis(1),
    };

    /// A pretend device: what the test sends it arrives as received data,
    /// and what the session writes is kept.
    #[derive(Default)]
    struct Device {
        incoming: Mutex<Vec<u8>>,
        written: Mutex<Vec<u8>>,
        lines: Mutex<ModemLines>,
        dtr: Mutex<Option<bool>>,
        plugged: Mutex<bool>,
        /// Writes take nothing, as when flow control holds the port.
        stalled: Mutex<bool>,
    }

    /// Pretend ports, which tests plug in and pull out.
    #[derive(Default)]
    struct FakeSystem {
        ports: Mutex<Vec<(PortInfo, Arc<Device>)>>,
        busy: Mutex<Option<String>>,
    }

    impl FakeSystem {
        fn plug(&self, name: &str, serial: &str) -> Arc<Device> {
            let device = Arc::new(Device::default());
            *device.plugged.lock().unwrap() = true;
            let info = PortInfo {
                name: name.into(),
                usb: Some(UsbId {
                    vid: 0x0483,
                    pid: 0x3754,
                    serial: Some(serial.into()),
                    interface: Some(1),
                    location: None,
                }),
                manufacturer: None,
                product: None,
            };
            self.ports.lock().unwrap().push((info, device.clone()));
            device
        }

        fn unplug(&self, name: &str) {
            let mut ports = self.ports.lock().unwrap();
            for (info, device) in ports.iter() {
                if info.name == name {
                    *device.plugged.lock().unwrap() = false;
                }
            }
            ports.retain(|(info, _)| info.name != name);
        }

        fn id(&self, name: &str) -> PortId {
            let ports = self.ports.lock().unwrap();
            PortId::from(&ports.iter().find(|(info, _)| info.name == name).unwrap().0)
        }
    }

    impl SerialSystem for FakeSystem {
        fn ports(&self) -> Vec<PortInfo> {
            self.ports
                .lock()
                .unwrap()
                .iter()
                .map(|(info, _)| info.clone())
                .collect()
        }

        fn open(&self, name: &str, config: &SerialConfig) -> io::Result<Box<dyn SerialLink>> {
            if self.busy.lock().unwrap().as_deref() == Some(name) {
                return Err(io::Error::new(ErrorKind::PermissionDenied, "busy"));
            }
            let ports = self.ports.lock().unwrap();
            let (_, device) = ports
                .iter()
                .find(|(info, _)| info.name == name)
                .ok_or_else(|| io::Error::new(ErrorKind::NotFound, "gone"))?;
            *device.dtr.lock().unwrap() = Some(config.dtr);
            Ok(Box::new(FakeLink(device.clone())))
        }
    }

    struct FakeLink(Arc<Device>);

    impl SerialLink for FakeLink {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if !*self.0.plugged.lock().unwrap() {
                return Err(io::Error::new(ErrorKind::BrokenPipe, "unplugged"));
            }
            let mut incoming = self.0.incoming.lock().unwrap();
            if incoming.is_empty() {
                drop(incoming);
                thread::sleep(Duration::from_millis(2));
                return Err(io::Error::new(ErrorKind::TimedOut, "nothing"));
            }
            let count = incoming.len().min(buf.len());
            buf[..count].copy_from_slice(&incoming[..count]);
            incoming.drain(..count);
            Ok(count)
        }

        fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
            if *self.0.stalled.lock().unwrap() {
                return Err(io::Error::new(ErrorKind::TimedOut, "stalled"));
            }
            self.0.written.lock().unwrap().extend_from_slice(data);
            Ok(())
        }

        fn set_dtr(&mut self, on: bool) -> io::Result<()> {
            *self.0.dtr.lock().unwrap() = Some(on);
            Ok(())
        }

        fn set_rts(&mut self, _on: bool) -> io::Result<()> {
            Ok(())
        }

        fn set_break(&mut self, _on: bool) -> io::Result<()> {
            Ok(())
        }

        fn modem_lines(&mut self) -> io::Result<ModemLines> {
            Ok(*self.0.lines.lock().unwrap())
        }
    }

    /// Starts a session and gives back what it reports.
    fn start(system: &Arc<FakeSystem>, port: PortId) -> (Session, Receiver<Event>) {
        let (sender, events) = mpsc::channel();
        let session = Session::start_with(
            system.clone(),
            port,
            SerialConfig::default(),
            Box::new(move |event| {
                let _ = sender.send(event);
            }),
            FAST,
        );
        (session, events)
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
            matches!(event, Event::State(SessionState::Connected { .. }))
        }) {
            Event::State(SessionState::Connected { port }) => port.name,
            _ => unreachable!(),
        }
    }

    fn waiting(events: &Receiver<Event>) -> Option<String> {
        match expect(events, "wait", |event| {
            matches!(event, Event::State(SessionState::Waiting { .. }))
        }) {
            Event::State(SessionState::Waiting { problem }) => problem,
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

    #[test]
    fn connects_and_passes_data_both_ways() {
        let system = Arc::new(FakeSystem::default());
        let device = system.plug("COM6", "AAA");
        let (session, events) = start(&system, system.id("COM6"));

        assert_eq!(connected(&events), "COM6");
        device
            .incoming
            .lock()
            .unwrap()
            .extend_from_slice(b"hello\r\n");
        assert_eq!(received(&events, 7), b"hello\r\n");

        session.write(b"help\r".to_vec());
        thread::sleep(Duration::from_millis(50));
        assert_eq!(*device.written.lock().unwrap(), b"help\r");
    }

    #[test]
    fn a_port_that_takes_nothing_stays_open() {
        let system = Arc::new(FakeSystem::default());
        let device = system.plug("COM6", "AAA");
        let (session, events) = start(&system, system.id("COM6"));
        connected(&events);

        *device.stalled.lock().unwrap() = true;
        session.write(b"lost in flow control".to_vec());
        thread::sleep(Duration::from_millis(100));
        assert!(
            events
                .try_iter()
                .all(|event| !matches!(event, Event::State(SessionState::Waiting { .. })))
        );

        *device.stalled.lock().unwrap() = false;
        session.write(b"ok".to_vec());
        thread::sleep(Duration::from_millis(50));
        assert_eq!(*device.written.lock().unwrap(), b"ok");
    }

    #[test]
    fn waits_for_a_device_that_is_not_plugged_in() {
        let system = Arc::new(FakeSystem::default());
        system.plug("COM6", "AAA");
        let id = system.id("COM6");
        system.unplug("COM6");

        let (_session, events) = start(&system, id);
        assert_eq!(waiting(&events), None);

        // It comes back under another name, as Windows sometimes does.
        system.plug("COM9", "AAA");
        assert_eq!(connected(&events), "COM9");
    }

    #[test]
    fn reconnects_after_the_device_is_unplugged_and_back() {
        let system = Arc::new(FakeSystem::default());
        system.plug("COM6", "AAA");
        let (_session, events) = start(&system, system.id("COM6"));
        assert_eq!(connected(&events), "COM6");

        system.unplug("COM6");
        assert_eq!(waiting(&events), None);

        let device = system.plug("COM6", "AAA");
        assert_eq!(connected(&events), "COM6");
        device.incoming.lock().unwrap().extend_from_slice(b"back");
        assert_eq!(received(&events, 4), b"back");
    }

    #[test]
    fn another_board_of_the_same_kind_is_not_taken() {
        let system = Arc::new(FakeSystem::default());
        system.plug("COM6", "AAA");
        let id = system.id("COM6");
        system.unplug("COM6");
        system.plug("COM6", "BBB");

        let (_session, events) = start(&system, id);
        assert_eq!(waiting(&events), None);
        thread::sleep(Duration::from_millis(100));
        assert!(
            events
                .try_iter()
                .all(|event| !matches!(event, Event::State(SessionState::Connected { .. })))
        );
    }

    #[test]
    fn busy_port_says_why_and_opens_when_freed() {
        let system = Arc::new(FakeSystem::default());
        system.plug("COM6", "AAA");
        *system.busy.lock().unwrap() = Some("COM6".into());

        let (_session, events) = start(&system, system.id("COM6"));
        let problem = waiting(&events).expect("a reason");
        assert!(problem.contains("COM6"), "{problem}");

        *system.busy.lock().unwrap() = None;
        assert_eq!(connected(&events), "COM6");
    }

    #[test]
    fn modem_lines_are_reported_when_they_change() {
        let system = Arc::new(FakeSystem::default());
        let device = system.plug("COM6", "AAA");
        let (_session, events) = start(&system, system.id("COM6"));
        connected(&events);

        device.lines.lock().unwrap().cts = true;
        let event = expect(
            &events,
            "lines with CTS",
            |event| matches!(event, Event::Lines(lines) if lines.cts),
        );
        assert_eq!(
            event,
            Event::Lines(ModemLines {
                cts: true,
                ..ModemLines::default()
            })
        );
    }

    #[test]
    fn dtr_follows_the_app_and_the_settings() {
        let system = Arc::new(FakeSystem::default());
        let device = system.plug("COM6", "AAA");
        let (session, events) = start(&system, system.id("COM6"));
        connected(&events);
        assert_eq!(*device.dtr.lock().unwrap(), Some(true));

        session.set_dtr(false);
        thread::sleep(Duration::from_millis(50));
        assert_eq!(*device.dtr.lock().unwrap(), Some(false));
    }

    #[test]
    fn dropping_the_session_ends_its_thread() {
        let system = Arc::new(FakeSystem::default());
        system.plug("COM6", "AAA");
        let (session, events) = start(&system, system.id("COM6"));
        connected(&events);

        let before = Instant::now();
        drop(session);
        assert!(before.elapsed() < Duration::from_secs(1));
        // Its sender is gone with the thread.
        while events.try_recv().is_ok() {}
        assert!(matches!(
            events.recv_timeout(Duration::from_millis(50)),
            Err(RecvTimeoutError::Disconnected)
        ));
    }

    #[test]
    fn open_problems_are_plain_words() {
        let busy = open_problem("COM6", &io::Error::new(ErrorKind::PermissionDenied, "x"));
        assert!(busy.contains("COM6"), "{busy}");
        let other = open_problem("COM6", &io::Error::new(ErrorKind::InvalidInput, "bad baud"));
        assert_eq!(other, "Could not open COM6: bad baud");
    }
}
