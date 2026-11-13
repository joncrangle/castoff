//! App state and the background jobs that feed it.

use std::collections::VecDeque;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use castoff_core::{Backend, Drive, Episode, Settings, SyncOutcome, TransferProgress};
use rune::Waker;

use crate::theme::DEFAULT_THEME;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppView {
    Normal,
    DriveSelection,
    Syncing,
    Confirm,
    ThemeSelection,
    Debug,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    Mac,
    Drive,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DebugMessage {
    /// Unix milliseconds.
    pub timestamp: i64,
    pub message: String,
    pub level: Level,
}

/// What a background job hands back to the UI thread.
#[derive(Debug)]
pub enum Msg {
    Settings(Settings),
    MacLoaded { generation: u64, result: Result<Vec<Episode>, String> },
    DrivesScanned { reason: ScanReason, result: Result<Vec<Drive>, String> },
    DriveLoaded { drive_id: String, result: Result<Vec<Episode>, String> },
    SyncProgress(TransferProgress),
    SyncDone(Result<SyncOutcome, String>),
    Deleted(Result<(), String>),
    Saved { what: &'static str, result: Result<(), String> },
}

/// Why drives were scanned, which decides what the answer does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanReason {
    /// Startup or the drive selector: replace the list (favourites first) and auto-select.
    Full,
    /// The periodic check for drives appearing and disappearing.
    Watch,
}

/// Sends messages to the UI thread and wakes it.
#[derive(Clone)]
pub struct Emit {
    tx: Sender<Msg>,
    waker: Arc<Mutex<Option<Waker>>>,
}

impl Emit {
    pub fn send(&self, msg: Msg) {
        let _ = self.tx.send(msg);
        let waker = self.waker.lock().ok().and_then(|w| w.clone());
        if let Some(w) = waker {
            w();
        }
    }
}

/// Runs backend calls off the UI thread. `inline` runs them at once on the caller instead,
/// still answering through the channel, so headless runs and most tests stay deterministic:
/// the answer lands in the next `poll`, whatever the machine's speed.
pub struct Jobs {
    inline: bool,
    emit: Emit,
    rx: Receiver<Msg>,
}

impl Jobs {
    pub fn new(inline: bool) -> Self {
        let (tx, rx) = channel();
        Jobs { inline, emit: Emit { tx, waker: Arc::new(Mutex::new(None)) }, rx }
    }

    pub fn inline(&self) -> bool {
        self.inline
    }

    pub fn set_waker(&self, waker: Waker) {
        if let Ok(mut w) = self.emit.waker.lock() {
            *w = Some(waker);
        }
    }

    pub fn spawn(&self, backend: &Arc<dyn Backend>, f: impl FnOnce(&dyn Backend, &Emit) + Send + 'static) {
        let backend = backend.clone();
        let emit = self.emit.clone();
        if self.inline {
            f(&*backend, &emit);
        } else {
            std::thread::spawn(move || f(&*backend, &emit));
        }
    }

    pub fn drain(&self) -> Vec<Msg> {
        self.rx.try_iter().collect()
    }
}

pub struct State {
    pub backend: Arc<dyn Backend>,
    pub jobs: Jobs,

    pub view: AppView,
    pub focused: Pane,
    pub mac_index: usize,
    pub drive_index: usize,
    pub drive_menu_index: usize,
    pub theme_menu_index: usize,
    pub debug_menu_index: usize,
    pub mac: Vec<Episode>,
    pub drive_eps: Vec<Episode>,
    pub drives: Vec<Drive>,
    pub current_drive: Option<Drive>,
    pub loading_mac: bool,
    pub loading_drive: bool,
    pub scanning: bool,
    pub progress: TransferProgress,
    pub error: String,
    pub debug_messages: Vec<DebugMessage>,
    pub last_key: Option<String>,
    /// The theme on screen, which the picker previews.
    pub theme: String,
    /// The theme in the settings file; Escape in the picker returns to it.
    pub saved_theme: String,
    pub favorites: Vec<String>,
    /// `DEBUG=true` or `--debug`: header shows the last key and window size.
    pub debug_enabled: bool,

    /// Startup work has been started (scenes set this to keep their hand-made state).
    pub booted: bool,
    /// Poll `scan_drives` every couple of seconds for drives appearing and disappearing.
    pub watch_drives: bool,
    pub next_drive_watch: f64,
    pub watch_in_flight: bool,
    pub mac_generation: u64,
    pub sync_cancel: Option<Arc<AtomicBool>>,
    pub last_refresh: f64,
    /// Rows one page of the focused list holds (Ctrl+U / Ctrl+D), from its laid-out height.
    pub page_rows: usize,
    pub window_size: (f32, f32),
    /// Set by `q` / Ctrl+C (the window also closes); tests read it.
    pub quit_requested: bool,
    /// Scroll the current row of the open list into view on the next poll.
    pub reveal_pending: bool,
    /// Headless recordings: inline jobs answer at once, so hand out one sync progress step a
    /// frame from `backlog`, and the transfer plays out on the virtual clock as it would live.
    pub paced: bool,
    pub backlog: VecDeque<Msg>,
}

impl State {
    pub fn new(backend: Arc<dyn Backend>, inline: bool) -> Self {
        State {
            backend,
            jobs: Jobs::new(inline),
            view: AppView::Normal,
            focused: Pane::Mac,
            mac_index: 0,
            drive_index: 0,
            drive_menu_index: 0,
            theme_menu_index: 0,
            debug_menu_index: 0,
            mac: Vec::new(),
            drive_eps: Vec::new(),
            drives: Vec::new(),
            current_drive: None,
            loading_mac: true,
            loading_drive: true,
            scanning: false,
            progress: TransferProgress::default(),
            error: String::new(),
            debug_messages: Vec::new(),
            last_key: None,
            theme: DEFAULT_THEME.into(),
            saved_theme: DEFAULT_THEME.into(),
            favorites: Vec::new(),
            debug_enabled: false,
            booted: false,
            watch_drives: true,
            next_drive_watch: 0.0,
            watch_in_flight: false,
            paced: false,
            backlog: VecDeque::new(),
            mac_generation: 0,
            sync_cancel: None,
            last_refresh: f64::NEG_INFINITY,
            page_rows: 8,
            window_size: (0.0, 0.0),
            quit_requested: false,
            reveal_pending: false,
        }
    }

    pub fn log(&mut self, message: impl Into<String>, level: Level) {
        let timestamp = chrono::Utc::now().timestamp_millis();
        self.debug_messages.push(DebugMessage { timestamp, message: message.into(), level });
        if self.debug_messages.len() > 500 {
            let extra = self.debug_messages.len() - 500;
            self.debug_messages.drain(..extra);
        }
    }

    pub fn is_favorite(&self, drive_id: &str) -> bool {
        self.favorites.iter().any(|f| f == drive_id)
    }

    pub fn list(&self, pane: Pane) -> &[Episode] {
        match pane {
            Pane::Mac => &self.mac,
            Pane::Drive => &self.drive_eps,
        }
    }

    pub fn list_mut(&mut self, pane: Pane) -> &mut Vec<Episode> {
        match pane {
            Pane::Mac => &mut self.mac,
            Pane::Drive => &mut self.drive_eps,
        }
    }

    pub fn index(&self, pane: Pane) -> usize {
        match pane {
            Pane::Mac => self.mac_index,
            Pane::Drive => self.drive_index,
        }
    }

    pub fn set_index(&mut self, pane: Pane, i: usize) {
        match pane {
            Pane::Mac => self.mac_index = i,
            Pane::Drive => self.drive_index = i,
        }
    }

    pub fn selected(&self, pane: Pane) -> Vec<Episode> {
        self.list(pane).iter().filter(|e| e.selected).cloned().collect()
    }

    pub fn settings(&self) -> Settings {
        Settings { theme: self.saved_theme.clone(), favorite_drives: self.favorites.clone() }
    }
}
