//! The session commands the UI calls, for serial ports, RTT and SWO, the
//! sessions they run, and the log file each can write.

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde::Serialize;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Manager, State};
use ternima_core::logfile::{LogSetup, Logger};
use ternima_core::{probe, rtt, serial, swo};

/// A running session of any kind.
enum Running {
    Serial(serial::Session),
    Rtt(rtt::Session),
    Swo(swo::Session),
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

    /// Adds a session. One with the same number is replaced; closing it can
    /// take a moment, so it happens after the lock is let go.
    fn insert(&self, id: u32, running: Running, log: SharedLog) {
        let old = self.lock().insert(id, Entry { running, log });
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
    sessions.with(id, |session| match session {
        Running::Serial(session) => session.write(data),
        Running::Rtt(session) => session.write(data),
        // SWO only comes from the chip.
        Running::Swo(_) => {}
    });
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

/// Resets the chip of an RTT or SWO session.
#[tauri::command]
pub fn reset_target(id: u32, sessions: State<Sessions>) {
    sessions.with(id, |session| match session {
        Running::Rtt(session) => session.reset(),
        Running::Swo(session) => session.reset(),
        Running::Serial(_) => {}
    });
}
