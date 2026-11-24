//! One RTT session: a chip attached through a probe on its own thread, found
//! again after the probe is unplugged or the chip is reset.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde::Serialize;

use super::config::RttConfig;
use super::link::{ChannelInfo, Drain, LinkProblem, RttBackend, RttLink};
use super::sysview;
use crate::rtos;
use crate::swo::trace::{Interrupt, Tracer};
use crate::watch::{Watched, Watcher};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RttState {
    /// Not reading yet. `problem` says why; without it, the probe is not plugged in.
    Waiting { problem: Option<String> },
    /// Reading the control block at `address` on `chip`.
    Connected { chip: String, address: u64 },
    /// Attached to `chip`, reading only the watched variables.
    Watching { chip: String },
}

/// What a session tells the app.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// Bytes from the up channel, in the order they came.
    Data(Vec<u8>),
    State(RttState),
    /// The channels the firmware set up, sent each time the control block is found.
    Channels {
        up: Vec<ChannelInfo>,
        down: Vec<ChannelInfo>,
    },
    /// The watched variables' values, in their order: `None` for one that
    /// could not be read, or is not in the firmware.
    Watch(Vec<Option<f64>>),
    /// The RTOS's tasks, as they were.
    Tasks(rtos::Snapshot),
    /// Which tasks ran, sampled, or every switch as SystemView tells: the
    /// switches since it was last told.
    Running(rtos::Running),
    /// The exceptions SystemView told since it was last told: each interrupt
    /// entered, left and returned to, at times in ms after the first, the
    /// whole ms since 1970 they count from.
    Interrupts(f64, Vec<Interrupt>),
    /// Whether the chip is halted, by the Live view's Pause, told when that changes.
    Halted(bool),
}

enum Command {
    Write(Vec<u8>),
    Reset,
    Reconfigure(RttConfig),
    Watch(Vec<Option<Watched>>, Duration),
    Tasks(Option<Arc<rtos::Layout>>, Duration, bool),
    Halt(bool),
    Close,
}

/// How often things happen. Tests make the times shorter.
#[derive(Clone, Copy)]
struct Timing {
    /// Between two tries to attach, and the longest between two tries to find the control block.
    retry: Duration,
    /// The first wait before the control block is looked for again. Each wait
    /// after it is twice as long, up to `retry`: firmware that is starting, as
    /// after a reset, sets it up within moments.
    search: Duration,
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
    search: Duration::from_millis(25),
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
            watcher: Watcher::default(),
            halted: false,
            tasks: (None, Duration::from_secs(1), false),
            sysview: None,
        };
        let thread = thread::Builder::new()
            .name("ternima-rtt".into())
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

    /// Reads these variables every `every` while the chip is attached, also
    /// before its RTT is found; `None` stands for one not in the firmware.
    /// An empty list stops it.
    pub fn watch(&self, watch: Vec<Option<Watched>>, every: Duration) {
        self.send(Command::Watch(watch, every));
    }

    /// Reads the tasks of the RTOS laid out so every `every` while the chip
    /// is attached, also before its RTT is found, and with `sample` which task
    /// runs, as fast as the probe answers. `None` stops it.
    pub fn tasks(&self, layout: Option<Arc<rtos::Layout>>, every: Duration, sample: bool) {
        self.send(Command::Tasks(layout, every, sample));
    }

    /// Halts the chip, or has it run again: the Live view's Pause. Ending
    /// the session runs it again too.
    pub fn halt(&self, halt: bool) {
        self.send(Command::Halt(halt));
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
    /// The firmware set the control block up again: read it again, still connected.
    Changed,
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
    /// The variables read as the chip runs, how often, and when next.
    watcher: Watcher,
    /// Whether the chip is halted, by the Live view's Pause.
    halted: bool,
    /// The RTOS's tasks as last asked for: its layout, how often, and
    /// whether the running task is sampled, for the timeline.
    tasks: (Option<Arc<rtos::Layout>>, Duration, bool),
    /// SystemView, while the firmware sends it.
    sysview: Option<SysView>,
}

/// SystemView read from its channel: every task switch and interrupt, which
/// the timeline then has instead of samples.
struct SysView {
    up: usize,
    down: usize,
    /// When it was last started: started again when no sync came.
    started: Instant,
    decoder: sysview::Decoder,
    to_trace: sysview::ToTrace,
    /// Once the firmware told its clock.
    tracer: Option<Tracer>,
    /// When its channel was read last.
    read_at: Instant,
}

/// How long SystemView may send with no sync before it is started again.
const SYSVIEW_SYNC_WAIT: Duration = Duration::from_secs(1);
/// How long a long read of memory, as of the tasks and their stacks, goes
/// on before SystemView's channel is read between its blocks. Switches
/// microseconds apart can take more than half of what a probe reads, as
/// 360 KB a second of an ST-Link V3's 600: read this often, the firmware's
/// buffer fills no more than between the reads of the channel alone, where
/// every 4 ms it filled near 8 KB.
const DRAIN_EVERY: Duration = Duration::from_millis(1);

/// Starts SystemView on its channels: stopped first, and what it sent read
/// away, so the sync and what the firmware is find room in its buffer: one
/// left recording, its buffer full, would drop them.
fn start_sysview(link: &mut dyn RttLink, up: usize, down: usize) -> Result<(), LinkProblem> {
    link.write(down, &[sysview::STOP])?;
    let mut away = vec![0; 16 * 1024];
    let until = Instant::now() + Duration::from_millis(50);
    while Instant::now() < until {
        link.read(up, &mut away)?;
    }
    link.write(down, &[sysview::START])?;
    Ok(())
}

/// The number of the channel of this name.
fn channel_named(channels: &[ChannelInfo], name: &str) -> Option<usize> {
    channels
        .iter()
        .find(|channel| channel.name.as_deref() == Some(name))
        .map(|channel| channel.number)
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

    /// Serves the chip attached, until the probe is lost or the session
    /// ends: never leaving it halted.
    fn serve(&mut self, mut link: Box<dyn RttLink>) -> Ended {
        let ended = if self.config.watch_only {
            self.serve_watch(link.as_mut())
        } else {
            self.serve_rtt(link.as_mut())
        };
        if self.halted {
            let _ = link.halt(false);
            self.ran_again();
        }
        ended
    }

    /// Reads the tasks, and samples which runs when asked: not while
    /// SystemView tells every switch.
    fn set_tasks(&mut self, layout: Option<Arc<rtos::Layout>>, every: Duration, sample: bool) {
        self.tasks = (layout, every, sample);
        self.apply_tasks();
    }

    fn apply_tasks(&mut self) {
        let (layout, every, sample) = self.tasks.clone();
        let traced = self
            .sysview
            .as_ref()
            .is_some_and(|sysview| sysview.tracer.is_some());
        self.watcher.set_tasks(layout, every, sample && !traced);
    }

    /// Reads the terminal's channel into `buf`, and SystemView's beside it
    /// into `sysview_buf` with the core taken once: how many bytes each gave.
    fn read_channels(
        &mut self,
        link: &mut dyn RttLink,
        buf: &mut [u8],
        sysview_buf: &mut [u8],
    ) -> Result<(usize, usize), LinkProblem> {
        let terminal = self.config.up_channel;
        let Some(sysview) = self.sysview.as_mut() else {
            return Ok((link.read(terminal, buf)?, 0));
        };
        // Read wrong, or its start lost: started again, for a new sync.
        if !sysview.decoder.synced() && sysview.started.elapsed() >= SYSVIEW_SYNC_WAIT {
            start_sysview(link, sysview.up, sysview.down)?;
            sysview.decoder.resync();
            sysview.started = Instant::now();
        }
        let (count, traced) = link.read_two((terminal, buf), (sysview.up, &mut *sysview_buf))?;
        sysview.read_at = Instant::now();
        self.take_sysview(&sysview_buf[..traced]);
        Ok((count, traced))
    }

    /// Takes in what SystemView sent, and tells what it traced every so often.
    fn take_sysview(&mut self, data: &[u8]) {
        let Some(sysview) = self.sysview.as_mut() else {
            return;
        };
        let mut started = false;
        let mut packets = Vec::new();
        for (event, delta) in sysview.decoder.feed(data) {
            match event {
                sysview::Event::Init { clock_hz } => {
                    let mut tracer = Tracer::new(clock_hz);
                    tracer.trace_switches(true);
                    sysview.tracer = Some(tracer);
                    started = true;
                }
                sysview::Event::Task { address, .. } => {
                    if let Some(tracer) = sysview.tracer.as_mut() {
                        tracer.know_task(address);
                    }
                }
                event => {
                    packets.clear();
                    sysview.to_trace.packets(&event, delta, &mut packets);
                    if let Some(tracer) = sysview.tracer.as_mut() {
                        for packet in &packets {
                            tracer.feed(packet);
                        }
                    }
                }
            }
        }
        let traced = sysview.tracer.as_mut().and_then(Tracer::take_if_due);
        // Every switch from now on: sampling would only take the probe's time.
        if started {
            self.apply_tasks();
        }
        if let Some(traced) = traced {
            if let Some(running) = traced.running {
                self.emit(Event::Running(running));
            }
            if !traced.interrupts.is_empty() {
                self.emit(Event::Interrupts(traced.base, traced.interrupts));
            }
        }
    }

    /// Halts the chip or has it run again, and tells it.
    fn halt(&mut self, link: &mut dyn RttLink, halt: bool) -> Result<(), LinkProblem> {
        if halt != self.halted {
            link.halt(halt)?;
            self.watcher.halt(halt);
            if let Some(tracer) = self
                .sysview
                .as_mut()
                .and_then(|sysview| sysview.tracer.as_mut())
            {
                tracer.halt(halt);
            }
            self.halted = halt;
            self.emit(Event::Halted(halt));
        }
        Ok(())
    }

    /// The chip runs again, as after a reset: told, if it was halted.
    fn ran_again(&mut self) {
        if self.halted {
            self.halted = false;
            self.watcher.halt(false);
            if let Some(tracer) = self
                .sysview
                .as_mut()
                .and_then(|sysview| sysview.tracer.as_mut())
            {
                tracer.halt(false);
            }
            self.emit(Event::Halted(false));
        }
    }

    /// Finds the control block, reads it, and finds it again after a reset,
    /// until the probe is lost or the session ends.
    fn serve_rtt(&mut self, link: &mut dyn RttLink) -> Ended {
        // How long to wait before looking for the control block again. While
        // the waits are short, only where it was last is looked at.
        let mut again = self.timing.search;
        loop {
            let attached = match link.find(&self.config, again < self.timing.retry) {
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
                    match self.wait_watching(link, again) {
                        Waited::Stop => return Ended::Closed,
                        Waited::Reopen => return Ended::Reopen,
                        Waited::Reset => {
                            if link.reset().is_err() {
                                return Ended::Lost;
                            }
                            self.ran_again();
                            again = self.timing.search;
                        }
                        Waited::Go => again = (again * 2).min(self.timing.retry),
                    }
                    continue;
                }
            };
            // When it goes, as after a reset, it is looked for soon again.
            again = self.timing.search;
            let sysview = channel_named(&attached.up, sysview::CHANNEL)
                .zip(channel_named(&attached.down, sysview::CHANNEL));

            self.emit(Event::Channels {
                up: attached.up,
                down: attached.down,
            });
            self.set_state(RttState::Connected {
                chip: self.config.chip.trim().to_string(),
                address: attached.address,
            });

            match self.pump(link, sysview) {
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
                // Found again at once, its channels told again; the tab stays connected.
                Pumped::Changed => {}
            }
        }
    }

    /// Reads only the watched variables, for firmware without RTT, until the
    /// probe is lost or the session ends.
    fn serve_watch(&mut self, link: &mut dyn RttLink) -> Ended {
        let watching = RttState::Watching {
            chip: self.config.chip.trim().to_string(),
        };
        self.set_state(watching.clone());
        loop {
            match self.wait_watching(link, self.timing.check) {
                Waited::Stop => return Ended::Closed,
                Waited::Reopen => return Ended::Reopen,
                Waited::Reset => {
                    if let Err(LinkProblem::Lost(problem)) = link.reset() {
                        self.set_state(RttState::Waiting {
                            problem: Some(problem),
                        });
                        return Ended::Lost;
                    }
                    self.ran_again();
                    // Told again, so the app knows the reset is done.
                    self.state = None;
                    self.set_state(watching.clone());
                }
                Waited::Go => {}
            }
            if let Err(LinkProblem::Lost(problem)) = link.reachable() {
                self.set_state(RttState::Waiting {
                    problem: Some(problem),
                });
                return Ended::Lost;
            }
        }
    }

    /// Passes data both ways until something stops it. With SystemView's
    /// channels, it is started, and read too.
    fn pump(&mut self, link: &mut dyn RttLink, sysview: Option<(usize, usize)>) -> Pumped {
        let mut buf = vec![0; 16 * 1024];
        let mut sysview_buf = vec![0; 16 * 1024];
        let mut pending = Vec::new();
        let mut batch_started: Option<Instant> = None;
        let mut checked = Instant::now();
        // Started: a sync, what the firmware is, and its tasks.
        if let Some((up, down)) = sysview
            && start_sysview(link, up, down).is_ok()
        {
            self.sysview = Some(SysView {
                up,
                down,
                started: Instant::now(),
                decoder: sysview::Decoder::new(),
                to_trace: sysview::ToTrace::default(),
                tracer: None,
                read_at: Instant::now(),
            });
        }

        let pumped = loop {
            if let Some(pumped) = self.do_commands(link) {
                break pumped;
            }

            let (count, traced) = match self.read_channels(link, &mut buf, &mut sysview_buf) {
                Ok(counts) => counts,
                Err(LinkProblem::Lost(problem)) => break Pumped::Lost(problem),
                Err(LinkProblem::NoControlBlock(_)) => break Pumped::Search,
            };
            if count > 0 {
                pending.extend_from_slice(&buf[..count]);
                batch_started.get_or_insert_with(Instant::now);
            }
            // A read found a channel's buffer moved: the firmware has set the
            // control block up again. Found again at once, nothing it writes
            // into its new buffers is lost.
            if link.moved() {
                break Pumped::Changed;
            }

            if batch_started.is_some_and(|start| start.elapsed() >= self.timing.batch) {
                self.emit(Event::Data(std::mem::take(&mut pending)));
                batch_started = None;
            }

            self.watch_now(link);

            if checked.elapsed() >= self.timing.check {
                checked = Instant::now();
                match link.still_there() {
                    Ok(true) => {}
                    Ok(false) | Err(LinkProblem::NoControlBlock(_)) => break Pumped::Search,
                    Err(LinkProblem::Lost(problem)) => break Pumped::Lost(problem),
                }
                match link.changed() {
                    Ok(false) => {}
                    Ok(true) => break Pumped::Changed,
                    Err(LinkProblem::NoControlBlock(_)) => break Pumped::Search,
                    Err(LinkProblem::Lost(problem)) => break Pumped::Lost(problem),
                }
            }

            // Read again at once while data flows or the running task is
            // sampled; rest a little otherwise.
            if count == 0 && traced == 0 && !self.watcher.sampling() {
                thread::sleep(self.timing.idle);
            }
        };

        if !pending.is_empty() {
            self.emit(Event::Data(pending));
        }
        // Stopped, so the firmware does not spend its time on it unread;
        // found again, it is started again.
        if let Some(sysview) = self.sysview.take() {
            let _ = link.write(sysview.down, &[sysview::STOP]);
            self.apply_tasks();
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
                    let reset = link.reset();
                    if reset.is_ok() {
                        self.ran_again();
                    }
                    return Some(match reset {
                        Ok(()) => Pumped::Search,
                        Err(LinkProblem::Lost(problem)) => Pumped::Lost(problem),
                        Err(LinkProblem::NoControlBlock(_)) => Pumped::Search,
                    });
                }
                Command::Halt(halt) => {
                    if let Err(LinkProblem::Lost(problem)) = self.halt(link, halt) {
                        return Some(Pumped::Lost(problem));
                    }
                }
                Command::Reconfigure(config) => {
                    self.config = config;
                    return Some(Pumped::Reopen);
                }
                Command::Watch(watch, every) => self.watcher.set(watch, every),
                Command::Tasks(layout, every, sample) => self.set_tasks(layout, every, sample),
                Command::Close => return Some(Pumped::Closed),
            }
        }
    }

    /// Reads the watched variables and the tasks when it is their time, and
    /// tells them: SystemView's channel is read between, so that its buffer
    /// does not fill meanwhile.
    fn watch_now(&mut self, link: &mut dyn RttLink) {
        if !self.watcher.due() {
            return;
        }
        let mut drained = Vec::new();
        let mut drain = self.sysview.as_ref().map(|sysview| Drain {
            channel: sysview.up,
            into: &mut drained,
            read_at: sysview.read_at,
            every: DRAIN_EVERY,
        });
        let watcher = &mut self.watcher;
        let (mut values, mut tasks, mut running) = (None, None, None);
        link.read_many(
            &mut |read| {
                values = watcher.read_if_due(&mut *read);
                tasks = watcher.tasks_if_due(&mut *read);
                running = watcher.running_if_due(&mut *read);
            },
            drain.as_mut(),
        );
        if let Some((drain, sysview)) = drain.zip(self.sysview.as_mut()) {
            sysview.read_at = drain.read_at;
        }
        if !drained.is_empty() {
            self.take_sysview(&drained);
        }
        if let Some(values) = values {
            self.emit(Event::Watch(values));
        }
        if let Some(tasks) = tasks {
            // SystemView names no idle task: it is the kernel's of that name.
            if let Some(sysview) = self.sysview.as_mut() {
                sysview.to_trace.idle = tasks
                    .tasks
                    .iter()
                    .find(|task| task.name.to_ascii_lowercase().starts_with("idle"))
                    .map(|task| task.address);
                if let Some(tracer) = sysview.tracer.as_mut() {
                    tracer.know(tasks.tasks.iter().map(|task| task.address));
                }
            }
            self.emit(Event::Tasks(tasks));
        }
        if let Some(running) = running {
            self.emit(Event::Running(running));
        }
    }

    /// Waits for the control block to come, still reading the watched
    /// variables and doing what the app asks.
    fn wait_watching(&mut self, link: &mut dyn RttLink, time: Duration) -> Waited {
        let until = Instant::now() + time;
        loop {
            self.watch_now(link);
            let now = Instant::now();
            if now >= until {
                return Waited::Go;
            }
            let mut left = until - now;
            if let Some(next) = self.watcher.next_in() {
                left = left.min(next);
            }
            match self.commands.recv_timeout(left) {
                Err(RecvTimeoutError::Timeout) => {}
                Ok(Command::Close) | Err(RecvTimeoutError::Disconnected) => return Waited::Stop,
                Ok(Command::Reconfigure(config)) => {
                    self.config = config;
                    return Waited::Reopen;
                }
                Ok(Command::Reset) => return Waited::Reset,
                Ok(Command::Watch(watch, every)) => self.watcher.set(watch, every),
                Ok(Command::Tasks(layout, every, sample)) => self.set_tasks(layout, every, sample),
                // A probe gone is found out by the next read.
                Ok(Command::Halt(halt)) => {
                    let _ = self.halt(link, halt);
                }
                // Nothing to send it to.
                Ok(Command::Write(_)) => {}
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
                // Kept for when the chip is attached.
                Ok(Command::Watch(watch, every)) => self.watcher.set(watch, every),
                Ok(Command::Tasks(layout, every, sample)) => self.set_tasks(layout, every, sample),
                // Nothing to send it to, or to halt.
                Ok(Command::Write(_) | Command::Halt(_)) => {}
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
    use crate::elf::Kind;

    const FAST: Timing = Timing {
        retry: Duration::from_millis(20),
        search: Duration::from_millis(5),
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
        /// RAM from 0x2000_0000, for the watched variables.
        ram: Vec<u8>,
        memory_reads: usize,
        /// Whether each look for the control block was a quick one.
        finds: Vec<bool>,
        /// What the firmware names its channels, Terminal when `None`.
        name: Option<String>,
        /// Which buffers the channels have: other ones once the firmware sets them up anew.
        buffers: u32,
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
                name: None,
                buffers: 0,
                moved: false,
            }))
        }
    }

    struct FakeLink {
        chip: Arc<Mutex<Chip>>,
        found: Option<u64>,
        /// The channels' name when the control block was found.
        name: Option<String>,
        /// The channels' buffers when it was found.
        buffers: u32,
        /// A read found other buffers since.
        moved: bool,
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
        fn find(&mut self, _config: &RttConfig, quick: bool) -> Result<Attached, LinkProblem> {
            let mut chip = self.chip()?;
            chip.finds.push(quick);
            let address = chip
                .control_block
                .ok_or_else(|| LinkProblem::NoControlBlock("no control block".into()))?;
            let name = chip.name.clone().unwrap_or_else(|| "Terminal".into());
            let buffers = chip.buffers;
            drop(chip);
            self.buffers = buffers;
            self.found = Some(address);
            self.name = Some(name.clone());
            self.moved = false;
            let terminal = ChannelInfo {
                number: 0,
                name: Some(name),
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
            // Read from the buffers found, what the firmware wrote into its new ones would be lost.
            if chip.buffers != self.buffers {
                drop(chip);
                self.moved = true;
                return Ok(0);
            }
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

        fn changed(&mut self) -> Result<bool, LinkProblem> {
            let now = self
                .chip()?
                .name
                .clone()
                .unwrap_or_else(|| "Terminal".into());
            Ok(self.name.as_ref() != Some(&now))
        }

        fn moved(&self) -> bool {
            self.moved
        }

        fn reset(&mut self) -> Result<(), LinkProblem> {
            self.chip()?.resets += 1;
            Ok(())
        }

        fn read_memory(&mut self, address: u64, buf: &mut [u8]) -> Result<(), LinkProblem> {
            let mut chip = self.chip()?;
            chip.memory_reads += 1;
            let at = address
                .checked_sub(0x2000_0000)
                .map(|at| at as usize)
                .filter(|at| at + buf.len() <= chip.ram.len())
                .ok_or_else(|| LinkProblem::Lost("no memory there".into()))?;
            buf.copy_from_slice(&chip.ram[at..at + buf.len()]);
            Ok(())
        }

        fn reachable(&mut self) -> Result<(), LinkProblem> {
            self.chip().map(|_| ())
        }
    }

    fn config() -> RttConfig {
        RttConfig {
            chip: "STM32H503RB".into(),
            ..RttConfig::default()
        }
    }

    fn start(chip: &Arc<Mutex<Chip>>, config: RttConfig) -> (Session, Receiver<Event>) {
        start_timed(chip, config, FAST)
    }

    fn start_timed(
        chip: &Arc<Mutex<Chip>>,
        config: RttConfig,
        timing: Timing,
    ) -> (Session, Receiver<Event>) {
        let (sender, events) = mpsc::channel();
        let session = Session::start_with(
            Arc::new(FakeBackend(chip.clone())),
            config,
            Box::new(move |event| {
                let _ = sender.send(event);
            }),
            timing,
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
    fn reads_the_channels_again_when_the_firmware_sets_them_up_anew() {
        // Found moments after a reset, the control block still had the last run's names.
        let chip = plugged_chip(Some(0x2000_0400));
        chip.lock().unwrap().name = Some("last run's".into());
        let (_session, events) = start(&chip, config());
        expect(
            &events,
            "the last run's channels",
            |event| matches!(event, Event::Channels { up, .. } if up[0].name.as_deref() == Some("last run's")),
        );
        connected(&events);

        chip.lock().unwrap().name = None;
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            let left = until.saturating_duration_since(Instant::now());
            let event = events
                .recv_timeout(left)
                .expect("the channels again within 5 s");
            assert!(
                !matches!(event, Event::State(RttState::Waiting { .. })),
                "the tab stays connected"
            );
            if let Event::Channels { up, .. } = event {
                assert_eq!(up[0].name.as_deref(), Some("Terminal"));
                break;
            }
        }
    }

    #[test]
    fn what_the_firmware_writes_as_it_sets_the_channels_up_anew_is_not_lost() {
        // Found moments after a reset, the control block still had the last run's buffers.
        let chip = plugged_chip(Some(0x2000_0400));
        chip.lock().unwrap().name = Some("last run's".into());
        // Checked seldom, so only a read can find the new buffers in time.
        let timing = Timing {
            check: Duration::from_secs(60),
            ..FAST
        };
        let (_session, events) = start_timed(&chip, config(), timing);
        connected(&events);

        {
            // The firmware sets RTT up, and says it booted at once.
            let mut chip = chip.lock().unwrap();
            chip.name = None;
            chip.buffers += 1;
            chip.up.extend_from_slice(b"*** Booting ***\r\n");
        }
        let until = Instant::now() + Duration::from_secs(5);
        let mut channels = false;
        let mut data = Vec::new();
        while data.len() < 17 {
            let left = until.saturating_duration_since(Instant::now());
            let event = events
                .recv_timeout(left)
                .expect("the channels and the data again within 5 s");
            match event {
                Event::State(RttState::Waiting { .. }) => panic!("the tab stays connected"),
                Event::Channels { up, .. } => {
                    assert_eq!(up[0].name.as_deref(), Some("Terminal"));
                    channels = true;
                }
                Event::Data(bytes) => {
                    assert!(channels, "read from the new buffers");
                    data.extend(bytes);
                }
                _ => {}
            }
        }
        assert_eq!(data, b"*** Booting ***\r\n");
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
    fn after_a_reset_the_control_block_is_found_again_at_once() {
        // Waits of up to 10 s, so only the short first ones can find it in time.
        let timing = Timing {
            retry: Duration::from_secs(10),
            ..FAST
        };
        let chip = plugged_chip(Some(0x2000_0400));
        let (session, events) = start_timed(&chip, config(), timing);
        connected(&events);

        // The firmware sets RTT up again a moment after the reset.
        chip.lock().unwrap().control_block = None;
        session.reset();
        waiting(&events);
        thread::sleep(Duration::from_millis(50));
        chip.lock().unwrap().control_block = Some(0x2000_0400);
        let back = Instant::now();
        connected(&events);
        assert!(
            back.elapsed() < Duration::from_secs(1),
            "found again after {:?}",
            back.elapsed()
        );
    }

    #[test]
    fn after_a_reset_it_is_looked_for_where_it_was_before_all_of_ram() {
        let chip = plugged_chip(Some(0x2000_0400));
        let (session, events) = start(&chip, config());
        connected(&events);
        chip.lock().unwrap().finds.clear();

        chip.lock().unwrap().control_block = None;
        session.reset();
        waiting(&events);
        thread::sleep(Duration::from_millis(200));
        let finds = chip.lock().unwrap().finds.clone();
        // Quick looks while the waits are short, then whole searches.
        assert_eq!(finds.first(), Some(&true), "{finds:?}");
        assert_eq!(finds.last(), Some(&false), "{finds:?}");
        assert!(finds.windows(2).all(|two| two[0] >= two[1]), "{finds:?}");
    }

    #[test]
    fn no_chip_chosen_is_reported() {
        let chip = plugged_chip(Some(0x2000_0400));
        let (_session, events) = start(&chip, RttConfig::default());
        assert!(waiting(&events).is_some_and(|text| text.contains("chip")));
        assert_eq!(chip.lock().unwrap().attaches, 0);
    }

    fn watched(events: &Receiver<Event>) -> Vec<Option<f64>> {
        match expect(events, "watched values", |event| {
            matches!(event, Event::Watch(_))
        }) {
            Event::Watch(values) => values,
            _ => unreachable!(),
        }
    }

    /// RAM with a u32 of 1000 at 0x2000_0000, an f32 of 21.5 at 0x2000_0004
    /// and an i16 of -2 far away at 0x2000_0400.
    fn ram() -> Vec<u8> {
        let mut ram = vec![0; 0x500];
        ram[0..4].copy_from_slice(&1000u32.to_le_bytes());
        ram[4..8].copy_from_slice(&21.5f32.to_le_bytes());
        ram[0x400..0x402].copy_from_slice(&(-2i16).to_le_bytes());
        ram
    }

    fn watch_all() -> Vec<Option<Watched>> {
        vec![
            Some(Watched {
                address: 0x2000_0400,
                kind: Kind::I16,
            }),
            Some(Watched {
                address: 0x2000_0000,
                kind: Kind::U32,
            }),
            None,
            Some(Watched {
                address: 0x2000_0004,
                kind: Kind::F32,
            }),
        ]
    }

    #[test]
    fn watched_variables_are_read_in_their_order() {
        let chip = plugged_chip(Some(0x2000_0400));
        chip.lock().unwrap().ram = ram();
        let (session, events) = start(&chip, config());
        connected(&events);

        session.watch(watch_all(), Duration::from_millis(10));
        assert_eq!(
            watched(&events),
            [Some(-2.0), Some(1000.0), None, Some(21.5)]
        );
        // The two side by side in one read, the one far away in another.
        let reads = chip.lock().unwrap().memory_reads;
        assert!(reads >= 2);
        chip.lock().unwrap().memory_reads = 0;
        watched(&events);
        assert_eq!(chip.lock().unwrap().memory_reads, 2);

        // They change as the firmware changes them.
        chip.lock().unwrap().ram[0..4].copy_from_slice(&1001u32.to_le_bytes());
        assert!(
            (0..10).any(|_| watched(&events)[1] == Some(1001.0)),
            "the new value never came"
        );
    }

    #[test]
    fn watched_variables_are_read_without_rtt_too() {
        let chip = plugged_chip(None);
        chip.lock().unwrap().ram = ram();
        let (session, events) = start(&chip, config());
        assert_eq!(waiting(&events).as_deref(), Some("no control block"));

        session.watch(watch_all(), Duration::from_millis(5));
        assert_eq!(watched(&events)[1], Some(1000.0));
    }

    fn watching(events: &Receiver<Event>) -> String {
        match expect(events, "watching", |event| {
            matches!(event, Event::State(RttState::Watching { .. }))
        }) {
            Event::State(RttState::Watching { chip }) => chip,
            _ => unreachable!(),
        }
    }

    fn watch_only() -> RttConfig {
        RttConfig {
            watch_only: true,
            ..config()
        }
    }

    #[test]
    fn with_rtt_off_only_the_watched_variables_are_read() {
        // A control block is there, but it is not looked for.
        let chip = plugged_chip(Some(0x2000_0400));
        chip.lock().unwrap().ram = ram();
        chip.lock().unwrap().up.extend_from_slice(b"hello");
        let (session, events) = start(&chip, watch_only());
        assert_eq!(watching(&events), "STM32H503RB");

        session.watch(watch_all(), Duration::from_millis(5));
        assert_eq!(
            watched(&events),
            [Some(-2.0), Some(1000.0), None, Some(21.5)]
        );
        thread::sleep(Duration::from_millis(50));
        assert!(
            events
                .try_iter()
                .all(|event| matches!(event, Event::Watch(_))),
            "nothing but values"
        );
        assert_eq!(chip.lock().unwrap().up, b"hello");
    }

    #[test]
    fn with_rtt_off_the_probe_is_found_again_after_it_is_unplugged() {
        let chip = plugged_chip(None);
        let (_session, events) = start(&chip, watch_only());
        watching(&events);

        chip.lock().unwrap().plugged = false;
        assert_eq!(waiting(&events).as_deref(), Some("probe unplugged"));
        chip.lock().unwrap().plugged = true;
        watching(&events);
        assert_eq!(chip.lock().unwrap().attaches, 2);
    }

    #[test]
    fn with_rtt_off_a_reset_says_it_is_watching_again() {
        let chip = plugged_chip(None);
        let (session, events) = start(&chip, watch_only());
        watching(&events);

        session.reset();
        watching(&events);
        assert_eq!(chip.lock().unwrap().resets, 1);
    }

    #[test]
    fn a_variable_that_cannot_be_read_has_no_value_and_the_chip_stays() {
        let chip = plugged_chip(Some(0x2000_0400));
        chip.lock().unwrap().ram = ram();
        let (session, events) = start(&chip, config());
        connected(&events);

        let outside = Watched {
            address: 0x3000_0000,
            kind: Kind::U32,
        };
        session.watch(
            vec![Some(outside), watch_all()[1]],
            Duration::from_millis(10),
        );
        assert_eq!(watched(&events), [None, Some(1000.0)]);
        assert_eq!(chip.lock().unwrap().attaches, 1);
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
