use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::thread;
use std::time::{Duration, Instant};

use thiserror::Error;
use wayland_client::protocol::{
    wl_callback::{self, WlCallback},
    wl_output::{self, WlOutput},
    wl_registry::{self, WlRegistry},
    wl_seat::{self, WlSeat},
};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1::{self, ZwlrForeignToplevelHandleV1},
    zwlr_foreign_toplevel_manager_v1::{self, ZwlrForeignToplevelManagerV1},
};

use super::{BackendController, BackendControllerInner, EventMailbox, controller_channel};
use crate::model::{BackendRequest, WindowState};

pub fn spawn(events: EventMailbox) -> Result<BackendController, WlrError> {
    let connection = Connection::connect_to_env().map_err(WlrError::Connect)?;
    let control = Arc::new(Mutex::new(ControlState::new(connection.clone())));
    let next_id = Arc::new(AtomicU64::new(1));
    let session = WaylandSession::new(
        connection,
        Arc::clone(&control),
        events.clone(),
        Arc::clone(&next_id),
    )?;
    let (controller, rx, lifetime) = controller_channel();

    {
        let control = Arc::clone(&control);
        thread::Builder::new()
            .name("rudo-wayland-commands".into())
            .spawn(move || command_loop(control, rx))
            .map_err(WlrError::Thread)?;
    }

    thread::Builder::new()
        .name("rudo-wayland-events".into())
        .spawn(move || event_loop(session, control, events, next_id, lifetime))
        .map_err(WlrError::Thread)?;

    Ok(controller)
}

struct WaylandSession {
    event_queue: EventQueue<WaylandState>,
    state: WaylandState,
    _registry: WlRegistry,
}

impl WaylandSession {
    fn connect(
        control: Arc<Mutex<ControlState>>,
        events: EventMailbox,
        next_id: Arc<AtomicU64>,
    ) -> Result<Self, WlrError> {
        let connection = Connection::connect_to_env().map_err(WlrError::Connect)?;
        Self::new(connection, control, events, next_id)
    }

    fn new(
        connection: Connection,
        control: Arc<Mutex<ControlState>>,
        events: EventMailbox,
        next_id: Arc<AtomicU64>,
    ) -> Result<Self, WlrError> {
        {
            let mut control = control.lock().map_err(|_| WlrError::ControlPoisoned)?;
            *control = ControlState::new(connection.clone());
        }

        let mut event_queue = connection.new_event_queue::<WaylandState>();
        let qh = event_queue.handle();
        let registry = connection.display().get_registry(&qh, ());
        let mut state = WaylandState {
            events,
            control,
            manager: None,
            manager_finished: false,
            windows: HashMap::new(),
            next_id,
        };

        event_queue
            .roundtrip(&mut state)
            .map_err(WlrError::Dispatch)?;
        if state.manager.is_none() {
            return Err(WlrError::UnsupportedProtocol);
        }
        if state.manager_finished {
            return Err(WlrError::ManagerFinished);
        }

        Ok(Self {
            event_queue,
            state,
            _registry: registry,
        })
    }

    fn run(mut self) -> Result<(), WlrError> {
        loop {
            self.event_queue
                .blocking_dispatch(&mut self.state)
                .map_err(WlrError::Dispatch)?;
            if self.state.manager_finished {
                return Err(WlrError::ManagerFinished);
            }
        }
    }
}

fn event_loop(
    initial_session: WaylandSession,
    control: Arc<Mutex<ControlState>>,
    events: EventMailbox,
    next_id: Arc<AtomicU64>,
    lifetime: Weak<BackendControllerInner>,
) {
    let mut session = Some(initial_session);
    let mut retry_delay = Duration::from_secs(1);

    while lifetime.upgrade().is_some() {
        let started_at = Instant::now();
        let result = match session.take() {
            Some(session) => session.run(),
            None => {
                WaylandSession::connect(Arc::clone(&control), events.clone(), Arc::clone(&next_id))
                    .and_then(WaylandSession::run)
            }
        };

        if lifetime.upgrade().is_none() {
            break;
        }

        if let Ok(mut control) = control.lock() {
            control.seat = None;
            control.handles.clear();
        }
        events.publish(Vec::new());

        if started_at.elapsed() >= Duration::from_secs(30) {
            retry_delay = Duration::from_secs(1);
        }
        if let Err(error) = result {
            eprintln!(
                "Wayland backend stopped ({error}); reconnecting in {}s",
                retry_delay.as_secs()
            );
        }

        thread::sleep(retry_delay);
        retry_delay = (retry_delay * 2).min(Duration::from_secs(30));
    }
}

#[derive(Debug, Error)]
pub(super) enum WlrError {
    #[error("failed to connect to Wayland: {0}")]
    Connect(wayland_client::ConnectError),
    #[error("Wayland dispatch failed: {0}")]
    Dispatch(wayland_client::DispatchError),
    #[error("compositor does not support wlr-foreign-toplevel-management")]
    UnsupportedProtocol,
    #[error("wlr-foreign-toplevel-management became unavailable")]
    ManagerFinished,
    #[error("Wayland control state is poisoned")]
    ControlPoisoned,
    #[error("failed to spawn backend thread: {0}")]
    Thread(std::io::Error),
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
    events: EventMailbox,
    control: Arc<Mutex<ControlState>>,
    manager: Option<ZwlrForeignToplevelManagerV1>,
    manager_finished: bool,
    windows: HashMap<ZwlrForeignToplevelHandleV1, ToplevelState>,
    next_id: Arc<AtomicU64>,
}

#[derive(Clone, Debug)]
struct ToplevelState {
    dock_id: String,
    app_id: Option<String>,
    title: Option<String>,
    active: bool,
    badge_count: Option<u32>,
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
                badge_count: window.badge_count,
            })
            .collect();
        self.events.publish(snapshot);
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
        // Callback events are handled by the event queue; no action needed
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
        // Seat capabilities not used directly; stored in ControlState via registry
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
        // Output events not used by this dock implementation
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
                let dock_id = format!("wlr-{}", state.next_id.fetch_add(1, Ordering::Relaxed));

                state.windows.insert(
                    toplevel.clone(),
                    ToplevelState {
                        dock_id: dock_id.clone(),
                        app_id: None,
                        title: None,
                        active: false,
                        badge_count: None,
                    },
                );

                if let Ok(mut control) = state.control.lock() {
                    control.handles.insert(dock_id, toplevel);
                }
            }
            zwlr_foreign_toplevel_manager_v1::Event::Finished => {
                state.manager_finished = true;
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
            // Note: Badge events are a compositor-specific extension
            // (e.g., Hyprland adds custom events). Standard foreign_toplevel v1
            // doesn't support badges. For now, badge support is limited to
            // compositors that provide this data through other means.
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
