use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use gtk::gdk;
use gtk::glib::{self, ControlFlow};
use gtk::prelude::*;
use gtk4 as gtk;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use crate::backend::{self, BackendController};
use crate::catalog::{AppCatalog, AppRecord};
use crate::config;
use crate::model::{BackendEvent, WindowState};

pub fn run() {
    let app = gtk::Application::builder()
        .application_id("dev.rudo.dock")
        .build();

    app.connect_activate(build_ui);
    app.run();
}

#[derive(Clone)]
struct DockItem {
    label: String,
    tooltip: String,
    app: Option<AppRecord>,
    windows: Vec<WindowState>,
    pinned: bool,
    active: bool,
    launching: bool,
}

struct DockState {
    catalog: AppCatalog,
    pins: Vec<String>,
    windows: Vec<WindowState>,
    backend: Option<BackendController>,
    launching: HashMap<String, Instant>,
}

impl DockState {
    fn mark_launching(&mut self, app_id: &str) {
        self.launching.insert(app_id.to_string(), Instant::now());
    }

    fn is_launching(&self, app_id: &str) -> bool {
        self.launching.contains_key(app_id)
    }

    fn prune_launching(&mut self) {
        self.launching
            .retain(|_, started_at| started_at.elapsed() < LAUNCH_TIMEOUT);
    }

    fn reconcile_launching(&mut self) {
        let opened_apps = self
            .windows
            .iter()
            .filter_map(|window| self.catalog.resolve(window.app_id.as_deref()))
            .map(|app| app.id)
            .collect::<HashSet<_>>();

        self.launching
            .retain(|app_id, _| !opened_apps.contains(app_id.as_str()));
    }
}

struct AutoHideState {
    revealer: gtk::Revealer,
    hide_source: Option<glib::SourceId>,
    enabled: bool,
    delay: Duration,
}

impl AutoHideState {
    fn new(revealer: &gtk::Revealer, enabled: bool, delay: Duration) -> Self {
        Self {
            revealer: revealer.clone(),
            hide_source: None,
            enabled,
            delay,
        }
    }
}

fn build_ui(app: &gtk::Application) {
    install_css();
    config::ensure_settings();
    let settings = config::load_settings();

    let catalog = AppCatalog::load();
    let mut pins = config::load_pins();
    pins.retain(|id| catalog.app(id).is_some());
    config::save_pins(&pins);

    let (backend_tx, backend_rx) = mpsc::channel();
    let backend = backend::spawn(backend_tx);

    let state = Rc::new(RefCell::new(DockState {
        catalog,
        pins,
        windows: Vec::new(),
        backend,
        launching: HashMap::new(),
    }));

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Rudo")
        .default_width(10)
        .default_height(10)
        .resizable(false)
        .build();
    window.add_css_class("rudo-window");

    if gtk4_layer_shell::is_supported() {
        window.init_layer_shell();
        window.set_namespace(Some("rudo-dock"));
        window.set_layer(Layer::Top);
        window.set_keyboard_mode(KeyboardMode::None);
        window.set_anchor(Edge::Bottom, true);
        window.set_margin(Edge::Bottom, 6);
        window.auto_exclusive_zone_enable();
    } else {
        window.set_decorated(false);
    }

    let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    outer.set_halign(gtk::Align::Center);
    outer.set_valign(gtk::Align::End);
    outer.set_margin_bottom(0);

    let dock_revealer = gtk::Revealer::builder()
        .transition_type(gtk::RevealerTransitionType::SlideUp)
        .transition_duration(220)
        .reveal_child(true)
        .build();

    let dock_surface = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    dock_surface.add_css_class("dock-surface");

    let items_box = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    items_box.set_valign(gtk::Align::Center);

    let picker_button = gtk::Button::new();
    picker_button.add_css_class("dock-item");
    picker_button.add_css_class("picker-button");
    picker_button.set_tooltip_text(Some("Pin an application"));
    picker_button.set_child(Some(&icon_widget(None)));

    let picker_popover = gtk::Popover::new();
    picker_popover.set_has_arrow(false);
    picker_popover.set_position(gtk::PositionType::Top);
    picker_popover.set_parent(&picker_button);

    let picker_layout = gtk::Box::new(gtk::Orientation::Vertical, 10);
    picker_layout.add_css_class("picker");
    let picker_search = gtk::SearchEntry::new();
    picker_search.set_placeholder_text(Some("Pin an app"));
    let picker_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_width(320)
        .min_content_height(360)
        .build();
    let picker_list = gtk::Box::new(gtk::Orientation::Vertical, 6);
    picker_scroll.set_child(Some(&picker_list));
    picker_layout.append(&picker_search);
    picker_layout.append(&picker_scroll);
    picker_popover.set_child(Some(&picker_layout));

    dock_surface.append(&items_box);
    dock_surface.append(&picker_button);
    dock_revealer.set_child(Some(&dock_surface));
    outer.append(&dock_revealer);

    let hover_strip = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    hover_strip.add_css_class("dock-hover-strip");
    hover_strip.set_halign(gtk::Align::Center);
    hover_strip.set_hexpand(true);
    hover_strip.set_visible(settings.autohide.enabled);
    outer.append(&hover_strip);
    window.set_child(Some(&outer));

    render_dock(&state, &items_box, &picker_search, &picker_list);

    let autohide = Rc::new(RefCell::new(AutoHideState::new(
        &dock_revealer,
        settings.autohide.enabled,
        Duration::from_secs(settings.autohide.delay_secs.max(1)),
    )));

    {
        let state = Rc::clone(&state);
        let items_box = items_box.clone();
        let picker_search = picker_search.clone();
        let picker_list = picker_list.clone();
        let picker_popover = picker_popover.clone();
        picker_button.connect_clicked(move |_| {
            picker_search.set_text("");
            render_picker(&state, &picker_list, "");
            picker_popover.popup();
            picker_search.grab_focus();
            render_dock(&state, &items_box, &picker_search, &picker_list);
        });
    }

    {
        let state = Rc::clone(&state);
        let picker_list = picker_list.clone();
        picker_search.connect_search_changed(move |entry| {
            render_picker(&state, &picker_list, entry.text().as_ref());
        });
    }

    {
        let state = Rc::clone(&state);
        let items_box = items_box.clone();
        let picker_search = picker_search.clone();
        let picker_list = picker_list.clone();
        let backend_rx = backend_rx;

        glib::timeout_add_local(Duration::from_millis(80), move || {
            let mut changed = false;
            while let Ok(event) = backend_rx.try_recv() {
                let BackendEvent::Snapshot(snapshot) = event;
                let mut dock_state = state.borrow_mut();
                dock_state.windows = snapshot;
                dock_state.reconcile_launching();
                changed = true;
            }

            if changed {
                render_dock(&state, &items_box, &picker_search, &picker_list);
            }

            ControlFlow::Continue
        });
    }

    {
        let autohide = Rc::clone(&autohide);
        let motion = gtk::EventControllerMotion::new();
        motion.connect_enter(move |_, _, _| show_dock(&autohide));
        window.add_controller(motion);
    }

    {
        let autohide = Rc::clone(&autohide);
        let motion = gtk::EventControllerMotion::new();
        motion.connect_enter(move |_, _, _| show_dock(&autohide));
        hover_strip.add_controller(motion);
    }

    {
        let autohide = Rc::clone(&autohide);
        let motion = gtk::EventControllerMotion::new();
        motion.connect_leave(move |_| schedule_hide(&autohide));
        window.add_controller(motion);
    }

    schedule_hide(&autohide);

    window.present();
}

fn render_dock(
    state: &Rc<RefCell<DockState>>,
    items_box: &gtk::Box,
    picker_search: &gtk::SearchEntry,
    picker_list: &gtk::Box,
) {
    clear_children(items_box);

    let (pinned_items, running_items) = {
        let mut dock_state = state.borrow_mut();
        dock_state.prune_launching();
        dock_state.reconcile_launching();
        collect_items(&dock_state)
    };
    let show_separator = !pinned_items.is_empty() && !running_items.is_empty();

    for item in pinned_items {
        items_box.append(&build_item_widget(
            state,
            item,
            items_box,
            picker_search,
            picker_list,
        ));
    }

    if show_separator {
        let separator = gtk::Separator::new(gtk::Orientation::Vertical);
        separator.add_css_class("dock-separator");
        items_box.append(&separator);
    }

    for item in running_items {
        items_box.append(&build_item_widget(
            state,
            item,
            items_box,
            picker_search,
            picker_list,
        ));
    }

    render_picker(state, picker_list, picker_search.text().as_ref());
}

fn build_item_widget(
    state: &Rc<RefCell<DockState>>,
    item: DockItem,
    items_box: &gtk::Box,
    picker_search: &gtk::SearchEntry,
    picker_list: &gtk::Box,
) -> gtk::Box {
    let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 4);
    wrapper.set_valign(gtk::Align::Center);

    let button = gtk::Button::new();
    button.add_css_class("dock-item");
    if item.active {
        button.add_css_class("is-active");
    }
    if !item.windows.is_empty() {
        button.add_css_class("is-running");
    }
    if item.launching {
        button.add_css_class("is-launching");
    }
    button.set_tooltip_text(Some(&item.tooltip));
    button.set_child(Some(&item_visual(item.app.as_ref(), item.launching)));

    let indicator = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    indicator.add_css_class("dock-indicator");
    if item.active {
        indicator.add_css_class("is-active");
    }
    if item.windows.is_empty() && !item.launching {
        indicator.set_opacity(0.0);
    }

    {
        let state = Rc::clone(state);
        let windows = item.windows.clone();
        let app = item.app.clone();
        let items_box = items_box.clone();
        let picker_search = picker_search.clone();
        let picker_list = picker_list.clone();
        button.connect_clicked(move |_| {
            if let Some(window) = windows
                .iter()
                .find(|window| window.active)
                .or_else(|| windows.first())
            {
                if let Some(backend) = state.borrow().backend.as_ref() {
                    backend.activate(&window.id);
                }
            } else if let Some(app) = app.as_ref() {
                {
                    let dock_state = state.borrow();
                    if dock_state.is_launching(&app.id) {
                        return;
                    }
                }

                match app.launch() {
                    Ok(()) => {
                        state.borrow_mut().mark_launching(&app.id);
                        render_dock(&state, &items_box, &picker_search, &picker_list);
                    }
                    Err(error) => eprintln!("failed to launch {}: {error}", app.id),
                }
            }
        });
    }

    {
        let state = Rc::clone(state);
        let item = item.clone();
        let items_box = items_box.clone();
        let picker_search = picker_search.clone();
        let picker_list = picker_list.clone();
        let popover = build_context_menu(Rc::clone(&state), &button, item, move || {
            render_dock(&state, &items_box, &picker_search, &picker_list)
        });

        let gesture = gtk::GestureClick::new();
        gesture.set_button(gdk::BUTTON_SECONDARY);
        gesture.connect_pressed(move |_, _, _, _| {
            popover.popup();
        });
        button.add_controller(gesture);
    }

    {
        let state = Rc::clone(state);
        let windows = item.windows.clone();
        let middle = gtk::GestureClick::new();
        middle.set_button(gdk::BUTTON_MIDDLE);
        middle.connect_pressed(move |_, _, _, _| {
            if let Some(window) = windows
                .iter()
                .find(|window| window.active)
                .or_else(|| windows.first())
                && let Some(backend) = state.borrow().backend.as_ref()
            {
                backend.close(&window.id);
            }
        });
        button.add_controller(middle);
    }

    wrapper.append(&button);
    wrapper.append(&indicator);
    wrapper
}

fn build_context_menu(
    state: Rc<RefCell<DockState>>,
    parent: &impl gtk::prelude::IsA<gtk::Widget>,
    item: DockItem,
    rerender: impl Fn() + 'static,
) -> gtk::Popover {
    let popover = gtk::Popover::new();
    popover.set_has_arrow(false);
    popover.set_position(gtk::PositionType::Top);
    popover.set_parent(parent);

    let layout = gtk::Box::new(gtk::Orientation::Vertical, 6);
    layout.add_css_class("item-menu");

    if let Some(app) = item.app.clone() {
        let new_window = gtk::Button::with_label("Open New Window");
        {
            let app = app.clone();
            new_window.connect_clicked(move |_| {
                if let Err(error) = app.launch() {
                    eprintln!("failed to launch {}: {error}", app.id);
                }
            });
        }
        layout.append(&new_window);

        let toggle_label = if item.pinned {
            "Unpin from Dock"
        } else {
            "Pin to Dock"
        };
        let toggle_pin = gtk::Button::with_label(toggle_label);
        {
            let state = Rc::clone(&state);
            let id = app.id.clone();
            let popover = popover.clone();
            toggle_pin.connect_clicked(move |_| {
                let mut state = state.borrow_mut();
                if let Some(position) = state.pins.iter().position(|pin| pin == &id) {
                    state.pins.remove(position);
                } else {
                    state.pins.push(id.clone());
                }
                config::save_pins(&state.pins);
                drop(state);
                popover.popdown();
                rerender();
            });
        }
        layout.append(&toggle_pin);
    }

    if !item.windows.is_empty() {
        let close_all = gtk::Button::with_label(if item.windows.len() > 1 {
            "Close All Windows"
        } else {
            "Close Window"
        });
        {
            let state = Rc::clone(&state);
            let windows = item.windows.clone();
            let popover = popover.clone();
            close_all.connect_clicked(move |_| {
                if let Some(backend) = state.borrow().backend.as_ref() {
                    for window in &windows {
                        backend.close(&window.id);
                    }
                }
                popover.popdown();
            });
        }
        layout.append(&close_all);
    }

    popover.set_child(Some(&layout));
    popover
}

fn render_picker(state: &Rc<RefCell<DockState>>, picker_list: &gtk::Box, query: &str) {
    clear_children(picker_list);

    let borrow = state.borrow();
    let exclude = borrow.pins.iter().cloned().collect::<HashSet<_>>();
    let matches = borrow.catalog.search(query, 40, &exclude);
    drop(borrow);

    if matches.is_empty() {
        let empty = gtk::Label::new(Some("No matching applications"));
        empty.add_css_class("picker-empty");
        picker_list.append(&empty);
        return;
    }

    for app in matches {
        let row_button = gtk::Button::new();
        row_button.add_css_class("picker-row");

        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        let icon = icon_widget(Some(&app));
        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let title = gtk::Label::new(Some(&app.name));
        title.set_xalign(0.0);
        title.add_css_class("picker-row-title");
        let subtitle = gtk::Label::new(Some(&app.id));
        subtitle.set_xalign(0.0);
        subtitle.add_css_class("picker-row-subtitle");
        text.append(&title);
        text.append(&subtitle);
        row.append(&icon);
        row.append(&text);
        row_button.set_child(Some(&row));

        {
            let state = Rc::clone(state);
            let picker_list = picker_list.clone();
            let app = app.clone();
            row_button.connect_clicked(move |_| {
                let mut dock_state = state.borrow_mut();
                if !dock_state.pins.iter().any(|pin| pin == &app.id) {
                    dock_state.pins.push(app.id.clone());
                    config::save_pins(&dock_state.pins);
                }
                drop(dock_state);
                render_picker(&state, &picker_list, "");
            });
        }

        picker_list.append(&row_button);
    }
}

fn collect_items(state: &DockState) -> (Vec<DockItem>, Vec<DockItem>) {
    let mut known = BTreeMap::<String, Vec<WindowState>>::new();
    let mut unknown = BTreeMap::<String, Vec<WindowState>>::new();

    for window in &state.windows {
        if let Some(app) = state.catalog.resolve(window.app_id.as_deref()) {
            known.entry(app.id).or_default().push(window.clone());
        } else {
            let key = window
                .app_id
                .clone()
                .or_else(|| window.title.clone())
                .unwrap_or_else(|| window.id.clone());
            unknown.entry(key).or_default().push(window.clone());
        }
    }

    let pinned = state
        .pins
        .iter()
        .filter_map(|id| {
            let app = state.catalog.app(id)?;
            let windows = known.remove(id).unwrap_or_default();
            let launching = state.is_launching(&app.id);
            Some(build_known_item(app, windows, true, launching))
        })
        .collect::<Vec<_>>();

    let mut running = known
        .into_iter()
        .filter_map(|(id, windows)| {
            let app = state.catalog.app(&id)?;
            let launching = state.is_launching(&app.id);
            Some(build_known_item(app, windows, false, launching))
        })
        .collect::<Vec<_>>();

    running.sort_by_cached_key(|item| (!item.active, item.label.to_lowercase()));

    let mut unknown_items = unknown
        .into_iter()
        .map(|(label, windows)| build_unknown_item(label, windows))
        .collect::<Vec<_>>();
    unknown_items.sort_by_cached_key(|item| (!item.active, item.label.to_lowercase()));

    running.extend(unknown_items);
    (pinned, running)
}

fn build_known_item(
    app: AppRecord,
    windows: Vec<WindowState>,
    pinned: bool,
    launching: bool,
) -> DockItem {
    let active = windows.iter().any(|window| window.active);
    let tooltip = tooltip_for(&app.name, &windows, launching);
    DockItem {
        label: app.name.clone(),
        tooltip,
        app: Some(app),
        windows,
        pinned,
        active,
        launching,
    }
}

fn build_unknown_item(label: String, windows: Vec<WindowState>) -> DockItem {
    let active = windows.iter().any(|window| window.active);
    let tooltip = tooltip_for(&label, &windows, false);
    DockItem {
        label,
        tooltip,
        app: None,
        windows,
        pinned: false,
        active,
        launching: false,
    }
}

fn tooltip_for(label: &str, windows: &[WindowState], launching: bool) -> String {
    match windows.len() {
        0 if launching => format!("{label}\nLaunching..."),
        0 => format!("{label}\nLaunch"),
        1 => {
            let title = windows[0].title.as_deref().unwrap_or("Running");
            format!("{label}\n{title}")
        }
        count => format!("{label}\n{count} windows"),
    }
}

fn icon_widget(app: Option<&AppRecord>) -> gtk::Image {
    let image = if let Some(icon) = app.and_then(|app| app.icon.as_ref()) {
        gtk::Image::from_gicon(icon)
    } else {
        gtk::Image::from_icon_name("grid-view-symbolic")
    };
    image.set_pixel_size(24);
    image
}

fn item_visual(app: Option<&AppRecord>, launching: bool) -> gtk::Overlay {
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&icon_widget(app)));

    if launching {
        let spinner = gtk::Spinner::new();
        spinner.start();
        spinner.set_halign(gtk::Align::End);
        spinner.set_valign(gtk::Align::Start);
        spinner.set_margin_top(4);
        spinner.set_margin_end(4);
        spinner.set_size_request(14, 14);
        spinner.add_css_class("launch-spinner");
        overlay.add_overlay(&spinner);
    }

    overlay
}

fn clear_children(widget: &gtk::Box) {
    while let Some(child) = widget.first_child() {
        widget.remove(&child);
    }
}

fn install_css() {
    if let Some(display) = gdk::Display::default() {
        let provider = gtk::CssProvider::new();
        provider.load_from_data(CSS);
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        config::ensure_style_css();
        if let Some(user_css) = config::load_style_css() {
            let user_provider = gtk::CssProvider::new();
            user_provider.load_from_data(&user_css);
            gtk::style_context_add_provider_for_display(
                &display,
                &user_provider,
                gtk::STYLE_PROVIDER_PRIORITY_USER,
            );
        }
    }
}

fn show_dock(autohide: &Rc<RefCell<AutoHideState>>) {
    let mut state = autohide.borrow_mut();
    if let Some(source) = state.hide_source.take() {
        source.remove();
    }
    state.revealer.set_reveal_child(true);
}

fn schedule_hide(autohide: &Rc<RefCell<AutoHideState>>) {
    let delay = {
        let state = autohide.borrow();
        if !state.enabled {
            state.revealer.set_reveal_child(true);
            return;
        }
        state.delay
    };

    {
        let mut state = autohide.borrow_mut();
        if let Some(source) = state.hide_source.take() {
            source.remove();
        }
    }

    let autohide_for_timeout = Rc::clone(autohide);
    let source = glib::timeout_add_local(delay, move || {
        let mut state = autohide_for_timeout.borrow_mut();
        state.revealer.set_reveal_child(false);
        state.hide_source = None;
        ControlFlow::Break
    });

    autohide.borrow_mut().hide_source = Some(source);
}

const CSS: &str = r#"
.rudo-window {
    background: transparent;
}

.dock-surface {
    padding: 8px 10px;
    border-radius: 22px;
    border: 1px solid rgba(255, 255, 255, 0.14);
    background:
        linear-gradient(180deg, rgba(31, 39, 55, 0.94), rgba(18, 24, 36, 0.92));
    box-shadow:
        0 20px 40px rgba(0, 0, 0, 0.42),
        inset 0 1px 0 rgba(255, 255, 255, 0.08);
}

.dock-item,
.picker-button {
    min-width: 46px;
    min-height: 46px;
    padding: 0;
    border-radius: 16px;
    border: 1px solid transparent;
    background: rgba(255, 255, 255, 0.04);
    box-shadow: none;
}

.dock-item:hover,
.picker-button:hover {
    background: rgba(255, 255, 255, 0.09);
    border-color: rgba(255, 255, 255, 0.12);
}

.dock-item.is-running {
    background: rgba(255, 255, 255, 0.08);
}

.dock-item.is-active {
    background:
        radial-gradient(circle at 50% 0%, rgba(255, 209, 102, 0.22), transparent 56%),
        rgba(255, 255, 255, 0.12);
    border-color: rgba(255, 214, 120, 0.44);
}

.dock-item.is-launching {
    background:
        radial-gradient(circle at 50% 0%, rgba(255, 214, 120, 0.16), transparent 56%),
        rgba(255, 255, 255, 0.09);
    border-color: rgba(255, 214, 120, 0.24);
}

.dock-indicator {
    min-width: 8px;
    min-height: 8px;
    margin-top: 1px;
    border-radius: 999px;
    background: rgba(255, 255, 255, 0.54);
}

.dock-indicator.is-active {
    min-width: 18px;
    background: linear-gradient(90deg, #ffd166, #fca311);
}

.launch-spinner {
    color: #ffd166;
}

.dock-separator {
    min-height: 44px;
    margin: 0 2px;
    opacity: 0.28;
}

.dock-hover-strip {
    min-width: 220px;
    min-height: 8px;
    margin-top: 4px;
    border-radius: 999px;
    background: rgba(255, 255, 255, 0.12);
}

.picker {
    padding: 12px;
}

.picker-row {
    padding: 10px 12px;
    border-radius: 18px;
    border: 1px solid transparent;
    background: rgba(255, 255, 255, 0.04);
}

.picker-row:hover {
    background: rgba(255, 255, 255, 0.08);
    border-color: rgba(255, 255, 255, 0.12);
}

.picker-row-title {
    font-weight: 700;
}

.picker-row-subtitle,
.picker-empty {
    opacity: 0.68;
}

.item-menu {
    padding: 10px;
}

.item-menu button {
    border-radius: 14px;
}
"#;

const LAUNCH_TIMEOUT: Duration = Duration::from_secs(6);
