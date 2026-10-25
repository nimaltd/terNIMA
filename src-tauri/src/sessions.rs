//! The session commands the UI calls, for serial ports, RTT, SWO and TCP,
//! the sessions they run, and the log file each can write.

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Manager, State};
use ternima_core::logfile::{LogSetup, Logger};
use ternima_core::send::{self, Pace, Stopped};
use ternima_core::{net, probe, rtt, serial, swo};

/// A running session of any kind.
enum Running {
    Serial(serial::Session),
    Rtt(rtt::Session),
    Swo(swo::Session),
    Tcp(net::Session),
}

impl Running {
    /// Sends bytes to the device. False for SWO, which only receives.
    fn write(&self, data: Vec<u8>) -> bool {
        match self {
            Running::Serial(session) => session.write(data),
            Running::Rtt(session) => session.write(data),
            Running::Tcp(session) => session.write(data),
            Running::Swo(_) => return false,
        }
        true
    }
}

/// A session's log, shared between its thread and the commands.
#[derive(Default)]
struct LogState {
    logger: Option<Logger>,
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

    /// Sends bytes to a session. False when it is not running, or only receives.
    fn write(&self, id: u32, data: Vec<u8>) -> bool {
        self.lock()
            .get(&id)
            .is_some_and(|entry| entry.running.write(data))
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
    /// The chip's trace FIFO overflowed this many times since the last report.
    Overflow {
        count: u32,
    },
    /// Writing the log failed, so it was turned off.
    LogStopped {
        problem: String,
    },
}

/// Whether a log is being written, and where; or why it could not start.
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

fn log_data(log: &SharedLog, events: &Channel<SessionEvent>, bytes: &[u8]) {
    write_log(log, events, |state| match &mut state.logger {
        Some(logger) => logger.data(bytes),
        None => Ok(()),
    });
}

/// Notes a connect, with what it connected to, or a disconnect.
fn log_state(log: &SharedLog, events: &Channel<SessionEvent>, connected_to: Option<String>) {
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
    let session =
        serial::Session::start(Arc::new(serial::SystemPorts), port, config, move |event| {
            // A send only fails when the window is gone, and then nobody is listening.
            let _ = match event {
                serial::Event::Data(bytes) => {
                    log_data(&log, &events, &bytes);
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
    let session = rtt::Session::start(Arc::new(rtt::ProbeRs), config, move |event| {
        let _ = match event {
            rtt::Event::Data(bytes) => {
                log_data(&log, &events, &bytes);
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
    let session = swo::Session::start(Arc::new(swo::ProbeRs), config, move |event| {
        let _ = match event {
            swo::Event::Data(bytes) => {
                log_data(&log, &events, &bytes);
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
    let session = net::Session::start(config, move |event| {
        let _ = match event {
            net::Event::Data(bytes) => {
                log_data(&log, &events, &bytes);
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

/// Writes text to a file the user chose, for File > Save output as.
#[tauri::command(async)]
pub fn save_text(path: String, text: String) -> Result<(), String> {
    std::fs::write(&path, text).map_err(|error| format!("Could not save {path}: {error}"))
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
        Running::Serial(_) | Running::Tcp(_) => {}
    });
}
