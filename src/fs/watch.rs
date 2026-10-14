//! Non-recursive directory watch.
//!
//! Reading a preview opens files and would otherwise reload the listing
//! forever, so access events are ignored. Creates, writes, renames, and
//! deletes arm one reload, held until a short quiet period so a burst
//! becomes a single rescan. A burst that never goes quiet still fires,
//! so a long copy cannot postpone the listing forever.

use std::path::Path;
use std::time::{Duration, Instant};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio_util::sync::CancellationToken;

pub const QUIET: Duration = Duration::from_millis(200);
pub const CAP: Duration = Duration::from_millis(500);

pub fn changes_the_listing(kind: EventKind) -> bool {
    match kind {
        EventKind::Access(_) | EventKind::Other => false,
        EventKind::Any | EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => true,
    }
}

/// Coalesces kernel events into "reload now".
#[derive(Debug, Default)]
pub struct QuietWindow {
    armed_at: Option<Instant>,
    last_change: Option<Instant>,
}

impl QuietWindow {
    pub fn observe(&mut self, kind: EventKind, now: Instant) {
        if !changes_the_listing(kind) {
            return;
        }
        if self.armed_at.is_none() {
            self.armed_at = Some(now);
        }
        self.last_change = Some(now);
    }

    pub fn due(&self, now: Instant) -> bool {
        match (self.armed_at, self.last_change) {
            (Some(armed), Some(last)) => {
                now.saturating_duration_since(last) >= QUIET || now.saturating_duration_since(armed) >= CAP
            }
            _ => false,
        }
    }

    pub fn reset(&mut self) {
        self.armed_at = None;
        self.last_change = None;
    }
}

/// Blocks until `cancel` is signalled or `emit` rejects a hit. `emit`
/// returns whether the listener is still interested.
pub fn drive(directory: &Path, cancel: &CancellationToken, mut emit: impl FnMut() -> bool) {
    let (sender, receiver) = std::sync::mpsc::channel();
    let mut watcher = match RecommendedWatcher::new(sender, notify::Config::default()) {
        Ok(watcher) => watcher,
        Err(_) => return,
    };
    if watcher.watch(directory, RecursiveMode::NonRecursive).is_err() {
        return;
    }

    let mut window = QuietWindow::default();
    loop {
        if cancel.is_cancelled() {
            return;
        }
        if window.due(Instant::now()) {
            window.reset();
            if !emit() {
                return;
            }
        }
        match receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(Ok(event)) => window.observe(event.kind, Instant::now()),
            Ok(Err(_)) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use notify::{EventKind, event::{AccessKind, CreateKind, ModifyKind}};

    use super::*;

    #[test]
    fn access_and_other_do_not_change_the_listing() {
        assert!(!changes_the_listing(EventKind::Access(AccessKind::Any)));
        assert!(!changes_the_listing(EventKind::Other));
        assert!(changes_the_listing(EventKind::Create(CreateKind::Any)));
        assert!(changes_the_listing(EventKind::Modify(ModifyKind::Any)));
        assert!(changes_the_listing(EventKind::Remove(notify::event::RemoveKind::Any)));
        assert!(changes_the_listing(EventKind::Any));
    }

    #[test]
    fn access_does_not_arm_or_postpone_a_reload() {
        let start = Instant::now();
        let mut window = QuietWindow::default();
        window.observe(EventKind::Access(AccessKind::Read), start);
        assert!(!window.due(start + CAP + QUIET));

        window.observe(EventKind::Create(CreateKind::File), start);
        window.observe(EventKind::Access(AccessKind::Read), start + Duration::from_millis(190));
        assert!(!window.due(start + Duration::from_millis(199)));
        assert!(window.due(start + QUIET));
    }

    #[test]
    fn a_second_change_postpones_the_quiet_deadline() {
        let start = Instant::now();
        let mut window = QuietWindow::default();
        window.observe(EventKind::Create(CreateKind::File), start);
        window.observe(EventKind::Modify(ModifyKind::Data(notify::event::DataChange::Any)), start + Duration::from_millis(150));
        assert!(!window.due(start + QUIET));
        assert!(window.due(start + Duration::from_millis(150) + QUIET));
    }

    #[test]
    fn a_burst_that_never_goes_quiet_still_fires_at_the_cap() {
        let start = Instant::now();
        let mut window = QuietWindow::default();
        window.observe(EventKind::Create(CreateKind::File), start);
        window.observe(EventKind::Create(CreateKind::File), start + Duration::from_millis(400));
        assert!(window.due(start + CAP));
        window.reset();
        assert!(!window.due(start + CAP + QUIET));
    }

    #[tokio::test]
    async fn creating_a_file_wakes_the_watch_and_cancel_stops_it() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = CancellationToken::new();
        let (sender, receiver) = std::sync::mpsc::channel();
        let path = dir.path().to_path_buf();
        let cancel_thread = cancel.clone();
        let handle = tokio::task::spawn_blocking(move || {
            drive(&path, &cancel_thread, || sender.send(()).is_ok());
        });

        tokio::time::sleep(Duration::from_millis(150)).await;
        std::fs::write(dir.path().join("created.txt"), b"x").unwrap();

        let hit = tokio::task::spawn_blocking(move || receiver.recv_timeout(Duration::from_secs(2))).await;
        assert!(hit.expect("watch thread panicked").is_ok(), "creating a file did not reload the watch");

        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("cancel did not stop the watch")
            .expect("watch thread panicked");
    }
}
