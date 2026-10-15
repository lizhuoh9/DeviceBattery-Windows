//! The background thread that reads all batteries.
//!
//! It refreshes once a minute, on request, and when devices are plugged in or out, then
//! hands the rows to the widget thread and wakes it with a window message.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use super::receivers::{self, Session};
use super::{bluetooth, hid, settings};
use crate::common::model::DeviceReading;
use crate::common::store::ReadingStore;
use crate::common::strings::Strings;

const REFRESH_INTERVAL: Duration = Duration::from_secs(60);
const BLUETOOTH_SOURCE_KEY: &str = "bluetooth";

pub enum Command {
    Refresh,
    DevicesChanged,
    Quit,
}

pub struct Monitor {
    commands: Sender<Command>,
}

impl Monitor {
    /// Starts the thread; rows arrive on `rows` and each delivery posts `message` to `window`.
    pub fn start(
        window: HWND,
        message: u32,
        strings: Strings,
        rows: Sender<Vec<DeviceReading>>,
    ) -> Self {
        let (commands, inbox) = mpsc::channel();
        let window = window.0 as isize;
        // Detached: quitting never waits for a read in flight (files are replaced atomically).
        let _ = thread::Builder::new()
            .name("battery-monitor".into())
            .spawn(move || {
                let mut worker = Worker::new(strings, rows, window, message);
                worker.run(&inbox);
            });
        Self { commands }
    }

    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    pub fn stop(&self) {
        self.send(Command::Quit);
    }
}

struct Worker {
    strings: Strings,
    rows: Sender<Vec<DeviceReading>>,
    window: isize,
    message: u32,
    sessions: Vec<Session>,
    store: ReadingStore,
    saved: String,
}

impl Worker {
    fn new(
        strings: Strings,
        rows: Sender<Vec<DeviceReading>>,
        window: isize,
        message: u32,
    ) -> Self {
        let remembered = settings::load_readings();
        let saved = serde_json::to_string(&remembered).unwrap_or_default();
        Self {
            strings,
            rows,
            window,
            message,
            sessions: receivers::discover(&hid::enumerate(), strings),
            store: ReadingStore::new(remembered),
            saved,
        }
    }

    fn run(&mut self, inbox: &Receiver<Command>) {
        loop {
            self.refresh();
            match inbox.recv_timeout(REFRESH_INTERVAL) {
                Ok(Command::Refresh) | Err(RecvTimeoutError::Timeout) => {}
                Ok(Command::DevicesChanged) => self.rediscover(),
                Ok(Command::Quit) | Err(RecvTimeoutError::Disconnected) => break,
            }
            // Several requests may have queued up during a refresh; one refresh serves them all.
            while let Ok(command) = inbox.try_recv() {
                match command {
                    Command::Quit => return,
                    Command::DevicesChanged => self.rediscover(),
                    Command::Refresh => {}
                }
            }
        }
    }

    fn refresh(&mut self) {
        let now = now_seconds();
        for session in &mut self.sessions {
            let readings = session.read(now);
            self.store.apply(readings, &session.key);
        }
        self.store.apply(bluetooth::read(now), BLUETOOTH_SOURCE_KEY);
        let rows = self.store.rows(&Default::default(), &Default::default());
        if self.rows.send(rows).is_ok() {
            // SAFETY: posting to a window handle owned by another thread is allowed; a stale
            // handle only makes the call fail.
            unsafe {
                let _ = PostMessageW(
                    Some(HWND(self.window as *mut _)),
                    self.message,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
        }
        self.persist();
    }

    /// Keeps the readers of receivers that are still there (they hold learned state), adds
    /// new receivers and forgets unplugged ones.
    fn rediscover(&mut self) {
        let mut old = std::mem::take(&mut self.sessions);
        let mut sessions = Vec::new();
        for session in receivers::discover(&hid::enumerate(), self.strings) {
            match old.iter().position(|existing| existing.key == session.key) {
                Some(index) => sessions.push(old.swap_remove(index)),
                None => sessions.push(session),
            }
        }
        for gone in old {
            self.store.remove_source(&gone.key);
        }
        self.sessions = sessions;
    }

    fn persist(&mut self) {
        let mut memory = self.store.memory_snapshot();
        memory.sort_by(|lhs, rhs| lhs.id.cmp(&rhs.id));
        let text = serde_json::to_string(&memory).unwrap_or_default();
        if text != self.saved {
            settings::save_readings(&memory);
            self.saved = text;
        }
    }
}

pub fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}
