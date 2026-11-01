//! Playing a recording back with its timing: paused, faster or slower, and
//! from any point in it.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::record::{Chunk, Recording};

/// How often where it is gets reported while it plays.
const TICK: Duration = Duration::from_millis(100);
/// Most bytes in one batch, so a jump does not send one huge piece.
const BATCH_BYTES: usize = 64 * 1024;
const SPEED_MIN: f64 = 0.01;
const SPEED_MAX: f64 = 1000.0;

/// Where the player is in its recording.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct PlayState {
    /// Seconds from the start.
    pub position: f64,
    pub length: f64,
    pub playing: bool,
    pub speed: f64,
}

pub enum Event {
    /// Pieces of the recording as they come due, in order.
    Chunks(Vec<Chunk>),
    /// Playing goes back to the start: what was shown should go.
    Restarted,
    State(PlayState),
}

enum Command {
    Play,
    Pause,
    Speed(f64),
    Seek(f64),
}

/// A recording being played on its own thread, until the player is dropped.
pub struct Player {
    commands: Option<Sender<Command>>,
    thread: Option<JoinHandle<()>>,
}

impl Player {
    /// Starts paused at the start. Everything that happens comes back through
    /// `on_event`, on the player's own thread.
    pub fn start(
        recording: Recording,
        speed: f64,
        on_event: impl FnMut(Event) + Send + 'static,
    ) -> Self {
        let (commands, receive) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("ternima-replay".into())
            .spawn(move || run(recording, speed, &receive, on_event))
            .expect("could not start the replay thread");
        Self {
            commands: Some(commands),
            thread: Some(thread),
        }
    }

    pub fn play(&self) {
        self.send(Command::Play);
    }

    pub fn pause(&self) {
        self.send(Command::Pause);
    }

    /// Plays this many times as fast as it was recorded.
    pub fn set_speed(&self, speed: f64) {
        self.send(Command::Speed(speed));
    }

    /// Goes to a point, in seconds from the start: forward at once, or back
    /// by starting again.
    pub fn seek(&self, position: f64) {
        self.send(Command::Seek(position));
    }

    fn send(&self, command: Command) {
        if let Some(commands) = &self.commands {
            let _ = commands.send(command);
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        // Without its commands, the thread ends.
        self.commands = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Playback<F: FnMut(Event)> {
    chunks: Vec<Chunk>,
    length: f64,
    /// The next piece to send.
    next: usize,
    playing: bool,
    speed: f64,
    /// Where it was at an instant; while playing, it moves on from there.
    anchor: (Instant, f64),
    on_event: F,
}

impl<F: FnMut(Event)> Playback<F> {
    fn position(&self) -> f64 {
        let (at, position) = self.anchor;
        if self.playing {
            (position + at.elapsed().as_secs_f64() * self.speed).min(self.length)
        } else {
            position
        }
    }

    fn set_anchor(&mut self, position: f64) {
        self.anchor = (Instant::now(), position);
    }

    fn report(&mut self) {
        let state = PlayState {
            position: self.position(),
            length: self.length,
            playing: self.playing,
            speed: self.speed,
        };
        (self.on_event)(Event::State(state));
    }

    /// Sends the pieces due by `position`, in batches.
    fn send_until(&mut self, position: f64) {
        while self.next < self.chunks.len() && self.chunks[self.next].at <= position {
            let mut batch = Vec::new();
            let mut bytes = 0;
            while self.next < self.chunks.len()
                && self.chunks[self.next].at <= position
                && bytes < BATCH_BYTES
            {
                let chunk = self.chunks[self.next].clone();
                bytes += chunk.bytes.len();
                batch.push(chunk);
                self.next += 1;
            }
            (self.on_event)(Event::Chunks(batch));
        }
    }

    fn restart(&mut self) {
        (self.on_event)(Event::Restarted);
        self.next = 0;
    }

    fn apply(&mut self, command: Command) {
        let position = self.position();
        match command {
            Command::Play => {
                // At the end, Play plays it again from the start.
                let from = if self.next >= self.chunks.len() && position >= self.length {
                    self.restart();
                    0.0
                } else {
                    position
                };
                self.playing = true;
                self.set_anchor(from);
            }
            Command::Pause => {
                self.playing = false;
                self.set_anchor(position);
            }
            Command::Speed(speed) => {
                self.speed = speed.clamp(SPEED_MIN, SPEED_MAX);
                self.set_anchor(position);
            }
            Command::Seek(target) => {
                let target = target.clamp(0.0, self.length);
                if target < position {
                    self.restart();
                }
                self.send_until(target);
                self.set_anchor(target);
            }
        }
    }

    /// Sends what came due, and stops at the end.
    fn advance(&mut self) {
        if !self.playing {
            return;
        }
        let position = self.position();
        self.send_until(position);
        if self.next >= self.chunks.len() && position >= self.length {
            self.playing = false;
            self.set_anchor(self.length);
            self.report();
        }
    }

    /// How long to wait for a command: until the next piece is due, or a tick.
    fn wait(&self) -> Duration {
        match self.chunks.get(self.next) {
            Some(chunk) if self.playing => {
                let seconds = (chunk.at - self.position()) / self.speed;
                Duration::from_secs_f64(seconds.max(0.0)).min(TICK)
            }
            _ => TICK,
        }
    }
}

fn run(
    recording: Recording,
    speed: f64,
    commands: &Receiver<Command>,
    on_event: impl FnMut(Event),
) {
    let length = recording.length();
    let mut playback = Playback {
        chunks: recording.chunks,
        length,
        next: 0,
        playing: false,
        speed: speed.clamp(SPEED_MIN, SPEED_MAX),
        anchor: (Instant::now(), 0.0),
        on_event,
    };
    playback.report();
    let mut reported = Instant::now();
    loop {
        match commands.recv_timeout(playback.wait()) {
            Ok(command) => {
                playback.apply(command);
                playback.advance();
                playback.report();
                reported = Instant::now();
                continue;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        playback.advance();
        if playback.playing && reported.elapsed() >= TICK {
            playback.report();
            reported = Instant::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::Direction;

    fn recording(times: &[f64]) -> Recording {
        Recording {
            name: "test".into(),
            started_ms: Some(0.0),
            pty: false,
            chunks: times
                .iter()
                .enumerate()
                .map(|(index, &at)| Chunk {
                    at,
                    direction: Direction::Received,
                    bytes: vec![index as u8],
                })
                .collect(),
        }
    }

    enum Seen {
        Bytes(Vec<u8>),
        Restarted,
        State(PlayState),
    }

    fn start(times: &[f64], speed: f64) -> (Player, mpsc::Receiver<Seen>) {
        let (send, seen) = mpsc::channel();
        let player = Player::start(recording(times), speed, move |event| {
            let _ = send.send(match event {
                Event::Chunks(chunks) => {
                    Seen::Bytes(chunks.into_iter().flat_map(|chunk| chunk.bytes).collect())
                }
                Event::Restarted => Seen::Restarted,
                Event::State(state) => Seen::State(state),
            });
        });
        (player, seen)
    }

    /// What comes until the player says it stopped, or a second passes.
    fn until_stopped(seen: &mpsc::Receiver<Seen>) -> (Vec<u8>, bool) {
        let mut bytes = Vec::new();
        let mut restarted = false;
        let deadline = Instant::now() + Duration::from_secs(2);
        while let Ok(event) = seen.recv_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            match event {
                Seen::Bytes(more) => bytes.extend(more),
                Seen::Restarted => restarted = true,
                Seen::State(state) if !state.playing && !bytes.is_empty() => break,
                Seen::State(_) => {}
            }
        }
        (bytes, restarted)
    }

    #[test]
    fn it_waits_paused_then_plays_everything_in_order() {
        let (player, seen) = start(&[0.0, 0.01, 0.02, 0.05], 10.0);
        match seen.recv_timeout(Duration::from_secs(1)) {
            Ok(Seen::State(state)) => {
                assert!(!state.playing);
                assert_eq!(state.length, 0.05);
            }
            _ => panic!("no state at the start"),
        }
        while let Ok(event) = seen.recv_timeout(Duration::from_millis(150)) {
            assert!(
                !matches!(event, Seen::Bytes(_)),
                "nothing plays before play"
            );
        }
        player.play();
        assert_eq!(until_stopped(&seen), (vec![0, 1, 2, 3], false));
    }

    #[test]
    fn a_jump_back_starts_again_and_a_jump_forward_sends_at_once() {
        let (player, seen) = start(&[0.0, 1.0, 2.0, 3.0], 1.0);
        player.seek(2.5);
        let mut bytes = Vec::new();
        while let Ok(event) = seen.recv_timeout(Duration::from_millis(200)) {
            if let Seen::Bytes(more) = event {
                bytes.extend(more);
            }
        }
        assert_eq!(bytes, [0, 1, 2]);
        player.seek(0.5);
        let mut restarted = false;
        bytes.clear();
        while let Ok(event) = seen.recv_timeout(Duration::from_millis(200)) {
            match event {
                Seen::Restarted => restarted = true,
                Seen::Bytes(more) => bytes.extend(more),
                Seen::State(_) => {}
            }
        }
        assert!(restarted);
        assert_eq!(bytes, [0]);
    }

    #[test]
    fn play_at_the_end_plays_it_again() {
        let (player, seen) = start(&[0.0, 0.01], 10.0);
        player.play();
        assert_eq!(until_stopped(&seen), (vec![0, 1], false));
        player.play();
        assert_eq!(until_stopped(&seen), (vec![0, 1], true));
    }

    #[test]
    fn pause_holds_until_play() {
        let (player, seen) = start(&[0.0, 0.3], 1.0);
        player.play();
        thread::sleep(Duration::from_millis(100));
        player.pause();
        thread::sleep(Duration::from_millis(400));
        let mut bytes = Vec::new();
        while let Ok(event) = seen.try_recv() {
            if let Seen::Bytes(more) = event {
                bytes.extend(more);
            }
        }
        assert_eq!(bytes, [0], "the second piece waits for play");
        player.set_speed(100.0);
        player.play();
        assert_eq!(until_stopped(&seen), (vec![1], false));
    }
}
