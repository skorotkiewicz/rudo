use std::collections::BTreeMap;
use std::env;
use std::io::{BufRead, BufReader, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::thread;

use serde::{Deserialize, Serialize};

use super::{BackendController, controller_channel};
use crate::model::{BackendEvent, BackendRequest, WindowState};

pub fn spawn(event_tx: Sender<BackendEvent>) -> Option<BackendController> {
    let socket_path = PathBuf::from(env::var_os("NIRI_SOCKET")?);
    let (controller, rx) = controller_channel();

    {
        let socket_path = socket_path.clone();
        thread::Builder::new()
            .name("rudo-niri-events".into())
            .spawn(move || event_loop(socket_path, event_tx))
            .ok()?;
    }

    thread::Builder::new()
        .name("rudo-niri-commands".into())
        .spawn(move || command_loop(socket_path, rx))
        .ok()?;

    Some(controller)
}

fn event_loop(socket_path: PathBuf, event_tx: Sender<BackendEvent>) {
    let Ok(mut stream) = UnixStream::connect(&socket_path) else {
        return;
    };

    if write_json_line(&mut stream, &Request::EventStream).is_err() {
        return;
    }

    let Ok(reader_stream) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(reader_stream);

    let mut line = String::new();
    if reader
        .read_line(&mut line)
        .ok()
        .filter(|read| *read > 0)
        .is_none()
    {
        return;
    }

    let Ok(reply) = serde_json::from_str::<Reply>(&line) else {
        return;
    };
    if !matches!(reply, Ok(Response::Handled)) {
        return;
    }

    let mut windows = BTreeMap::<u64, Window>::new();

    loop {
        line.clear();
        let Ok(read) = reader.read_line(&mut line) else {
            break;
        };
        if read == 0 {
            break;
        }

        let Ok(event) = serde_json::from_str::<Event>(&line) else {
            continue;
        };

        match event {
            Event::WindowsChanged { windows: snapshot } => {
                windows = snapshot
                    .into_iter()
                    .map(|window| (window.id, window))
                    .collect();
                publish_snapshot(&event_tx, &windows);
            }
            Event::WindowOpenedOrChanged { window } => {
                windows.insert(window.id, window);
                publish_snapshot(&event_tx, &windows);
            }
            Event::WindowClosed { id } => {
                windows.remove(&id);
                publish_snapshot(&event_tx, &windows);
            }
            Event::WindowFocusChanged { id } => {
                for (window_id, window) in &mut windows {
                    window.is_focused = Some(*window_id) == id;
                }
                publish_snapshot(&event_tx, &windows);
            }
            _ => {}
        }
    }
}

fn command_loop(socket_path: PathBuf, rx: std::sync::mpsc::Receiver<BackendRequest>) {
    while let Ok(request) = rx.recv() {
        let id = match &request {
            BackendRequest::Activate(id) | BackendRequest::Close(id) => id,
        };

        let Some(window_id) = id
            .strip_prefix("niri-")
            .and_then(|value| value.parse::<u64>().ok())
        else {
            continue;
        };

        let request = match request {
            BackendRequest::Activate(_) => Request::Action(Action::FocusWindow { id: window_id }),
            BackendRequest::Close(_) => Request::Action(Action::CloseWindow {
                id: Some(window_id),
            }),
        };

        let Ok(mut stream) = UnixStream::connect(&socket_path) else {
            continue;
        };

        let _ = write_json_line(&mut stream, &request);
        let _ = stream.shutdown(Shutdown::Write);
    }
}

fn publish_snapshot(event_tx: &Sender<BackendEvent>, windows: &BTreeMap<u64, Window>) {
    let snapshot = windows
        .values()
        .map(|window| WindowState {
            id: format!("niri-{}", window.id),
            app_id: window.app_id.clone(),
            title: window.title.clone(),
            active: window.is_focused,
            badge_count: None, // Niri doesn't support badge notifications yet
            output_id: None,   // Niri doesn't provide output info yet
            coordinates: None, // Niri doesn't provide window coordinates yet
        })
        .collect();
    let _ = event_tx.send(BackendEvent::Snapshot(snapshot));
}

fn write_json_line<T: Serialize>(
    stream: &mut UnixStream,
    value: &T,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    serde_json::to_writer(&mut *stream, value)?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    Ok(())
}

type Reply = Result<Response, String>;

#[derive(Debug, Deserialize)]
enum Response {
    Handled,
}

#[derive(Debug, Deserialize)]
enum Event {
    WindowsChanged {
        windows: Vec<Window>,
    },
    WindowOpenedOrChanged {
        window: Window,
    },
    WindowClosed {
        id: u64,
    },
    WindowFocusChanged {
        id: Option<u64>,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize, Clone)]
struct Window {
    id: u64,
    title: Option<String>,
    app_id: Option<String>,
    is_focused: bool,
}

#[derive(Debug, Serialize)]
enum Request {
    EventStream,
    Action(Action),
}

#[derive(Debug, Serialize)]
enum Action {
    FocusWindow { id: u64 },
    CloseWindow { id: Option<u64> },
}
