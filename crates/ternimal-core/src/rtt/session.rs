//! One RTT session: a chip attached through a probe on its own thread, found
//! again after the probe is unplugged or the chip is reset.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde::Serialize;

use super::config::RttConfig;
use super::link::{ChannelInfo, LinkProblem, RttBackend, RttLink};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RttState {
    /// Not reading yet. `problem` says why; without it, the probe is not plugged in.
    Waiting { problem: Option<String> },
    /// Reading the control block at `address` on `chip`.
    Connected { chip: String, address: u64 },
}

/// What a session tells the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// Bytes from the up channel, in the order they came.
    Data(Vec<u8>),
    State(RttState),
    /// The channels the firmware set up, sent each time the control block is found.
    Channels {
        up: Vec<ChannelInfo>,
        down: Vec<ChannelInfo>,
    },
}

enum Command {
    Write(Vec<u8>),
    Reset,
    Reconfigure(RttConfig),
    Close,
}

/// How often things happen. Tests make the times shorter.
#[derive(Clone, Copy)]
struct Timing {
    /// Between two tries to attach, or to find the control block.
    retry: Duration,
    /// The longest received bytes wait before they are passed on.
    batch: Duration,
    /// The pause between reads when nothing came.
    idle: Duration,
    /// Between two checks that the control block is still there.
    check: Duration,
    /// How long a write waits for room in a full down channel.
    write_wait: Duration,
}

const TIMING: Timing = Timing {
    retry: Duration::from_secs(1),
    batch: Duration::from_millis(16),
    idle: Duration::from_millis(2),
    check: Duration::from_secs(1),
    write_wait: Duration::from_millis(500),
};

/// An RTT session. Dropping it lets go of the probe.
pub struct Session {
    commands: Sender<Command>,
    thread: Option<JoinHandle<()>>,
}

impl Session {
    /// Starts attaching and keeps reading until the session is dropped.
    /// Everything that happens comes back through `on_event`, on the session's own thread.
    pub fn start(
        backend: Arc<dyn RttBackend>,
        config: RttConfig,
        on_event: impl FnMut(Event) + Send + 'static,
    ) -> Self {
        Self::start_with(backend, config, Box::new(on_event), TIMING)
    }

    fn start_with(
        backend: Arc<dyn RttBackend>,
        config: RttConfig,
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
            .name("ternimal-rtt".into())
            .spawn(move || worker.run())
            .expect("the OS could not start a thread");
        Self {
            commands,
            thread: Some(thread),
        }
    }

    /// Sends bytes to the down channel. They are dropped while not connected.
    pub fn write(&self, data: Vec<u8>) {
        self.send(Command::Write(data));
    }

    /// Resets the chip. The control block is then looked for again.
    pub fn reset(&self) {
        self.send(Command::Reset);
    }

    /// Attaches again with new settings.
    pub fn reconfigure(&self, config: RttConfig) {
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

/// Why serving an attached chip ended.
enum Ended {
    Closed,
    Lost,
    Reopen,
}

/// Why reading ended.
enum Pumped {
    Closed,
    Reopen,
    Lost(String),
    /// The control block is gone, as after a reset: look for it again.
    Search,
}

/// How a wait ended.
enum Waited {
    Go,
    Stop,
    Reopen,
    Reset,
}

struct Worker {
    backend: Arc<dyn RttBackend>,
    config: RttConfig,
    on_event: Box<dyn FnMut(Event) + Send>,
    commands: Receiver<Command>,
    timing: Timing,
    /// The state last told to the app, so each change is told once.
    state: Option<RttState>,
}

impl Worker {
    fn run(mut self) {
        loop {
            if let Some(problem) = self.config.problem() {
                self.set_state(RttState::Waiting {
                    problem: Some(problem),
                });
                if matches!(self.wait(self.timing.retry), Waited::Stop) {
                    return;
                }
                continue;
            }
            match self.backend.attach(&self.config) {
                Ok(link) => match self.serve(link) {
                    Ended::Closed => return,
                    Ended::Reopen => continue,
                    // Give the probe a moment before trying again.
                    Ended::Lost => {
                        if matches!(self.wait(self.timing.retry), Waited::Stop) {
                            return;
                        }
                    }
                },
                Err(problem) => {
                    self.set_state(RttState::Waiting { problem });
                    if matches!(self.wait(self.timing.retry), Waited::Stop) {
                        return;
                    }
                }
            }
        }
    }

    /// Finds the control block, reads it, and finds it again after a reset,
    /// until the probe is lost or the session ends.
    fn serve(&mut self, mut link: Box<dyn RttLink>) -> Ended {
        loop {
            let attached = match link.find(&self.config) {
                Ok(attached) => attached,
                Err(LinkProblem::Lost(problem)) => {
                    self.set_state(RttState::Waiting {
                        problem: Some(problem),
                    });
                    return Ended::Lost;
                }
                Err(LinkProblem::NoControlBlock(problem)) => {
                    self.set_state(RttState::Waiting {
                        problem: Some(problem),
                    });
                    match self.wait(self.timing.retry) {
                        Waited::Stop => return Ended::Closed,
                        Waited::Reopen => return Ended::Reopen,
                        Waited::Reset => {
                            if link.reset().is_err() {
                                return Ended::Lost;
                            }
                        }
                        Waited::Go => {}
                    }
                    continue;
                }
            };

            self.emit(Event::Channels {
                up: attached.up,
                down: attached.down,
            });
            self.set_state(RttState::Connected {
                chip: self.config.chip.trim().to_string(),
                address: attached.address,
            });

            match self.pump(link.as_mut()) {
                Pumped::Closed => return Ended::Closed,
                Pumped::Reopen => return Ended::Reopen,
                Pumped::Lost(problem) => {
                    self.set_state(RttState::Waiting {
                        problem: Some(problem),
                    });
                    return Ended::Lost;
                }
                Pumped::Search => self.set_state(RttState::Waiting {
                    problem: Some("Looking for the RTT control block again.".into()),
                }),
            }
        }
    }

    /// Passes data both ways until something stops it.
    fn pump(&mut self, link: &mut dyn RttLink) -> Pumped {
        let mut buf = vec![0; 16 * 1024];
        let mut pending = Vec::new();
        let mut batch_started: Option<Instant> = None;
        let mut checked = Instant::now();

        let pumped = loop {
            if let Some(pumped) = self.do_commands(link) {
                break pumped;
            }

            let count = match link.read(self.config.up_channel, &mut buf) {
                Ok(count) => count,
                Err(LinkProblem::Lost(problem)) => break Pumped::Lost(problem),
                Err(LinkProblem::NoControlBlock(_)) => break Pumped::Search,
            };
            if count > 0 {
                pending.extend_from_slice(&buf[..count]);
                batch_started.get_or_insert_with(Instant::now);
            }
            if batch_started.is_some_and(|start| start.elapsed() >= self.timing.batch) {
                self.emit(Event::Data(std::mem::take(&mut pending)));
                batch_started = None;
            }

            if checked.elapsed() >= self.timing.check {
                checked = Instant::now();
                match link.still_there() {
                    Ok(true) => {}
                    Ok(false) | Err(LinkProblem::NoControlBlock(_)) => break Pumped::Search,
                    Err(LinkProblem::Lost(problem)) => break Pumped::Lost(problem),
                }
            }

            // Read again at once while data flows; rest a little when it does not.
            if count == 0 {
                thread::sleep(self.timing.idle);
            }
        };

        if !pending.is_empty() {
            self.emit(Event::Data(pending));
        }
        pumped
    }

    /// Does what the app asked, until no command is left.
    fn do_commands(&mut self, link: &mut dyn RttLink) -> Option<Pumped> {
        loop {
            let command = match self.commands.try_recv() {
                Ok(command) => command,
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => return Some(Pumped::Closed),
            };
            match command {
                Command::Write(data) => {
                    if let Err(problem) = self.write_all(link, &data) {
                        return Some(match problem {
                            LinkProblem::Lost(problem) => Pumped::Lost(problem),
                            LinkProblem::NoControlBlock(_) => Pumped::Search,
                        });
                    }
                }
                Command::Reset => {
                    return Some(match link.reset() {
                        Ok(()) => Pumped::Search,
                        Err(LinkProblem::Lost(problem)) => Pumped::Lost(problem),
                        Err(LinkProblem::NoControlBlock(_)) => Pumped::Search,
                    });
                }
                Command::Reconfigure(config) => {
                    self.config = config;
                    return Some(Pumped::Reopen);
                }
                Command::Close => return Some(Pumped::Closed),
            }
        }
    }

    /// Writes everything, waiting a little for room when the firmware is slow
    /// to empty its down channel. What does not fit in time is dropped.
    fn write_all(&mut self, link: &mut dyn RttLink, mut data: &[u8]) -> Result<(), LinkProblem> {
        let until = Instant::now() + self.timing.write_wait;
        while !data.is_empty() && Instant::now() < until {
            let written = link.write(self.config.down_channel, data)?;
            data = &data[written..];
            if written == 0 {
                thread::sleep(Duration::from_millis(1));
            }
        }
        Ok(())
    }

    /// Waits while not reading, still doing what the app asks.
    fn wait(&mut self, time: Duration) -> Waited {
        let until = Instant::now() + time;
        loop {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Waited::Go;
            }
            match self.commands.recv_timeout(left) {
                Err(RecvTimeoutError::Timeout) => return Waited::Go,
                Ok(Command::Close) | Err(RecvTimeoutError::Disconnected) => return Waited::Stop,
                Ok(Command::Reconfigure(config)) => {
                    self.config = config;
                    return Waited::Reopen;
                }
                Ok(Command::Reset) => return Waited::Reset,
                // Nothing to send it to.
                Ok(Command::Write(_)) => {}
            }
        }
    }

    fn set_state(&mut self, state: RttState) {
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

    use super::super::link::Attached;
    use super::*;

    const FAST: Timing = Timing {
        retry: Duration::from_millis(20),
        batch: Duration::from_millis(5),
        idle: Duration::from_millis(1),
        check: Duration::from_millis(20),
        write_wait: Duration::from_millis(50),
    };

    /// A pretend probe and chip, which tests change while the session runs.
    #[derive(Default)]
    struct Chip {
        plugged: bool,
        attach_problem: Option<String>,
        /// Where the control block is; `None` until the firmware sets it up.
        control_block: Option<u64>,
        up: Vec<u8>,
        down: Vec<u8>,
        resets: usize,
        attaches: usize,
    }

    #[derive(Default)]
    struct FakeBackend(Arc<Mutex<Chip>>);

    impl RttBackend for FakeBackend {
        fn attach(&self, _config: &RttConfig) -> Result<Box<dyn RttLink>, Option<String>> {
            let mut chip = self.0.lock().unwrap();
            if !chip.plugged {
                return Err(None);
            }
            if let Some(problem) = &chip.attach_problem {
                return Err(Some(problem.clone()));
            }
            chip.attaches += 1;
            Ok(Box::new(FakeLink {
                chip: self.0.clone(),
                found: None,
            }))
        }
    }

    struct FakeLink {
        chip: Arc<Mutex<Chip>>,
        found: Option<u64>,
    }

    impl FakeLink {
        fn chip(&self) -> Result<std::sync::MutexGuard<'_, Chip>, LinkProblem> {
            let chip = self.chip.lock().unwrap();
            if chip.plugged {
                Ok(chip)
            } else {
                Err(LinkProblem::Lost("probe unplugged".into()))
            }
        }
    }

    impl RttLink for FakeLink {
        fn find(&mut self, _config: &RttConfig) -> Result<Attached, LinkProblem> {
            let address = self
                .chip()?
                .control_block
                .ok_or_else(|| LinkProblem::NoControlBlock("no control block".into()))?;
            self.found = Some(address);
            let terminal = ChannelInfo {
                number: 0,
                name: Some("Terminal".into()),
                size: 1024,
            };
            Ok(Attached {
                address,
                up: vec![terminal.clone()],
                down: vec![terminal],
            })
        }

        fn read(&mut self, _channel: usize, buf: &mut [u8]) -> Result<usize, LinkProblem> {
            let mut chip = self.chip()?;
            let count = chip.up.len().min(buf.len());
            buf[..count].copy_from_slice(&chip.up[..count]);
            chip.up.drain(..count);
            Ok(count)
        }

        fn write(&mut self, _channel: usize, data: &[u8]) -> Result<usize, LinkProblem> {
            // A small channel, so long writes take more than one go.
            let count = data.len().min(8);
            self.chip()?.down.extend_from_slice(&data[..count]);
            Ok(count)
        }

        fn still_there(&mut self) -> Result<bool, LinkProblem> {
            let found = self.found;
            Ok(self.chip()?.control_block.is_some() && self.chip()?.control_block == found)
        }

        fn reset(&mut self) -> Result<(), LinkProblem> {
            self.chip()?.resets += 1;
            Ok(())
        }
    }

    fn config() -> RttConfig {
        RttConfig {
            chip: "STM32H503RB".into(),
            ..RttConfig::default()
        }
    }

    fn start(chip: &Arc<Mutex<Chip>>, config: RttConfig) -> (Session, Receiver<Event>) {
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

    fn plugged_chip(control_block: Option<u64>) -> Arc<Mutex<Chip>> {
        Arc::new(Mutex::new(Chip {
            plugged: true,
            control_block,
            ..Chip::default()
        }))
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

    fn connected(events: &Receiver<Event>) -> u64 {
        match expect(events, "connect", |event| {
            matches!(event, Event::State(RttState::Connected { .. }))
        }) {
            Event::State(RttState::Connected { address, .. }) => address,
            _ => unreachable!(),
        }
    }

    fn waiting(events: &Receiver<Event>) -> Option<String> {
        match expect(events, "wait", |event| {
            matches!(event, Event::State(RttState::Waiting { .. }))
        }) {
            Event::State(RttState::Waiting { problem }) => problem,
            _ => unreachable!(),
        }
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

    #[test]
    fn reads_the_up_channel_and_writes_the_down_channel() {
        let chip = plugged_chip(Some(0x2000_0400));
        let (session, events) = start(&chip, config());

        let channels = expect(&events, "channels", |event| {
            matches!(event, Event::Channels { .. })
        });
        assert!(
            matches!(channels, Event::Channels { up, .. } if up[0].name.as_deref() == Some("Terminal"))
        );
        assert_eq!(connected(&events), 0x2000_0400);

        chip.lock().unwrap().up.extend_from_slice(b"uart:~$ ");
        assert_eq!(received(&events, 8), b"uart:~$ ");

        // Longer than the pretend channel takes at once.
        session.write(b"kernel threads\r".to_vec());
        thread::sleep(Duration::from_millis(100));
        assert_eq!(chip.lock().unwrap().down, b"kernel threads\r");
    }

    #[test]
    fn waits_for_the_probe() {
        let chip = Arc::new(Mutex::new(Chip::default()));
        let (_session, events) = start(&chip, config());
        assert_eq!(waiting(&events), None);

        {
            let mut chip = chip.lock().unwrap();
            chip.plugged = true;
            chip.control_block = Some(0x2000_0000);
        }
        connected(&events);
    }

    #[test]
    fn waits_for_the_firmware_to_set_up_rtt() {
        let chip = plugged_chip(None);
        let (_session, events) = start(&chip, config());
        assert_eq!(waiting(&events).as_deref(), Some("no control block"));

        chip.lock().unwrap().control_block = Some(0x2000_0800);
        assert_eq!(connected(&events), 0x2000_0800);
        // The probe was attached only once: only the search was repeated.
        assert_eq!(chip.lock().unwrap().attaches, 1);
    }

    #[test]
    fn finds_the_control_block_again_after_new_firmware() {
        let chip = plugged_chip(Some(0x2000_0400));
        let (_session, events) = start(&chip, config());
        connected(&events);

        chip.lock().unwrap().control_block = Some(0x2000_1000);
        assert_eq!(connected(&events), 0x2000_1000);
    }

    #[test]
    fn attaches_again_after_the_probe_comes_back() {
        let chip = plugged_chip(Some(0x2000_0400));
        let (_session, events) = start(&chip, config());
        connected(&events);

        chip.lock().unwrap().plugged = false;
        assert_eq!(waiting(&events).as_deref(), Some("probe unplugged"));

        chip.lock().unwrap().plugged = true;
        connected(&events);
        assert_eq!(chip.lock().unwrap().attaches, 2);
    }

    #[test]
    fn attach_problems_are_reported() {
        let chip = plugged_chip(Some(0x2000_0400));
        chip.lock().unwrap().attach_problem = Some("Could not attach to STM32H503RB".into());
        let (_session, events) = start(&chip, config());
        assert_eq!(
            waiting(&events).as_deref(),
            Some("Could not attach to STM32H503RB")
        );
    }

    #[test]
    fn reset_resets_the_chip_and_looks_again() {
        let chip = plugged_chip(Some(0x2000_0400));
        let (session, events) = start(&chip, config());
        connected(&events);

        session.reset();
        let problem = waiting(&events);
        assert!(problem.is_some_and(|text| text.contains("again")));
        connected(&events);
        assert_eq!(chip.lock().unwrap().resets, 1);
    }

    #[test]
    fn no_chip_chosen_is_reported() {
        let chip = plugged_chip(Some(0x2000_0400));
        let (_session, events) = start(&chip, RttConfig::default());
        assert!(waiting(&events).is_some_and(|text| text.contains("chip")));
        assert_eq!(chip.lock().unwrap().attaches, 0);
    }

    #[test]
    fn dropping_the_session_ends_its_thread() {
        let chip = plugged_chip(Some(0x2000_0400));
        let (session, events) = start(&chip, config());
        connected(&events);
        drop(session);
        while events.try_recv().is_ok() {}
        assert!(matches!(
            events.recv_timeout(Duration::from_millis(50)),
            Err(RecvTimeoutError::Disconnected)
        ));
    }
}
