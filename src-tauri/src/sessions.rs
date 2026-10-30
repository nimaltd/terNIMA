//! The session commands the UI calls, for serial ports, RTT, SWO, TCP and
//! recordings played back, the sessions they run, and the log file and
//! recording each can write.

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Manager, State};
use ternima_core::lines::{self, LineSplitter};
use ternima_core::logfile::{LogSetup, Logger};
use ternima_core::record::{self, Direction, Recorder};
use ternima_core::replay::{self, Player};
use ternima_core::send::{self, Pace, Stopped};
use ternima_core::{elf, net, probe, rtt, serial, swo};

/// The most time, in ms, that lines read at once are spread over, the same
/// as SPREAD_MS in tabs.svelte.ts, which does it for live sessions.
const SPREAD_MS: f64 = 200.0;

/// A running session of any kind.
enum Running {
    Serial(serial::Session),
    Rtt(rtt::Session),
    Swo(swo::Session),
    Tcp(net::Session),
    Replay(Player),
}

impl Running {
    /// Whether bytes can be sent: not to SWO, which only receives, nor to a recording.
    fn takes_input(&self) -> bool {
        !matches!(self, Running::Swo(_) | Running::Replay(_))
    }

    /// Sends bytes to the device.
    fn write(&self, data: Vec<u8>) {
        match self {
            Running::Serial(session) => session.write(data),
            Running::Rtt(session) => session.write(data),
            Running::Tcp(session) => session.write(data),
            Running::Swo(_) | Running::Replay(_) => {}
        }
    }
}

/// A session's log and recording, shared between its thread and the commands.
#[derive(Default)]
struct LogState {
    logger: Option<Logger>,
    recorder: Option<Recorder>,
    /// What the session is connected to now, for the first line of a log
    /// turned on while it runs.
    connected_to: Option<String>,
}

type SharedLog = Arc<Mutex<LogState>>;

fn lock_log(log: &SharedLog) -> MutexGuard<'_, LogState> {
    log.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A running session and its log.
struct Entry {
    running: Running,
    log: SharedLog,
    /// Set while a file is being sent; setting the flag stops it.
    sending: Option<Arc<AtomicBool>>,
}

/// The running sessions, by the number the UI gave each.
#[derive(Default)]
pub struct Sessions(Mutex<HashMap<u32, Entry>>);

impl Sessions {
    fn lock(&self) -> MutexGuard<'_, HashMap<u32, Entry>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Runs `action` on a session, if it is running.
    fn with(&self, id: u32, action: impl FnOnce(&Running)) {
        if let Some(entry) = self.lock().get(&id) {
            action(&entry.running);
        }
    }

    /// Sends bytes to a session, and records them if it records. False when
    /// it is not running, or only receives.
    fn write(&self, id: u32, data: Vec<u8>) -> bool {
        let all = self.lock();
        let Some(entry) = all.get(&id).filter(|entry| entry.running.takes_input()) else {
            return false;
        };
        let mut state = lock_log(&entry.log);
        // A recording that cannot be written stops.
        if let Some(recorder) = &mut state.recorder
            && recorder.data(Direction::Sent, &data).is_err()
        {
            state.recorder = None;
        }
        drop(state);
        entry.running.write(data);
        true
    }

    /// Adds a session. One with the same number is replaced; closing it can
    /// take a moment, so it happens after the lock is let go.
    fn insert(&self, id: u32, running: Running, log: SharedLog) {
        let entry = Entry {
            running,
            log,
            sending: None,
        };
        let old = self.lock().insert(id, entry);
        drop(old);
    }
}

/// What a session tells the UI, apart from the data it receives.
#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionEvent {
    State(serial::SessionState),
    Lines(serial::ModemLines),
    RttState(rtt::RttState),
    Channels {
        up: Vec<rtt::ChannelInfo>,
        down: Vec<rtt::ChannelInfo>,
    },
    SwoState(swo::SwoState),
    TcpState(net::NetState),
    /// The lines of text the last bytes finished, for the Lines view and the
    /// plot, with when those bytes were read: ms since 1970.
    TextLines {
        lines: Vec<lines::Line>,
        at: f64,
    },
    /// What a recording played back next, in order, with its lines and when
    /// each came, in ms since 1970. It all comes on this one channel, so it
    /// keeps its order with `Restarted`.
    Replayed {
        pieces: Vec<Piece>,
        lines: Vec<lines::Line>,
        times: Vec<f64>,
    },
    /// A recording plays from the start again: what it showed goes.
    Restarted,
    ReplayState(replay::PlayState),
    /// Writing the recording failed, so it stopped.
    RecordingStopped {
        problem: String,
    },
    /// The chip's trace FIFO overflowed this many times since the last report.
    Overflow {
        count: u32,
    },
    /// Writing the log failed, so it was turned off.
    LogStopped {
        problem: String,
    },
    /// The watched variables' values, in the order the Watch view asked for
    /// them: `null` for one that could not be read. With when they were read,
    /// in ms since 1970.
    Watch {
        values: Vec<Option<f64>>,
        at: f64,
    },
}

/// Bytes a recording played back, as they were received or sent.
#[derive(Clone, Serialize)]
pub struct Piece {
    sent: bool,
    bytes: Vec<u8>,
    /// When it came, in ms since 1970.
    at: f64,
}

/// Whether a log is being written, and where; or why it could not start.
/// The same for a recording.
#[derive(Clone, Default, Serialize)]
pub struct LogStatus {
    path: Option<String>,
    problem: Option<String>,
}

/// The folder logs go to when the settings leave it empty: "ternima logs" in Documents.
fn default_folder(app: &AppHandle) -> PathBuf {
    let documents = app
        .path()
        .document_dir()
        .or_else(|_| app.path().home_dir())
        .unwrap_or_else(|_| PathBuf::from("."));
    documents.join("ternima logs")
}

/// Starts a log for a session, if the UI asked for one.
fn start_log(
    app: &AppHandle,
    setup: Option<LogSetup>,
    connected_to: Option<&str>,
) -> (Option<Logger>, LogStatus) {
    let Some(mut setup) = setup else {
        return (None, LogStatus::default());
    };
    if setup.folder.as_os_str().is_empty() {
        setup.folder = default_folder(app);
    }
    let started = Logger::create(&setup).and_then(|mut logger| {
        if let Some(what) = connected_to {
            logger.connected(what)?;
        }
        Ok(logger)
    });
    match started {
        Ok(logger) => {
            let path = Some(logger.path().display().to_string());
            (
                Some(logger),
                LogStatus {
                    path,
                    problem: None,
                },
            )
        }
        Err(error) => {
            let problem = format!(
                "Could not start a log in {}: {error}",
                setup.folder.display()
            );
            (
                None,
                LogStatus {
                    path: None,
                    problem: Some(problem),
                },
            )
        }
    }
}

/// Writes to a session's log. A failed write turns the log off and tells the UI why.
fn write_log(
    log: &SharedLog,
    events: &Channel<SessionEvent>,
    write: impl FnOnce(&mut LogState) -> io::Result<()>,
) {
    let mut state = lock_log(log);
    if let Err(error) = write(&mut state) {
        let path = state
            .logger
            .as_ref()
            .map(|logger| logger.path().display().to_string())
            .unwrap_or_default();
        state.logger = None;
        let _ = events.send(SessionEvent::LogStopped {
            problem: format!("Logging stopped: could not write {path}: {error}"),
        });
    }
}

/// Sends the lines that received bytes finished, for the Lines view and the
/// plot. They are timed here, as the bytes are read, not when the UI gets them.
fn send_lines(splitter: &mut LineSplitter, events: &Channel<SessionEvent>, bytes: &[u8]) {
    let lines = splitter.feed(bytes);
    if !lines.is_empty() {
        let at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0.0, |since| since.as_secs_f64() * 1000.0);
        let _ = events.send(SessionEvent::TextLines { lines, at });
    }
}

/// Writes to a session's recording. A failed write stops it and tells the UI why.
fn write_record(
    log: &SharedLog,
    events: &Channel<SessionEvent>,
    write: impl FnOnce(&mut Recorder) -> io::Result<()>,
) {
    let mut state = lock_log(log);
    let Some(recorder) = &mut state.recorder else {
        return;
    };
    if let Err(error) = write(recorder) {
        let path = recorder.path().display().to_string();
        state.recorder = None;
        let _ = events.send(SessionEvent::RecordingStopped {
            problem: format!("Recording stopped: could not write {path}: {error}"),
        });
    }
}

fn log_data(log: &SharedLog, events: &Channel<SessionEvent>, bytes: &[u8]) {
    write_log(log, events, |state| match &mut state.logger {
        Some(logger) => logger.data(bytes),
        None => Ok(()),
    });
    write_record(log, events, |recorder| {
        recorder.data(Direction::Received, bytes)
    });
}

/// Notes a connect, with what it connected to, or a disconnect.
fn log_state(log: &SharedLog, events: &Channel<SessionEvent>, connected_to: Option<String>) {
    let note = match &connected_to {
        Some(what) => format!("connected to {what}"),
        None => "disconnected".to_string(),
    };
    write_record(log, events, |recorder| recorder.note(&note));
    write_log(log, events, |state| {
        state.connected_to = connected_to.clone();
        match (&mut state.logger, connected_to) {
            (Some(logger), Some(what)) => logger.connected(&what),
            (Some(logger), None) => logger.disconnected(),
            (None, _) => Ok(()),
        }
    });
}

/// A new session's shared log, started if the UI asked for one.
fn new_log(app: &AppHandle, setup: Option<LogSetup>) -> (SharedLog, LogStatus) {
    let (logger, status) = start_log(app, setup, None);
    let state = LogState {
        logger,
        recorder: None,
        connected_to: None,
    };
    (Arc::new(Mutex::new(state)), status)
}

#[tauri::command(async)]
pub fn list_ports() -> Vec<serial::PortInfo> {
    serial::list_ports()
}

#[tauri::command(async)]
pub fn list_probes() -> Vec<probe::ProbeInfo> {
    probe::list_probes()
}

#[tauri::command(async)]
pub fn search_chips(text: String, limit: usize) -> Vec<String> {
    probe::search_chips(&text, limit)
}

/// Finds out which chip is on the board through a probe. Only reads, for STM32.
#[tauri::command(async)]
pub fn detect_chip(
    probe: Option<probe::ProbeId>,
    speed_khz: u32,
) -> Result<ternima_core::detect::Detected, String> {
    ternima_core::detect::detect(probe.as_ref(), speed_khz)
}

/// The folder logs go to when the settings leave it empty.
#[tauri::command]
pub fn default_log_folder(app: AppHandle) -> String {
    default_folder(&app).display().to_string()
}

/// Starts a serial session. Received bytes go to `data` as raw bytes, and to
/// the log if `log` asks for one; everything else goes to `events`.
#[tauri::command(async)]
#[allow(clippy::too_many_arguments)]
pub fn open_session(
    id: u32,
    port: serial::PortId,
    config: serial::SerialConfig,
    log: Option<LogSetup>,
    data: Channel<InvokeResponseBody>,
    events: Channel<SessionEvent>,
    app: AppHandle,
    sessions: State<Sessions>,
) -> LogStatus {
    let (shared, status) = new_log(&app, log);
    let log = shared.clone();
    let mut splitter = LineSplitter::default();
    let session =
        serial::Session::start(Arc::new(serial::SystemPorts), port, config, move |event| {
            // A send only fails when the window is gone, and then nobody is listening.
            let _ = match event {
                serial::Event::Data(bytes) => {
                    log_data(&log, &events, &bytes);
                    send_lines(&mut splitter, &events, &bytes);
                    data.send(InvokeResponseBody::Raw(bytes))
                }
                serial::Event::State(state) => {
                    let connected_to = match &state {
                        serial::SessionState::Connected { port } => Some(port.name.clone()),
                        serial::SessionState::Waiting { .. } => None,
                    };
                    log_state(&log, &events, connected_to);
                    events.send(SessionEvent::State(state))
                }
                serial::Event::Lines(lines) => events.send(SessionEvent::Lines(lines)),
            };
        });
    sessions.insert(id, Running::Serial(session), shared);
    status
}

/// Starts an RTT session, with the same channels and log as a serial one.
#[tauri::command(async)]
pub fn open_rtt_session(
    id: u32,
    config: rtt::RttConfig,
    log: Option<LogSetup>,
    data: Channel<InvokeResponseBody>,
    events: Channel<SessionEvent>,
    app: AppHandle,
    sessions: State<Sessions>,
) -> LogStatus {
    let (shared, status) = new_log(&app, log);
    let log = shared.clone();
    let mut splitter = LineSplitter::default();
    let session = rtt::Session::start(Arc::new(rtt::ProbeRs), config, move |event| {
        let _ = match event {
            rtt::Event::Data(bytes) => {
                log_data(&log, &events, &bytes);
                send_lines(&mut splitter, &events, &bytes);
                data.send(InvokeResponseBody::Raw(bytes))
            }
            rtt::Event::State(state) => {
                let connected_to = match &state {
                    rtt::RttState::Connected { chip, address } => {
                        Some(format!("{chip}, RTT at {address:#010x}"))
                    }
                    rtt::RttState::Waiting { .. } => None,
                };
                log_state(&log, &events, connected_to);
                events.send(SessionEvent::RttState(state))
            }
            rtt::Event::Channels { up, down } => events.send(SessionEvent::Channels { up, down }),
            rtt::Event::Watch(values) => {
                let at = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0.0, |since| since.as_secs_f64() * 1000.0);
                events.send(SessionEvent::Watch { values, at })
            }
        };
    });
    sessions.insert(id, Running::Rtt(session), shared);
    status
}

/// Starts an SWO session, with the same channels and log as the others. It only receives.
#[tauri::command(async)]
pub fn open_swo_session(
    id: u32,
    config: swo::SwoConfig,
    log: Option<LogSetup>,
    data: Channel<InvokeResponseBody>,
    events: Channel<SessionEvent>,
    app: AppHandle,
    sessions: State<Sessions>,
) -> LogStatus {
    let (shared, status) = new_log(&app, log);
    let log = shared.clone();
    let mut splitter = LineSplitter::default();
    let session = swo::Session::start(Arc::new(swo::ProbeRs), config, move |event| {
        let _ = match event {
            swo::Event::Data(bytes) => {
                log_data(&log, &events, &bytes);
                send_lines(&mut splitter, &events, &bytes);
                data.send(InvokeResponseBody::Raw(bytes))
            }
            swo::Event::State(state) => {
                let connected_to = match &state {
                    swo::SwoState::Connected { chip, baud } => {
                        Some(format!("{chip}, SWO at {baud} baud"))
                    }
                    swo::SwoState::Waiting { .. } => None,
                };
                log_state(&log, &events, connected_to);
                events.send(SessionEvent::SwoState(state))
            }
            swo::Event::Overflow(count) => events.send(SessionEvent::Overflow { count }),
        };
    });
    sessions.insert(id, Running::Swo(session), shared);
    status
}

/// Starts a TCP session: a plain socket, Telnet, or J-Link's or OpenOCD's
/// RTT server, with the same channels and log as the others.
#[tauri::command(async)]
pub fn open_tcp_session(
    id: u32,
    config: net::TcpConfig,
    log: Option<LogSetup>,
    data: Channel<InvokeResponseBody>,
    events: Channel<SessionEvent>,
    app: AppHandle,
    sessions: State<Sessions>,
) -> LogStatus {
    let (shared, status) = new_log(&app, log);
    let log = shared.clone();
    let mut splitter = LineSplitter::default();
    let session = net::Session::start(config, move |event| {
        let _ = match event {
            net::Event::Data(bytes) => {
                log_data(&log, &events, &bytes);
                send_lines(&mut splitter, &events, &bytes);
                data.send(InvokeResponseBody::Raw(bytes))
            }
            net::Event::State(state) => {
                let connected_to = match &state {
                    net::NetState::Connected { address, protocol } => {
                        Some(format!("{address}, {}", protocol.name()))
                    }
                    net::NetState::Waiting { .. } => None,
                };
                log_state(&log, &events, connected_to);
                events.send(SessionEvent::TcpState(state))
            }
        };
    });
    sessions.insert(id, Running::Tcp(session), shared);
    status
}

/// Turns a running session's log on with `log`, or off without it. A log
/// turned on starts a new file.
#[tauri::command(async)]
pub fn set_log(
    id: u32,
    log: Option<LogSetup>,
    app: AppHandle,
    sessions: State<Sessions>,
) -> LogStatus {
    let Some(shared) = sessions.lock().get(&id).map(|entry| entry.log.clone()) else {
        return LogStatus::default();
    };
    let connected_to = lock_log(&shared).connected_to.clone();
    let (logger, status) = start_log(&app, log, connected_to.as_deref());
    lock_log(&shared).logger = logger;
    status
}

/// Starts recording a running session to a new file in `folder` (empty for
/// the log folder), or stops it without a folder. A recording keeps every
/// byte received and sent with its time, to play back later.
#[tauri::command(async)]
pub fn set_recording(
    id: u32,
    folder: Option<String>,
    name: String,
    app: AppHandle,
    sessions: State<Sessions>,
) -> LogStatus {
    let Some(shared) = sessions.lock().get(&id).map(|entry| entry.log.clone()) else {
        return LogStatus {
            path: None,
            problem: Some("Connect first, then record.".into()),
        };
    };
    let Some(folder) = folder else {
        lock_log(&shared).recorder = None;
        return LogStatus::default();
    };
    let folder = if folder.is_empty() {
        default_folder(&app)
    } else {
        PathBuf::from(folder)
    };
    let connected_to = lock_log(&shared).connected_to.clone();
    let started = Recorder::create(&folder, &name).and_then(|mut recorder| {
        if let Some(what) = connected_to {
            recorder.note(&format!("connected to {what}"))?;
        }
        Ok(recorder)
    });
    match started {
        Ok(recorder) => {
            let path = Some(recorder.path().display().to_string());
            lock_log(&shared).recorder = Some(recorder);
            LogStatus {
                path,
                problem: None,
            }
        }
        Err(error) => LogStatus {
            path: None,
            problem: Some(format!(
                "Could not start a recording in {}: {error}",
                folder.display()
            )),
        },
    }
}

/// What the UI learns of a recording it opens.
#[derive(Clone, Serialize)]
pub struct ReplayInfo {
    /// The name of the tab it was recorded from.
    name: String,
    /// How long it plays, in seconds.
    length: f64,
}

/// Opens a recording to play back, paused at its start. What it received
/// and sent, and its lines, come to `events` in order as it plays.
#[tauri::command(async)]
pub fn open_replay_session(
    id: u32,
    path: String,
    speed: f64,
    events: Channel<SessionEvent>,
    sessions: State<Sessions>,
) -> Result<ReplayInfo, String> {
    let recording = record::read(std::path::Path::new(&path))?;
    let info = ReplayInfo {
        name: recording.name.clone(),
        length: recording.length(),
    };
    // Lines keep the time they were recorded at, so the plot shows it, at any speed.
    let started = recording.started_ms.unwrap_or_else(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0.0, |since| since.as_secs_f64() * 1000.0)
    });
    let mut splitter = LineSplitter::default();
    let mut last_at: Option<f64> = None;
    let player = Player::start(recording, speed, move |event| {
        let _ = match event {
            replay::Event::Chunks(chunks) => {
                let mut pieces: Vec<Piece> = Vec::new();
                let mut lines = Vec::new();
                let mut times = Vec::new();
                for chunk in chunks {
                    let sent = chunk.direction == Direction::Sent;
                    let at = started + chunk.at * 1000.0;
                    if !sent {
                        // Lines read at once came one after another since the read
                        // before: spread as the live views spread them.
                        let found = splitter.feed(&chunk.bytes);
                        let gap = last_at.map_or(0.0, |before| (at - before).clamp(0.0, SPREAD_MS));
                        let count = found.len();
                        for (index, line) in found.into_iter().enumerate() {
                            lines.push(line);
                            times.push(at - (count - 1 - index) as f64 * gap / count as f64);
                        }
                        last_at = Some(at);
                    }
                    pieces.push(Piece {
                        sent,
                        bytes: chunk.bytes,
                        at,
                    });
                }
                events.send(SessionEvent::Replayed {
                    pieces,
                    lines,
                    times,
                })
            }
            replay::Event::Restarted => {
                splitter = LineSplitter::default();
                last_at = None;
                events.send(SessionEvent::Restarted)
            }
            replay::Event::State(state) => events.send(SessionEvent::ReplayState(state)),
        };
    });
    sessions.insert(id, Running::Replay(player), Arc::default());
    Ok(info)
}

/// What the UI asks of a recording being played.
#[derive(Clone, Copy, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReplayCommand {
    Play,
    Pause,
    Speed { speed: f64 },
    Seek { position: f64 },
}

#[tauri::command]
pub fn control_replay(id: u32, command: ReplayCommand, sessions: State<Sessions>) {
    sessions.with(id, |session| {
        if let Running::Replay(player) = session {
            match command {
                ReplayCommand::Play => player.play(),
                ReplayCommand::Pause => player.pause(),
                ReplayCommand::Speed { speed } => player.set_speed(speed),
                ReplayCommand::Seek { position } => player.seek(position),
            }
        }
    });
}

/// Writes text to a file the user chose, for File > Save output as.
#[tauri::command(async)]
pub fn save_text(path: String, text: String) -> Result<(), String> {
    std::fs::write(&path, text).map_err(|error| format!("Could not save {path}: {error}"))
}

/// Writes bytes to a file the user chose, such as a chart as a PNG.
#[tauri::command(async)]
pub fn save_bytes(path: String, data: Vec<u8>) -> Result<(), String> {
    std::fs::write(&path, data).map_err(|error| format!("Could not save {path}: {error}"))
}

#[tauri::command(async)]
pub fn close_session(id: u32, sessions: State<Sessions>) {
    let old = sessions.lock().remove(&id);
    drop(old);
}

/// Closes every session. The UI calls it when it loads, so a reload never
/// leaves a session holding its port or probe.
#[tauri::command(async)]
pub fn close_all_sessions(sessions: State<Sessions>) {
    let old = std::mem::take(&mut *sessions.lock());
    drop(old);
}

/// Closes every session as the app ends, each letting go of its port or
/// probe as it does when closed. An ST-LINK left in the middle of a transfer
/// would not open again until it is plugged in again.
pub fn close_all(app: &AppHandle) {
    let old = std::mem::take(&mut *app.state::<Sessions>().lock());
    drop(old);
}

/// Sends bytes to a device. Not async, so key presses go out in the order they were typed.
#[tauri::command]
pub fn write_session(id: u32, data: Vec<u8>, sessions: State<Sessions>) {
    sessions.write(id, data);
}

/// Turns what the send box or a macro holds into bytes: hex digits, or text
/// with escapes such as `\r`. An error says what is wrong in plain words.
#[tauri::command]
pub fn encode_input(text: String, hex: bool) -> Result<Vec<u8>, String> {
    send::encode(&text, hex)
}

/// How sending a file goes, as the UI sees it.
#[derive(Clone, Serialize)]
pub struct FileProgress {
    sent: u64,
    total: u64,
    /// Set on the last report, when sending has ended one way or another.
    done: bool,
    cancelled: bool,
    problem: Option<String>,
}

/// Starts sending a file through a running session, in pieces paced for
/// `bytes_per_second` (0 for as fast as it takes them), and returns its size.
/// How it goes is reported to `progress`, a few times a second.
#[tauri::command(async)]
pub fn send_file(
    id: u32,
    path: String,
    bytes_per_second: u32,
    progress: Channel<FileProgress>,
    app: AppHandle,
    sessions: State<Sessions>,
) -> Result<u64, String> {
    let data = std::fs::read(&path).map_err(|error| format!("Could not read {path}: {error}"))?;
    let total = data.len() as u64;
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut all = sessions.lock();
        let entry = all.get_mut(&id).ok_or("The tab is not connected.")?;
        if entry.sending.is_some() {
            return Err("A file is being sent already.".into());
        }
        entry.sending = Some(cancel.clone());
    }

    thread::Builder::new()
        .name("ternima-send-file".into())
        .spawn(move || {
            let sessions = app.state::<Sessions>();
            let mut sent = 0;
            let mut reported = Instant::now();
            let result = send::send_paced(
                &data,
                Pace::for_rate(bytes_per_second),
                &cancel,
                |piece| sessions.write(id, piece.to_vec()),
                |now| {
                    sent = now as u64;
                    if reported.elapsed() >= Duration::from_millis(100) {
                        reported = Instant::now();
                        let _ = progress.send(FileProgress {
                            sent,
                            total,
                            done: false,
                            cancelled: false,
                            problem: None,
                        });
                    }
                },
            );
            if let Some(entry) = sessions.lock().get_mut(&id)
                && entry
                    .sending
                    .as_ref()
                    .is_some_and(|flag| Arc::ptr_eq(flag, &cancel))
            {
                entry.sending = None;
            }
            let _ = progress.send(FileProgress {
                sent,
                total,
                done: true,
                cancelled: result == Err(Stopped::Cancelled),
                problem: (result == Err(Stopped::Gone))
                    .then(|| "The tab disconnected before the whole file was sent.".into()),
            });
        })
        .map_err(|error| format!("Could not start sending: {error}"))?;
    Ok(total)
}

/// Stops sending a file, between two pieces.
#[tauri::command]
pub fn cancel_send(id: u32, sessions: State<Sessions>) {
    if let Some(flag) = sessions
        .lock()
        .get(&id)
        .and_then(|entry| entry.sending.as_ref())
    {
        flag.store(true, Ordering::Relaxed);
    }
}

#[tauri::command]
pub fn set_session_lines(id: u32, dtr: Option<bool>, rts: Option<bool>, sessions: State<Sessions>) {
    sessions.with(id, |session| {
        if let Running::Serial(session) = session {
            if let Some(on) = dtr {
                session.set_dtr(on);
            }
            if let Some(on) = rts {
                session.set_rts(on);
            }
        }
    });
}

#[tauri::command]
pub fn break_session(id: u32, sessions: State<Sessions>) {
    sessions.with(id, |session| {
        if let Running::Serial(session) = session {
            session.send_break();
        }
    });
}

#[tauri::command]
pub fn reconfigure_session(id: u32, config: serial::SerialConfig, sessions: State<Sessions>) {
    sessions.with(id, |session| {
        if let Running::Serial(session) = session {
            session.reconfigure(config);
        }
    });
}

#[tauri::command]
pub fn reconfigure_rtt_session(id: u32, config: rtt::RttConfig, sessions: State<Sessions>) {
    sessions.with(id, |session| {
        if let Running::Rtt(session) = session {
            session.reconfigure(config);
        }
    });
}

/// The variables of the ELF file read last, with when the file was changed:
/// it is read again only after the firmware is built again.
type KnownVariables = (PathBuf, SystemTime, Arc<Vec<elf::Variable>>);
static VARIABLES: Mutex<Option<KnownVariables>> = Mutex::new(None);

/// The variables in an ELF file that can be watched.
fn variables_in(path: &str) -> Result<Arc<Vec<elf::Variable>>, String> {
    if path.trim().is_empty() {
        return Err("Choose the firmware's ELF file, built with debug information.".into());
    }
    let path = PathBuf::from(path);
    let modified = std::fs::metadata(&path)
        .and_then(|metadata| metadata.modified())
        .map_err(|error| format!("Could not read {}: {error}", path.display()))?;
    let mut known = VARIABLES.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((known_path, known_modified, variables)) = known.as_ref()
        && *known_path == path
        && *known_modified == modified
    {
        return Ok(variables.clone());
    }
    let data = std::fs::read(&path)
        .map_err(|error| format!("Could not read {}: {error}", path.display()))?;
    let variables = Arc::new(elf::variables(&data)?);
    *known = Some((path, modified, variables.clone()));
    Ok(variables)
}

/// Every variable in an ELF file that the Watch view can read, by name.
#[tauri::command(async)]
pub fn elf_variables(path: String) -> Result<Vec<elf::Variable>, String> {
    variables_in(&path).map(|variables| variables.as_ref().clone())
}

/// Reads these variables of an RTT tab's chip every `every_ms`, found by
/// name in its ELF file, and says which of them the file has. No names stop it.
#[tauri::command(async)]
pub fn watch_session(
    id: u32,
    elf: String,
    names: Vec<String>,
    every_ms: u32,
    sessions: State<Sessions>,
) -> Result<Vec<bool>, String> {
    let variables = if names.is_empty() {
        Arc::new(Vec::new())
    } else {
        variables_in(&elf)?
    };
    let watch: Vec<Option<rtt::Watched>> = names
        .iter()
        .map(|name| {
            variables
                .iter()
                .find(|variable| &variable.name == name)
                .map(|variable| rtt::Watched {
                    address: variable.address,
                    kind: variable.kind,
                })
        })
        .collect();
    let found = watch.iter().map(Option::is_some).collect();
    let every = Duration::from_millis(u64::from(every_ms.max(1)));
    sessions.with(id, |session| {
        if let Running::Rtt(session) = session {
            session.watch(watch, every);
        }
    });
    Ok(found)
}

#[tauri::command]
pub fn reconfigure_swo_session(id: u32, config: swo::SwoConfig, sessions: State<Sessions>) {
    sessions.with(id, |session| {
        if let Running::Swo(session) = session {
            session.reconfigure(config);
        }
    });
}

#[tauri::command]
pub fn reconfigure_tcp_session(id: u32, config: net::TcpConfig, sessions: State<Sessions>) {
    sessions.with(id, |session| {
        if let Running::Tcp(session) = session {
            session.reconfigure(config);
        }
    });
}

/// Resets the chip of an RTT or SWO session.
#[tauri::command]
pub fn reset_target(id: u32, sessions: State<Sessions>) {
    sessions.with(id, |session| match session {
        Running::Rtt(session) => session.reset(),
        Running::Swo(session) => session.reset(),
        // No probe of its own: an RTT server's probe belongs to the debugger.
        Running::Serial(_) | Running::Tcp(_) | Running::Replay(_) => {}
    });
}
