mod niri;
mod wayland;

use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, Weak};

use crate::model::{BackendRequest, WindowState};

/// A bounded, coalescing hand-off from a backend thread to GTK.
///
/// Window snapshots supersede one another, so retaining only the newest state
/// avoids unbounded memory growth and prevents GTK's main thread from getting
/// trapped draining an event storm.
#[derive(Clone, Default)]
pub struct EventMailbox {
    latest: Arc<Mutex<Option<Vec<WindowState>>>>,
}

impl EventMailbox {
    pub fn publish(&self, snapshot: Vec<WindowState>) {
        *self
            .latest
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(snapshot);
    }

    pub fn take_latest(&self) -> Option<Vec<WindowState>> {
        self.latest
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
    }
}

#[derive(Clone)]
pub struct BackendController {
    inner: Arc<BackendControllerInner>,
}

struct BackendControllerInner {
    tx: Sender<BackendRequest>,
}

impl BackendController {
    fn new(tx: Sender<BackendRequest>) -> Self {
        Self {
            inner: Arc::new(BackendControllerInner { tx }),
        }
    }

    pub fn activate(&self, id: &str) {
        if self
            .inner
            .tx
            .send(BackendRequest::Activate(id.to_string()))
            .is_err()
        {
            eprintln!("window backend is unavailable; cannot activate {id}");
        }
    }

    pub fn close(&self, id: &str) {
        if self
            .inner
            .tx
            .send(BackendRequest::Close(id.to_string()))
            .is_err()
        {
            eprintln!("window backend is unavailable; cannot close {id}");
        }
    }
}

pub fn spawn(events: EventMailbox) -> Option<BackendController> {
    if std::env::var_os("NIRI_SOCKET").is_some() {
        match niri::spawn(events.clone()) {
            Ok(controller) => return Some(controller),
            Err(error) => {
                eprintln!("failed to start Niri backend ({error}); trying Wayland fallback");
            }
        }
    }

    match wayland::spawn(events) {
        Ok(controller) => Some(controller),
        Err(error) => {
            eprintln!(
                "no supported window-tracking backend is available ({error}); pinned application launching will still work"
            );
            None
        }
    }
}

fn controller_channel() -> (
    BackendController,
    mpsc::Receiver<BackendRequest>,
    Weak<BackendControllerInner>,
) {
    let (tx, rx) = mpsc::channel();
    let controller = BackendController::new(tx);
    let lifetime = Arc::downgrade(&controller.inner);
    (controller, rx, lifetime)
}

#[cfg(test)]
mod tests {
    use super::EventMailbox;
    use crate::model::WindowState;

    fn snapshot(id: &str) -> Vec<WindowState> {
        vec![WindowState {
            id: id.to_string(),
            app_id: None,
            title: None,
            active: false,
            badge_count: None,
        }]
    }

    #[test]
    fn mailbox_keeps_only_the_latest_snapshot() {
        let mailbox = EventMailbox::default();
        mailbox.publish(snapshot("old"));
        mailbox.publish(snapshot("new"));

        assert_eq!(mailbox.take_latest().unwrap()[0].id, "new");
        assert!(mailbox.take_latest().is_none());
    }
}
