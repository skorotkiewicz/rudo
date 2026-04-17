mod niri;
mod wayland;

use std::sync::mpsc::{self, Sender};

use crate::model::{BackendEvent, BackendRequest};

#[derive(Clone)]
pub struct BackendController {
    tx: Sender<BackendRequest>,
}

impl BackendController {
    fn new(tx: Sender<BackendRequest>) -> Self {
        Self { tx }
    }

    pub fn activate(&self, id: &str) {
        let _ = self.tx.send(BackendRequest::Activate(id.to_string()));
    }

    pub fn close(&self, id: &str) {
        let _ = self.tx.send(BackendRequest::Close(id.to_string()));
    }
}

pub fn spawn(event_tx: Sender<BackendEvent>) -> Option<BackendController> {
    if std::env::var_os("NIRI_SOCKET").is_some()
        && let Some(controller) = niri::spawn(event_tx.clone())
    {
        return Some(controller);
    }

    wayland::spawn(event_tx)
}

fn controller_channel() -> (BackendController, mpsc::Receiver<BackendRequest>) {
    let (tx, rx) = mpsc::channel();
    (BackendController::new(tx), rx)
}
