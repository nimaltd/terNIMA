//! One SWO session: a chip attached through a probe with tracing on, read on
//! its own thread, and attached again when the probe comes back.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use probe_rs::architecture::arm::component::TraceSink;
use probe_rs::{MemoryInterface, Session as ProbeSession};
use serde::Serialize;

use super::config::SwoConfig;
use super::itm::{ItmDecoder, ItmPacket};
use crate::probe::{self, Target};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SwoState {
    /// Not reading yet. `problem` says why; without it, the probe is not plugged in.
    Waiting { problem: Option<String> },
    /// Reading the trace pin of `chip` at `baud`.
    Connected { chip: String, baud: u32 },
}

/// What a session tells the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// Bytes from the stimulus port shown, in the order they came.
    Data(Vec<u8>),
    State(SwoState),
    /// The chip's trace FIFO overflowed and some output was lost. Told at
    /// most once a second, with how many overflows there were.
    Overflow(u32),
}

/// Starts tracing on a chip.
pub trait SwoBackend: Send + Sync + 'static {
    /// Attaches and turns SWO on. Gives `Err(None)` when the probe is not
    /// plugged in, and the reason when it is but did not work.
    fn start(&self, config: &SwoConfig) -> Result<Box<dyn SwoLink>, Option<String>>;
}

/// A chip with tracing on.
pub trait SwoLink: Send {
    /// What came out of the trace pin since the last read. An error means
    /// the probe or the chip is gone.
    fn read(&mut self) -> Result<Vec<u8>, String>;
    /// Resets the chip and turns tracing on again.
    fn reset(&mut self) -> Result<(), String>;
}

/// Real probes, through probe-rs.
pub struct ProbeRs;

impl SwoBackend for ProbeRs {
    fn start(&self, config: &SwoConfig) -> Result<Box<dyn SwoLink>, Option<String>> {
        let session = probe::attach(&Target {
            probe: config.probe.as_ref(),
            chip: &config.chip,
            speed_khz: config.speed_khz,
            under_reset: config.under_reset,
        })?;
        let mut link = ProbeRsLink {
            session,
            config: config.clone(),
        };
        link.trace_on().map_err(Some)?;
        Ok(Box::new(link))
    }
}

struct ProbeRsLink {
    session: ProbeSession,
    config: SwoConfig,
}

impl ProbeRsLink {
    fn trace_on(&mut self) -> Result<(), String> {
        let swo = probe_rs::architecture::arm::SwoConfig::new(self.config.clock_hz)
            .set_baud(self.config.baud)
            .set_mode_uart()
            .set_continuous_formatting(false);
        self.session
            .setup_tracing(0, TraceSink::Swo(swo))
            .map_err(|error| format!("Could not turn SWO on: {error}"))?;
        self.h5_trace_pin()
    }

    /// probe-rs turns the trace pin on through DBGMCU_CR at 0xE0044004, where
    /// the STM32U5 and L5 have it. The STM32H5 has it at 0x44024004, so it is
    /// set there too: TRACE_IOEN (bit 4) and TRACE_EN (bit 5).
    fn h5_trace_pin(&mut self) -> Result<(), String> {
        const H5_DBGMCU_CR: u64 = 0x4402_4004;
        const TRACE_IOEN_AND_EN: u32 = (1 << 4) | (1 << 5);
        if !self
            .config
            .chip
            .trim()
            .to_uppercase()
            .starts_with("STM32H5")
        {
            return Ok(());
        }
        let mut core = self.session.core(0).map_err(|error| error.to_string())?;
        let value = core
            .read_word_32(H5_DBGMCU_CR)
            .map_err(|error| error.to_string())?;
        core.write_word_32(H5_DBGMCU_CR, value | TRACE_IOEN_AND_EN)
            .map_err(|error| error.to_string())
    }
}

impl SwoLink for ProbeRsLink {
    fn read(&mut self) -> Result<Vec<u8>, String> {
        self.session
            .read_trace_data()
            .map_err(|error| error.to_string())
    }

    fn reset(&mut self) -> Result<(), String> {
        self.session
            .core(0)
            .and_then(|mut core| core.reset())
            .map_err(|error| error.to_string())?;
        self.trace_on()
    }
}

enum Command {
    Reset,
    Reconfigure(SwoConfig),
    Close,
}

/// How often things happen. Tests make the times shorter.
#[derive(Clone, Copy)]
struct Timing {
    /// Between two tries to attach.
    retry: Duration,
    /// The longest received bytes wait before they are passed on.
    batch: Duration,
    /// The pause between reads when nothing came.
    idle: Duration,
    /// The shortest time between two overflow reports.
    overflow: Duration,
}

const TIMING: Timing = Timing {
    retry: Duration::from_secs(1),
    batch: Duration::from_millis(16),
    idle: Duration::from_millis(5),
    overflow: Duration::from_secs(1),
};

/// An SWO session. Dropping it lets go of the probe.
pub struct Session {
    commands: Sender<Command>,
    thread: Option<JoinHandle<()>>,
}

impl Session {
    /// Starts attaching and keeps reading until the session is dropped.
    /// Everything that happens comes back through `on_event`, on the session's own thread.
    pub fn start(
        backend: Arc<dyn SwoBackend>,
        config: SwoConfig,
        on_event: impl FnMut(Event) + Send + 'static,
    ) -> Self {
        Self::start_with(backend, config, Box::new(on_event), TIMING)
    }

    fn start_with(
        backend: Arc<dyn SwoBackend>,
        config: SwoConfig,
        on_event: Box<dyn FnMut(Event) + Send>,
        timing: Timing,
    ) -> Self {
        let (commands, receiver) = mpsc::channel();
        let worker = Worker {
            backend,
            config,
            on_event,
            commands: receiver,
            timing,
            state: None,
        };
        let thread = thread::Builder::new()
            .name("ternimal-swo".into())
            .spawn(move || worker.run())
            .expect("the OS could not start a thread");
        Self {
            commands,
            thread: Some(thread),
        }
    }

    /// Resets the chip, then turns tracing on again.
    pub fn reset(&self) {
        let _ = self.commands.send(Command::Reset);
    }

    /// Attaches again with new settings.
    pub fn reconfigure(&self, config: SwoConfig) {
        let _ = self.commands.send(Command::Reconfigure(config));
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

/// Why reading ended.
enum Ended {
    Closed,
    Reopen,
    Lost(String),
}

struct Worker {
    backend: Arc<dyn SwoBackend>,
    config: SwoConfig,
    on_event: Box<dyn FnMut(Event) + Send>,
    commands: Receiver<Command>,
    timing: Timing,
    /// The state last told to the app, so each change is told once.
    state: Option<SwoState>,
}

impl Worker {
    fn run(mut self) {
        loop {
            let started = match self.config.problem() {
                Some(problem) => Err(Some(problem)),
                None => self.backend.start(&self.config),
            };
            match started {
                Ok(link) => match self.pump(link) {
                    Ended::Closed => return,
                    Ended::Reopen => continue,
                    Ended::Lost(problem) => self.set_state(SwoState::Waiting {
                        problem: Some(problem),
                    }),
                },
                Err(problem) => self.set_state(SwoState::Waiting { problem }),
            }
            if !self.wait(self.timing.retry) {
                return;
            }
        }
    }

    /// Reads the trace pin and passes on the chosen port's data.
    fn pump(&mut self, mut link: Box<dyn SwoLink>) -> Ended {
        self.set_state(SwoState::Connected {
            chip: self.config.chip.trim().to_string(),
            baud: self.config.baud,
        });
        let mut decoder = ItmDecoder::new();
        let mut pending = Vec::new();
        let mut batch_started: Option<Instant> = None;
        let mut overflows = 0;
        let mut overflow_told = Instant::now() - self.timing.overflow;

        let ended = loop {
            match self.commands.try_recv() {
                Ok(Command::Close) | Err(TryRecvError::Disconnected) => break Ended::Closed,
                Ok(Command::Reconfigure(config)) => {
                    self.config = config;
                    break Ended::Reopen;
                }
                Ok(Command::Reset) => {
                    if let Err(problem) = link.reset() {
                        break Ended::Lost(problem);
                    }
                    decoder = ItmDecoder::new();
                }
                Err(TryRecvError::Empty) => {}
            }

            let raw = match link.read() {
                Ok(raw) => raw,
                Err(problem) => break Ended::Lost(problem),
            };
            for packet in decoder.feed(&raw) {
                match packet {
                    ItmPacket::Data { port, bytes } => {
                        if self.config.all_ports || port == self.config.port {
                            pending.extend_from_slice(&bytes);
                            batch_started.get_or_insert_with(Instant::now);
                        }
                    }
                    ItmPacket::Overflow => overflows += 1,
                }
            }

            if batch_started.is_some_and(|start| start.elapsed() >= self.timing.batch) {
                self.emit(Event::Data(std::mem::take(&mut pending)));
                batch_started = None;
            }
            if overflows > 0 && overflow_told.elapsed() >= self.timing.overflow {
                self.emit(Event::Overflow(overflows));
                overflows = 0;
                overflow_told = Instant::now();
            }
            if raw.is_empty() {
                thread::sleep(self.timing.idle);
            }
        };

        if !pending.is_empty() {
            self.emit(Event::Data(pending));
        }
        ended
    }

    /// Waits while not reading. Returns false when the session should end.
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
                // Nothing to reset while not attached.
                Ok(Command::Reset) => {}
            }
        }
    }

    fn set_state(&mut self, state: SwoState) {
        if self.state.as_ref() != Some(&state) {
            self.state = Some(state.clone());
            self.emit(Event::State(state));
        }
    }

    fn emit(&mut self, event: Event) {
        (self.on_event)(event);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    const FAST: Timing = Timing {
        retry: Duration::from_millis(20),
        batch: Duration::from_millis(5),
        idle: Duration::from_millis(1),
        overflow: Duration::from_millis(20),
    };

    /// A pretend probe and chip, which tests change while the session runs.
    #[derive(Default)]
    struct Chip {
        plugged: bool,
        start_problem: Option<String>,
        /// Raw SWO bytes waiting to be read.
        trace: Vec<u8>,
        resets: usize,
        starts: usize,
    }

    struct FakeBackend(Arc<Mutex<Chip>>);

    impl SwoBackend for FakeBackend {
        fn start(&self, _config: &SwoConfig) -> Result<Box<dyn SwoLink>, Option<String>> {
            let mut chip = self.0.lock().unwrap();
            if !chip.plugged {
                return Err(None);
            }
            if let Some(problem) = &chip.start_problem {
                return Err(Some(problem.clone()));
            }
            chip.starts += 1;
            Ok(Box::new(FakeLink(self.0.clone())))
        }
    }

    struct FakeLink(Arc<Mutex<Chip>>);

    impl SwoLink for FakeLink {
        fn read(&mut self) -> Result<Vec<u8>, String> {
            let mut chip = self.0.lock().unwrap();
            if !chip.plugged {
                return Err("probe unplugged".into());
            }
            Ok(std::mem::take(&mut chip.trace))
        }

        fn reset(&mut self) -> Result<(), String> {
            self.0.lock().unwrap().resets += 1;
            Ok(())
        }
    }

    fn config(port: u8, all_ports: bool) -> SwoConfig {
        SwoConfig {
            chip: "STM32H503RB".into(),
            clock_hz: 32_000_000,
            baud: 2_000_000,
            port,
            all_ports,
            ..SwoConfig::default()
        }
    }

    fn plugged() -> Arc<Mutex<Chip>> {
        Arc::new(Mutex::new(Chip {
            plugged: true,
            ..Chip::default()
        }))
    }

    fn start(chip: &Arc<Mutex<Chip>>, config: SwoConfig) -> (Session, Receiver<Event>) {
        let (sender, events) = mpsc::channel();
        let session = Session::start_with(
            Arc::new(FakeBackend(chip.clone())),
            config,
            Box::new(move |event| {
                let _ = sender.send(event);
            }),
            FAST,
        );
        (session, events)
    }

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

    fn connected(events: &Receiver<Event>) {
        expect(events, "connect", |event| {
            matches!(event, Event::State(SwoState::Connected { .. }))
        });
    }

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

    /// ITM packets of one byte each, for `text` on `port`.
    fn itm(port: u8, text: &str) -> Vec<u8> {
        text.bytes()
            .flat_map(|byte| [(port << 3) | 0x01, byte])
            .collect()
    }

    #[test]
    fn shows_the_chosen_port_only() {
        let chip = plugged();
        let (_session, events) = start(&chip, config(0, false));
        connected(&events);

        let mut trace = itm(1, "other");
        trace.extend(itm(0, "tick 1\r\n"));
        chip.lock().unwrap().trace = trace;
        assert_eq!(received(&events, 8), b"tick 1\r\n");
    }

    #[test]
    fn shows_every_port_when_asked() {
        let chip = plugged();
        let (_session, events) = start(&chip, config(0, true));
        connected(&events);

        let mut trace = itm(1, "a");
        trace.extend(itm(5, "b"));
        chip.lock().unwrap().trace = trace;
        assert_eq!(received(&events, 2), b"ab");
    }

    #[test]
    fn overflow_is_told() {
        let chip = plugged();
        let (_session, events) = start(&chip, config(0, false));
        connected(&events);
        chip.lock().unwrap().trace = vec![0x70, 0x70];
        let event = expect(&events, "overflow", |event| {
            matches!(event, Event::Overflow(_))
        });
        assert_eq!(event, Event::Overflow(2));
    }

    #[test]
    fn attaches_again_after_the_probe_comes_back() {
        let chip = plugged();
        let (_session, events) = start(&chip, config(0, false));
        connected(&events);

        chip.lock().unwrap().plugged = false;
        expect(
            &events,
            "wait",
            |event| matches!(event, Event::State(SwoState::Waiting { problem: Some(text) }) if text == "probe unplugged"),
        );
        chip.lock().unwrap().plugged = true;
        connected(&events);
        assert_eq!(chip.lock().unwrap().starts, 2);
    }

    #[test]
    fn start_problems_are_reported() {
        let chip = plugged();
        chip.lock().unwrap().start_problem = Some("Could not turn SWO on: no SWO".into());
        let (_session, events) = start(&chip, config(0, false));
        expect(
            &events,
            "the problem",
            |event| matches!(event, Event::State(SwoState::Waiting { problem: Some(text) }) if text.contains("no SWO")),
        );
    }

    #[test]
    fn a_wrong_clock_never_attaches() {
        let chip = plugged();
        let (_session, events) = start(
            &chip,
            SwoConfig {
                clock_hz: 0,
                ..config(0, false)
            },
        );
        expect(
            &events,
            "the problem",
            |event| matches!(event, Event::State(SwoState::Waiting { problem: Some(text) }) if text.contains("clock")),
        );
        assert_eq!(chip.lock().unwrap().starts, 0);
    }

    #[test]
    fn reset_resets_the_chip() {
        let chip = plugged();
        let (session, events) = start(&chip, config(0, false));
        connected(&events);
        session.reset();
        thread::sleep(Duration::from_millis(50));
        assert_eq!(chip.lock().unwrap().resets, 1);
    }

    #[test]
    fn dropping_the_session_ends_its_thread() {
        let chip = plugged();
        let (session, events) = start(&chip, config(0, false));
        connected(&events);
        drop(session);
        while events.try_recv().is_ok() {}
        assert!(matches!(
            events.recv_timeout(Duration::from_millis(50)),
            Err(RecvTimeoutError::Disconnected)
        ));
    }
}
