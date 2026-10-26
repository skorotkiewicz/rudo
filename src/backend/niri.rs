use std::collections::BTreeMap;
use std::env;
use std::io::{BufRead, BufReader, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Weak;
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{BackendController, BackendControllerInner, EventMailbox, controller_channel};
use crate::model::{BackendRequest, WindowState};

pub fn spawn(events: EventMailbox) -> Result<BackendController, NiriError> {
    let socket_path = PathBuf::from(env::var_os("NIRI_SOCKET").ok_or(NiriError::NoSocket)?);
    let initial_stream = UnixStream::connect(&socket_path)?;
    let (controller, rx, lifetime) = controller_channel();

    let command_socket_path = socket_path.clone();
    thread::Builder::new()
        .name("rudo-niri-commands".into())
        .spawn(move || command_loop(&command_socket_path, &rx))
        .map_err(NiriError::Thread)?;

    thread::Builder::new()
        .name("rudo-niri-events".into())
        .spawn(move || event_loop(&socket_path, initial_stream, &events, &lifetime))
        .map_err(NiriError::Thread)?;

    Ok(controller)
}

fn event_loop(
    socket_path: &PathBuf,
    initial_stream: UnixStream,
    events: &EventMailbox,
    lifetime: &Weak<BackendControllerInner>,
) {
    let mut stream = Some(initial_stream);
    let mut retry_delay = Duration::from_secs(1);

    while lifetime.upgrade().is_some() {
        let started_at = Instant::now();
        let result = match stream.take() {
            Some(stream) => event_session(stream, events),
            None => UnixStream::connect(socket_path)
                .map_err(NiriError::Connection)
                .and_then(|stream| event_session(stream, events)),
        };

        if lifetime.upgrade().is_none() {
            break;
        }

        if started_at.elapsed() >= Duration::from_secs(30) {
            retry_delay = Duration::from_secs(1);
        }

        if let Err(error) = result {
            eprintln!(
                "Niri event stream stopped ({error}); reconnecting in {}s",
                retry_delay.as_secs()
            );
        }

        thread::sleep(retry_delay);
        retry_delay = (retry_delay * 2).min(Duration::from_secs(30));
    }
}

fn event_session(stream: UnixStream, events: &EventMailbox) -> Result<(), NiriError> {
    let result = read_event_stream(stream, events);
    // A disconnected compositor can no longer vouch for the previous windows.
    events.publish(Vec::new());
    result
}

fn read_event_stream(mut stream: UnixStream, events: &EventMailbox) -> Result<(), NiriError> {
    write_json_line(&mut stream, &Request::EventStream)?;

    let reader_stream = stream.try_clone()?;
    let mut reader = BufReader::new(reader_stream);

    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Err(NiriError::ConnectionClosed);
    }

    let reply = serde_json::from_str::<Reply>(&line)?;
    if !matches!(reply, Ok(Response::Handled)) {
        return Err(NiriError::Protocol(
            "event stream request was rejected".to_string(),
        ));
    }

    let mut windows = BTreeMap::<u64, Window>::new();
    let mut workspaces = BTreeMap::<u64, Workspace>::new();
    let mut logged_invalid_event = false;

    loop {
        line.clear();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            return Err(NiriError::ConnectionClosed);
        }

        let event = match serde_json::from_str::<Event>(&line) {
            Ok(event) => event,
            Err(error) => {
                if !logged_invalid_event {
                    eprintln!(
                        "ignoring malformed Niri event; further parse errors on this connection will be suppressed: {error}"
                    );
                    logged_invalid_event = true;
                }
                continue;
            }
        };

        apply_event(event, events, &mut windows, &mut workspaces);
    }
}

fn apply_event(
    event: Event,
    events: &EventMailbox,
    windows: &mut BTreeMap<u64, Window>,
    workspaces: &mut BTreeMap<u64, Workspace>,
) {
    match event {
        Event::WorkspacesChanged {
            workspaces: snapshot,
        } => {
            // This is a full replacement, including workspace moves between outputs.
            *workspaces = snapshot
                .into_iter()
                .map(|workspace| (workspace.id, workspace))
                .collect();
        }
        Event::WindowsChanged { windows: snapshot } => {
            *windows = snapshot
                .into_iter()
                .map(|window| (window.id, window))
                .collect();
        }
        Event::WindowOpenedOrChanged { window } => {
            if window.is_focused {
                for w in windows.values_mut() {
                    w.is_focused = false;
                }
            }
            windows.insert(window.id, window);
        }
        Event::WindowClosed { id } => {
            windows.remove(&id);
        }
        Event::WindowFocusChanged { id } => {
            for (window_id, window) in windows.iter_mut() {
                window.is_focused = Some(*window_id) == id;
            }
        }
        Event::Other => return,
    }
    publish_snapshot(events, windows, workspaces);
}

fn command_loop(socket_path: &PathBuf, rx: &std::sync::mpsc::Receiver<BackendRequest>) {
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

        let mut stream = match UnixStream::connect(socket_path) {
            Ok(stream) => stream,
            Err(error) => {
                eprintln!("failed to connect to Niri for window action: {error}");
                continue;
            }
        };

        if let Err(error) = write_json_line(&mut stream, &request) {
            eprintln!("failed to send Niri window action: {error}");
        }
        let _ = stream.shutdown(Shutdown::Write);
    }
}

fn publish_snapshot(
    events: &EventMailbox,
    windows: &BTreeMap<u64, Window>,
    workspaces: &BTreeMap<u64, Workspace>,
) {
    let snapshot = windows
        .values()
        .map(|window| WindowState {
            id: format!("niri-{}", window.id),
            app_id: window.app_id.clone(),
            title: window.title.clone(),
            active: window.is_focused,
            badge_count: None,
            // Missing workspace metadata is transiently possible during snapshots.
            // A known workspace with no output means no outputs are connected.
            outputs: window
                .workspace_id
                .and_then(|id| workspaces.get(&id))
                .map(|workspace| workspace.output.iter().cloned().collect()),
        })
        .collect();
    events.publish(snapshot);
}

#[derive(Debug, Error)]
pub(super) enum NiriError {
    #[error("NIRI_SOCKET is not set")]
    NoSocket,
    #[error("Failed to connect to Niri socket: {0}")]
    Connection(#[from] std::io::Error),
    #[error("Failed to serialize request: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("Niri event stream closed")]
    ConnectionClosed,
    #[error("Niri protocol error: {0}")]
    Protocol(String),
    #[error("Failed to spawn backend thread: {0}")]
    Thread(std::io::Error),
}

fn write_json_line<T: Serialize>(stream: &mut UnixStream, value: &T) -> Result<(), NiriError> {
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
    WorkspacesChanged {
        workspaces: Vec<Workspace>,
    },
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
    workspace_id: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct Workspace {
    id: u64,
    output: Option<String>,
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

#[cfg(test)]
mod tests {
    use super::{NiriError, apply_event, event_session};
    use crate::backend::EventMailbox;
    use crate::model::WindowState;
    use std::collections::BTreeMap;
    use std::io::Write;
    use std::net::Shutdown;
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    const WORKSPACES: &str = r#"{"WorkspacesChanged":{"workspaces":[{"id":1,"output":"DP-1"},{"id":2,"output":"HDMI-A-1"}]}}"#;
    const WINDOWS: &str = r#"{"WindowsChanged":{"windows":[{"id":7,"title":"Notes","app_id":"obsidian","is_focused":true,"workspace_id":1}]}}"#;

    fn replay(json_events: &[&str]) -> Vec<WindowState> {
        let events = EventMailbox::default();
        let mut windows = BTreeMap::new();
        let mut workspaces = BTreeMap::new();
        json_events
            .iter()
            .map(|json| {
                let event =
                    serde_json::from_str(json).expect("test event must match the IPC schema");
                apply_event(event, &events, &mut windows, &mut workspaces);
                events
                    .take_latest()
                    .expect("each state-changing event must republish a snapshot")
            })
            .last()
            .expect("test must supply at least one event")
    }

    #[test]
    fn window_move_updates_output() {
        let moved = r#"{"WindowOpenedOrChanged":{"window":{"id":7,"title":"Notes","app_id":"obsidian","is_focused":true,"workspace_id":2}}}"#;
        let snapshot = replay(&[WORKSPACES, WINDOWS, moved]);
        assert_eq!(snapshot[0].outputs, Some(vec!["HDMI-A-1".into()]));
        assert_eq!(snapshot[0].id, "niri-7");
        assert_eq!(snapshot[0].app_id.as_deref(), Some("obsidian"));
        assert_eq!(snapshot[0].title.as_deref(), Some("Notes"));
        assert!(
            snapshot[0].active,
            "moving a focused window must preserve focus"
        );
    }

    #[test]
    fn workspace_move_updates_existing_windows() {
        let moved = r#"{"WorkspacesChanged":{"workspaces":[{"id":1,"output":"HDMI-A-1"},{"id":2,"output":"DP-1"}]}}"#;
        assert_eq!(
            replay(&[WORKSPACES, WINDOWS, moved])[0].outputs,
            Some(vec!["HDMI-A-1".into()])
        );
    }

    #[test]
    fn unknown_workspace_metadata_falls_back_to_global() {
        let missing_id = r#"{"WindowsChanged":{"windows":[{"id":7,"title":null,"app_id":null,"is_focused":false}]}}"#;
        let null_id = r#"{"WindowOpenedOrChanged":{"window":{"id":7,"title":null,"app_id":null,"is_focused":false,"workspace_id":null}}}"#;
        let removed = r#"{"WorkspacesChanged":{"workspaces":[]}}"#;
        assert_eq!(replay(&[WINDOWS])[0].outputs, None);
        assert_eq!(replay(&[WORKSPACES, missing_id])[0].outputs, None);
        assert_eq!(replay(&[WORKSPACES, WINDOWS, null_id])[0].outputs, None);
        assert_eq!(replay(&[WORKSPACES, WINDOWS, removed])[0].outputs, None);
    }

    #[test]
    fn known_workspace_without_output_has_empty_membership() {
        let no_output = r#"{"WorkspacesChanged":{"workspaces":[{"id":1,"output":null}]}}"#;
        assert_eq!(replay(&[WINDOWS, no_output])[0].outputs, Some(Vec::new()));
    }

    #[test]
    fn initial_snapshots_resolve_membership_in_either_order() {
        for order in [[WORKSPACES, WINDOWS], [WINDOWS, WORKSPACES]] {
            assert_eq!(replay(&order)[0].outputs, Some(vec!["DP-1".into()]));
        }
    }

    #[test]
    fn disconnect_clears_previously_published_windows() {
        let events = EventMailbox::default();
        let (client, mut server) = UnixStream::pair().expect("Unix socket pair must be available");
        server
            .write_all(b"{\"Ok\":\"Handled\"}\n{\"WindowsChanged\":{\"windows\":[{\"id\":7,\"title\":\"Notes\",\"app_id\":\"obsidian\",\"is_focused\":true}]}}\n")
            .expect("test stream must be writable");
        server
            .shutdown(Shutdown::Write)
            .expect("server write side must close");
        client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .expect("socket timeout must be configurable");

        assert!(
            matches!(
                event_session(client, &events),
                Err(NiriError::ConnectionClosed)
            ),
            "EOF must terminate the session"
        );
        assert_eq!(
            events.take_latest(),
            Some(Vec::new()),
            "disconnected windows must not remain actionable"
        );
    }
}
