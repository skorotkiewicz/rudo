use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc::Sender};
use std::thread;

use wayland_client::protocol::{
    wl_callback::{self, WlCallback},
    wl_output::{self, WlOutput},
    wl_registry::{self, WlRegistry},
    wl_seat::{self, WlSeat},
};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1::{self, ZwlrForeignToplevelHandleV1},
    zwlr_foreign_toplevel_manager_v1::{self, ZwlrForeignToplevelManagerV1},
};

use super::{BackendController, controller_channel};
use crate::model::{BackendEvent, BackendRequest, WindowState};

pub fn spawn(event_tx: Sender<BackendEvent>) -> Option<BackendController> {
    let connection = Connection::connect_to_env().ok()?;
    let (controller, rx) = controller_channel();
    let control = Arc::new(Mutex::new(ControlState::new(connection.clone())));

    {
        let control = Arc::clone(&control);
        thread::Builder::new()
            .name("rudo-wayland-commands".into())
            .spawn(move || command_loop(control, rx))
            .ok()?;
    }

    thread::Builder::new()
        .name("rudo-wayland-events".into())
        .spawn(move || event_loop(connection, control, event_tx))
        .ok()?;

    Some(controller)
}

fn event_loop(
    connection: Connection,
    control: Arc<Mutex<ControlState>>,
    event_tx: Sender<BackendEvent>,
) {
    let mut event_queue = connection.new_event_queue::<WaylandState>();
    let qh = event_queue.handle();
    let _registry = connection.display().get_registry(&qh, ());
    let _sync = connection.display().sync(&qh, ());

    let mut state = WaylandState {
        event_tx,
        control,
        manager: None,
        windows: HashMap::new(),
        next_id: 1,
    };

    if event_queue.roundtrip(&mut state).is_err() || state.manager.is_none() {
        return;
    }

    while event_queue.blocking_dispatch(&mut state).is_ok() {}
}

fn command_loop(control: Arc<Mutex<ControlState>>, rx: std::sync::mpsc::Receiver<BackendRequest>) {
    while let Ok(request) = rx.recv() {
        let Ok(control) = control.lock() else {
            continue;
        };

        let id = match &request {
            BackendRequest::Activate(id) | BackendRequest::Close(id) => id,
        };

        let Some(handle) = control.handles.get(id).cloned() else {
            continue;
        };

        match request {
            BackendRequest::Activate(_) => {
                let Some(seat) = control.seat.clone() else {
                    continue;
                };
                handle.activate(&seat);
            }
            BackendRequest::Close(_) => handle.close(),
        }

        let _ = control.connection.flush();
    }
}

struct ControlState {
    connection: Connection,
    seat: Option<WlSeat>,
    handles: HashMap<String, ZwlrForeignToplevelHandleV1>,
}

impl ControlState {
    fn new(connection: Connection) -> Self {
        Self {
            connection,
            seat: None,
            handles: HashMap::new(),
        }
    }
}

struct WaylandState {
    event_tx: Sender<BackendEvent>,
    control: Arc<Mutex<ControlState>>,
    manager: Option<ZwlrForeignToplevelManagerV1>,
    windows: HashMap<ZwlrForeignToplevelHandleV1, ToplevelState>,
    next_id: u64,
}

#[derive(Clone, Debug)]
struct ToplevelState {
    dock_id: String,
    app_id: Option<String>,
    title: Option<String>,
    active: bool,
}

impl WaylandState {
    fn publish_snapshot(&self) {
        let snapshot = self
            .windows
            .values()
            .map(|window| WindowState {
                id: window.dock_id.clone(),
                app_id: window.app_id.clone(),
                title: window.title.clone(),
                active: window.active,
            })
            .collect();
        let _ = self.event_tx.send(BackendEvent::Snapshot(snapshot));
    }
}

impl Dispatch<WlRegistry, ()> for WaylandState {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            if interface == "zwlr_foreign_toplevel_manager_v1" && state.manager.is_none() {
                let manager = registry.bind::<ZwlrForeignToplevelManagerV1, _, _>(
                    name,
                    version.min(3),
                    qh,
                    (),
                );
                state.manager = Some(manager);
            } else if interface == "wl_seat" {
                let seat = registry.bind::<WlSeat, _, _>(name, version.min(9), qh, ());
                if let Ok(mut control) = state.control.lock() {
                    control.seat.get_or_insert(seat);
                }
            }
        }
    }
}

impl Dispatch<WlCallback, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &WlCallback,
        _: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlSeat, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &WlSeat,
        _: wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlOutput, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &WlOutput,
        _: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for WaylandState {
    fn event(
        state: &mut Self,
        _: &ZwlrForeignToplevelManagerV1,
        event: zwlr_foreign_toplevel_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_foreign_toplevel_manager_v1::Event::Toplevel { toplevel } => {
                let dock_id = format!("wlr-{}", state.next_id);
                state.next_id += 1;

                state.windows.insert(
                    toplevel.clone(),
                    ToplevelState {
                        dock_id: dock_id.clone(),
                        app_id: None,
                        title: None,
                        active: false,
                    },
                );

                if let Ok(mut control) = state.control.lock() {
                    control.handles.insert(dock_id, toplevel);
                }
            }
            zwlr_foreign_toplevel_manager_v1::Event::Finished => {
                state.windows.clear();
                if let Ok(mut control) = state.control.lock() {
                    control.handles.clear();
                }
                state.publish_snapshot();
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwlrForeignToplevelHandleV1, ()> for WaylandState {
    fn event(
        state: &mut Self,
        handle: &ZwlrForeignToplevelHandleV1,
        event: zwlr_foreign_toplevel_handle_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(window) = state.windows.get_mut(handle) else {
            return;
        };

        match event {
            zwlr_foreign_toplevel_handle_v1::Event::Title { title } => {
                window.title = Some(title);
            }
            zwlr_foreign_toplevel_handle_v1::Event::AppId { app_id } => {
                window.app_id = Some(app_id);
            }
            zwlr_foreign_toplevel_handle_v1::Event::State { state: states } => {
                window.active = decode_states(&states)
                    .any(|value| value == zwlr_foreign_toplevel_handle_v1::State::Activated as u32);
            }
            zwlr_foreign_toplevel_handle_v1::Event::Done => state.publish_snapshot(),
            zwlr_foreign_toplevel_handle_v1::Event::Closed => {
                let removed = state.windows.remove(handle);
                if let Some(window) = removed
                    && let Ok(mut control) = state.control.lock()
                {
                    control.handles.remove(&window.dock_id);
                }
                state.publish_snapshot();
            }
            _ => {}
        }
    }
}

fn decode_states(bytes: &[u8]) -> impl Iterator<Item = u32> + '_ {
    bytes.chunks_exact(4).map(|chunk| {
        let bytes: [u8; 4] = chunk.try_into().expect("state value must be four bytes");
        u32::from_ne_bytes(bytes)
    })
}
