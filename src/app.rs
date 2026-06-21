use chrono::{DateTime, Local};
use std::time::{Duration, Instant};
use tokio::sync::watch;

use crate::fetch::{fetch_all, UsageData};

pub const REFRESH_SECS: u64 = 300;

pub struct App {
    pub data: UsageData,
    pub fetching: bool,
    pub last_updated: Option<DateTime<Local>>,
    pub next_refresh: Instant,
    rx: watch::Receiver<Option<(UsageData, DateTime<Local>)>>,
    tx: watch::Sender<Option<(UsageData, DateTime<Local>)>>,
}

impl App {
    pub fn new() -> Self {
        let (tx, rx) = watch::channel(None);
        App {
            data: UsageData::default(),
            fetching: false,
            last_updated: None,
            next_refresh: Instant::now(),
            rx,
            tx,
        }
    }

    pub fn tick(&mut self) {
        // Drain any completed fetch result
        if self.rx.has_changed().unwrap_or(false) {
            if let Some((data, ts)) = self.rx.borrow_and_update().clone() {
                // Keep stale data when a fetch fails so display doesn't blank out
                self.data = UsageData {
                    claude: data.claude.or_else(|| self.data.claude.clone()),
                    claude_error: data.claude_error,
                    codex: data.codex.or_else(|| self.data.codex.clone()),
                    codex_error: data.codex_error,
                };
                self.last_updated = Some(ts);
                self.fetching = false;
                self.next_refresh = Instant::now() + Duration::from_secs(REFRESH_SECS);
            }
        }

        // Kick off a fetch if due
        if !self.fetching && Instant::now() >= self.next_refresh {
            self.fetching = true;
            let tx = self.tx.clone();
            tokio::spawn(async move {
                let data = fetch_all().await;
                let ts = Local::now();
                let _ = tx.send(Some((data, ts)));
            });
        }
    }

    pub fn force_refresh(&mut self) {
        self.next_refresh = Instant::now();
    }

    pub fn secs_until_refresh(&self) -> u64 {
        self.next_refresh
            .checked_duration_since(Instant::now())
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }
}
