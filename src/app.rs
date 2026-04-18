use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

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
    icon_size: i32,
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

struct ConfigWatchState {
    pins_mtime: Option<SystemTime>,
    settings_mtime: Option<SystemTime>,
    style_mtime: Option<SystemTime>,
    settings: config::Settings,
}

impl ConfigWatchState {
    fn new(settings: config::Settings) -> Self {
        Self {
            pins_mtime: modified_time(config::pins_path().as_deref()),
            settings_mtime: modified_time(config::settings_path().as_deref()),
            style_mtime: modified_time(config::style_path().as_deref()),
            settings,
        }
    }
}

fn build_ui(app: &gtk::Application) {
    let user_css_provider = install_css();
    config::ensure_settings();
    let settings = config::load_settings();
    let autohide_enabled = settings.autohide.enabled;
    let show_pin_button = settings.show_pin_button;
    let position = settings.position.clone();

    let catalog = AppCatalog::load();
    let pins = sanitize_pins(&catalog, config::load_pins());
    config::save_pins(&pins);

    let (backend_tx, backend_rx) = mpsc::channel();
    let backend = backend::spawn(backend_tx);

    let state = Rc::new(RefCell::new(DockState {
        catalog,
        pins,
        windows: Vec::new(),
        backend,
        launching: HashMap::new(),
        icon_size: settings.icon_size,
    }));

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Rudo")
        .default_width(10)
        .default_height(10)
        .resizable(false)
        .build();
    window.add_css_class("rudo-window");

    // Parse position and set up dock orientation
    let (anchor_edge, transition_type, orientation, halign, valign, margin_edge) =
        match position.as_str() {
            "top" => (
                Edge::Top,
                gtk::RevealerTransitionType::SlideDown,
                gtk::Orientation::Vertical,
                gtk::Align::Center,
                gtk::Align::Start,
                Edge::Top,
            ),
            "left" => (
                Edge::Left,
                gtk::RevealerTransitionType::SlideRight,
                gtk::Orientation::Horizontal,
                gtk::Align::Start,
                gtk::Align::Center,
                Edge::Left,
            ),
            "right" => (
                Edge::Right,
                gtk::RevealerTransitionType::SlideLeft,
                gtk::Orientation::Horizontal,
                gtk::Align::End,
                gtk::Align::Center,
                Edge::Right,
            ),
            _ => (
                Edge::Bottom,
                gtk::RevealerTransitionType::SlideUp,
                gtk::Orientation::Vertical,
                gtk::Align::Center,
                gtk::Align::End,
                Edge::Bottom,
            ),
        };

    if gtk4_layer_shell::is_supported() {
        window.init_layer_shell();
        window.set_namespace(Some("rudo-dock"));
        window.set_layer(Layer::Top);
        window.set_keyboard_mode(KeyboardMode::None);
        window.set_anchor(anchor_edge, true);
        window.set_margin(margin_edge, 6);
    } else {
        window.set_decorated(false);
    }

    let outer = gtk::Box::new(orientation, 0);
    outer.set_halign(halign);
    outer.set_valign(valign);

    let dock_revealer = gtk::Revealer::builder()
        .transition_type(transition_type)
        .transition_duration(settings.animation_duration_ms)
        .reveal_child(true)
        .build();

    let dock_surface = gtk::Box::new(
        if orientation == gtk::Orientation::Vertical {
            gtk::Orientation::Horizontal
        } else {
            gtk::Orientation::Vertical
        },
        12,
    );
    dock_surface.add_css_class("dock-surface");

    let items_box = gtk::Box::new(
        if orientation == gtk::Orientation::Vertical {
            gtk::Orientation::Horizontal
        } else {
            gtk::Orientation::Vertical
        },
        10,
    );
    items_box.set_valign(gtk::Align::Center);

    let picker_button = gtk::Button::new();
    picker_button.add_css_class("dock-item");
    picker_button.add_css_class("picker-button");
    picker_button.set_tooltip_text(Some("Pin an application"));
    picker_button.set_child(Some(&icon_widget(None, settings.icon_size)));
    picker_button.set_visible(show_pin_button);

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

    let hover_strip = gtk::Box::new(
        if orientation == gtk::Orientation::Vertical {
            gtk::Orientation::Horizontal
        } else {
            gtk::Orientation::Vertical
        },
        0,
    );
    hover_strip.add_css_class("dock-hover-strip");
    hover_strip.set_halign(halign);
    hover_strip.set_valign(valign);
    hover_strip.set_hexpand(true);
    hover_strip.set_visible(autohide_enabled);
    outer.append(&hover_strip);
    window.set_child(Some(&outer));

    render_dock(
        &state,
        &items_box,
        &picker_search,
        &picker_list,
        &Rc::new(RefCell::new(AutoHideState::new(
            &dock_revealer,
            autohide_enabled,
            Duration::from_secs(settings.autohide.delay_secs.max(1)),
        ))),
    );

    let autohide = Rc::new(RefCell::new(AutoHideState::new(
        &dock_revealer,
        autohide_enabled,
        Duration::from_secs(settings.autohide.delay_secs.max(1)),
    )));
    apply_autohide_settings(&window, &hover_strip, &autohide, &settings);
    install_autohide_hover(&picker_popover, &autohide);
    let config_watch = Rc::new(RefCell::new(ConfigWatchState::new(settings.clone())));

    {
        let state = Rc::clone(&state);
        let autohide = Rc::clone(&autohide);
        let items_box = items_box.clone();
        let picker_search = picker_search.clone();
        let picker_list = picker_list.clone();
        let picker_popover = picker_popover.clone();
        picker_button.connect_clicked(move |_| {
            picker_search.set_text("");
            render_picker(&state, &picker_list, "");
            picker_popover.popup();
            picker_search.grab_focus();
            render_dock(&state, &items_box, &picker_search, &picker_list, &autohide);
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
        let autohide_for_rx = Rc::clone(&autohide);
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
                render_dock(
                    &state,
                    &items_box,
                    &picker_search,
                    &picker_list,
                    &autohide_for_rx,
                );
            }

            ControlFlow::Continue
        });
    }

    {
        let state = Rc::clone(&state);
        let autohide = Rc::clone(&autohide);
        let config_watch = Rc::clone(&config_watch);
        let items_box = items_box.clone();
        let picker_button = picker_button.clone();
        let picker_search = picker_search.clone();
        let picker_list = picker_list.clone();
        let hover_strip = hover_strip.clone();
        let window = window.clone();
        let user_css_provider = user_css_provider.clone();

        glib::timeout_add_local(Duration::from_millis(700), move || {
            let mut rerender = false;
            let mut settings_to_apply = None;

            {
                let mut watch = config_watch.borrow_mut();

                let pins_mtime = modified_time(config::pins_path().as_deref());
                if pins_mtime != watch.pins_mtime {
                    watch.pins_mtime = pins_mtime;
                    let mut dock_state = state.borrow_mut();
                    let pins = sanitize_pins(&dock_state.catalog, config::load_pins());
                    if dock_state.pins != pins {
                        dock_state.pins = pins.clone();
                        config::save_pins(&pins);
                        rerender = true;
                    }
                }

                let settings_mtime = modified_time(config::settings_path().as_deref());
                if settings_mtime != watch.settings_mtime {
                    watch.settings_mtime = settings_mtime;
                    let new_settings = config::load_settings();
                    if new_settings != watch.settings {
                        watch.settings = new_settings.clone();
                        settings_to_apply = Some(new_settings);
                    }
                }

                let style_mtime = modified_time(config::style_path().as_deref());
                if style_mtime != watch.style_mtime {
                    watch.style_mtime = style_mtime;
                    if let Some(provider) = user_css_provider.as_ref() {
                        provider.load_from_data(&config::load_style_css().unwrap_or_default());
                    }
                    rerender = true;
                }
            }

            if let Some(new_settings) = settings_to_apply {
                picker_button.set_visible(new_settings.show_pin_button);
                apply_autohide_settings(&window, &hover_strip, &autohide, &new_settings);
                rerender = true;
            }

            if rerender {
                render_dock(&state, &items_box, &picker_search, &picker_list, &autohide);
            }

            ControlFlow::Continue
        });
    }

    install_autohide_hover(&dock_surface, &autohide);
    install_autohide_hover(&hover_strip, &autohide);

    schedule_hide(&autohide);

    window.present();
}

fn render_dock(
    state: &Rc<RefCell<DockState>>,
    items_box: &gtk::Box,
    picker_search: &gtk::SearchEntry,
    picker_list: &gtk::Box,
    autohide: &Rc<RefCell<AutoHideState>>,
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
            autohide,
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
            autohide,
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
    autohide: &Rc<RefCell<AutoHideState>>,
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
    let icon_size = state.borrow().icon_size;
    button.set_child(Some(&item_visual(
        item.app.as_ref(),
        item.launching,
        icon_size,
    )));

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
        let autohide = Rc::clone(autohide);
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
                        render_dock(&state, &items_box, &picker_search, &picker_list, &autohide);
                    }
                    Err(error) => eprintln!("failed to launch {}: {error}", app.id),
                }
            }
        });
    }

    {
        let state = Rc::clone(state);
        let autohide_for_closure = Rc::clone(autohide);
        let item = item.clone();
        let items_box = items_box.clone();
        let picker_search = picker_search.clone();
        let picker_list = picker_list.clone();
        let popover = build_context_menu(Rc::clone(&state), &button, item, autohide, move || {
            render_dock(
                &state,
                &items_box,
                &picker_search,
                &picker_list,
                &autohide_for_closure,
            )
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

    if item.pinned
        && let Some(app) = item.app.as_ref()
    {
        install_pin_drag_and_drop(
            &wrapper,
            state,
            items_box,
            picker_search,
            picker_list,
            &app.id,
            autohide,
        );
    }

    wrapper
}

fn build_context_menu(
    state: Rc<RefCell<DockState>>,
    parent: &impl gtk::prelude::IsA<gtk::Widget>,
    item: DockItem,
    autohide: &Rc<RefCell<AutoHideState>>,
    rerender: impl Fn() + 'static,
) -> gtk::Popover {
    let popover = gtk::Popover::new();
    popover.set_has_arrow(false);
    popover.set_position(gtk::PositionType::Top);
    popover.set_parent(parent);

    install_autohide_hover(&popover, autohide);

    let layout = gtk::Box::new(gtk::Orientation::Vertical, 6);
    layout.add_css_class("item-menu");

    let title = gtk::Label::new(Some(&item.label));
    title.set_xalign(0.0);
    title.add_css_class("item-menu-title");
    layout.append(&title);

    if let Some(app) = item.app.as_ref() {
        let subtitle = gtk::Label::new(Some(&app.id));
        subtitle.set_xalign(0.0);
        subtitle.add_css_class("item-menu-subtitle");
        layout.append(&subtitle);
    }

    if !item.windows.is_empty() {
        layout.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

        let windows_label = gtk::Label::new(Some("Windows"));
        windows_label.set_xalign(0.0);
        windows_label.add_css_class("item-menu-section");
        layout.append(&windows_label);

        let multiple = item.windows.len() > 1;
        for window in item.windows.iter().take(8) {
            let focus_window = gtk::Button::with_label(&window_menu_label(window, multiple));
            if window.active {
                focus_window.add_css_class("is-active");
            }
            {
                let state = Rc::clone(&state);
                let window = window.clone();
                let popover = popover.clone();
                focus_window.connect_clicked(move |_| {
                    if let Some(backend) = state.borrow().backend.as_ref() {
                        backend.activate(&window.id);
                    }
                    popover.popdown();
                });
            }
            layout.append(&focus_window);
        }
    }

    if let Some(app) = item.app.clone() {
        layout.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

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
        if let Some(active_window) = item.windows.iter().find(|window| window.active).cloned() {
            let close_active = gtk::Button::with_label("Close Focused Window");
            {
                let state = Rc::clone(&state);
                let popover = popover.clone();
                close_active.connect_clicked(move |_| {
                    if let Some(backend) = state.borrow().backend.as_ref() {
                        backend.close(&active_window.id);
                    }
                    popover.popdown();
                });
            }
            layout.append(&close_active);
        }

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
        let icon_size = state.borrow().icon_size;
        let icon = icon_widget(Some(&app), icon_size);
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

fn icon_widget(app: Option<&AppRecord>, icon_size: i32) -> gtk::Image {
    let image = if let Some(icon) = app.and_then(|app| app.icon.as_ref()) {
        gtk::Image::from_gicon(icon)
    } else {
        gtk::Image::from_icon_name("grid-view-symbolic")
    };
    image.set_pixel_size(icon_size);
    image
}

fn item_visual(app: Option<&AppRecord>, launching: bool, icon_size: i32) -> gtk::Overlay {
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&icon_widget(app, icon_size)));

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
        cleanup_widget_tree(&child);
        widget.remove(&child);
    }
}

fn cleanup_widget_tree(widget: &gtk::Widget) {
    let mut current = widget.first_child();
    while let Some(child) = current {
        let next = child.next_sibling();
        cleanup_widget_tree(&child);
        current = next;
    }

    if let Some(popover) = widget.downcast_ref::<gtk::Popover>() {
        popover.popdown();
        popover.set_child(None::<&gtk::Widget>);
        popover.unparent();
    }
}

fn install_css() -> Option<gtk::CssProvider> {
    if let Some(display) = gdk::Display::default() {
        let provider = gtk::CssProvider::new();
        provider.load_from_data(CSS);
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        config::ensure_style_css();
        let user_provider = gtk::CssProvider::new();
        user_provider.load_from_data(&config::load_style_css().unwrap_or_default());
        gtk::style_context_add_provider_for_display(
            &display,
            &user_provider,
            gtk::STYLE_PROVIDER_PRIORITY_USER,
        );
        return Some(user_provider);
    }

    None
}

fn show_dock(autohide: &Rc<RefCell<AutoHideState>>) {
    let mut state = autohide.borrow_mut();
    if let Some(source) = state.hide_source.take() {
        source.remove();
    }
    state.revealer.set_reveal_child(true);
}

fn install_autohide_hover(
    widget: &impl gtk::prelude::IsA<gtk::Widget>,
    autohide: &Rc<RefCell<AutoHideState>>,
) {
    let enter_state = Rc::clone(autohide);
    let leave_state = Rc::clone(autohide);
    let motion = gtk::EventControllerMotion::new();
    motion.connect_enter(move |_, _, _| show_dock(&enter_state));
    motion.connect_leave(move |_| schedule_hide(&leave_state));
    widget.add_controller(motion);
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

fn apply_autohide_settings(
    window: &gtk::ApplicationWindow,
    hover_strip: &gtk::Box,
    autohide: &Rc<RefCell<AutoHideState>>,
    settings: &config::Settings,
) {
    let enabled = settings.autohide.enabled;
    let delay = Duration::from_secs(settings.autohide.delay_secs.max(1));

    if gtk4_layer_shell::is_supported() {
        if enabled {
            // Hover-revealed dock floats above windows instead of relayouting them.
            window.set_exclusive_zone(0);
        } else {
            window.auto_exclusive_zone_enable();
        }
    }

    hover_strip.set_visible(enabled);

    {
        let mut state = autohide.borrow_mut();
        if let Some(source) = state.hide_source.take() {
            source.remove();
        }
        state.enabled = enabled;
        state.delay = delay;
    }

    if enabled {
        show_dock(autohide);
        schedule_hide(autohide);
    } else {
        show_dock(autohide);
    }
}

fn install_pin_drag_and_drop(
    wrapper: &gtk::Box,
    state: &Rc<RefCell<DockState>>,
    items_box: &gtk::Box,
    picker_search: &gtk::SearchEntry,
    picker_list: &gtk::Box,
    pin_id: &str,
    autohide: &Rc<RefCell<AutoHideState>>,
) {
    let drag_source = gtk::DragSource::new();
    drag_source.set_actions(gdk::DragAction::MOVE);
    let source_pin = pin_id.to_string();
    drag_source.connect_prepare(move |_, _, _| {
        Some(gdk::ContentProvider::for_value(&source_pin.to_value()))
    });
    wrapper.add_controller(drag_source);

    let drop_target = gtk::DropTarget::new(String::static_type(), gdk::DragAction::MOVE);

    {
        let wrapper = wrapper.clone();
        drop_target.connect_enter(move |_, _, _| {
            wrapper.add_css_class("is-drop-target");
            gdk::DragAction::MOVE
        });
    }

    {
        let wrapper = wrapper.clone();
        drop_target.connect_leave(move |_| {
            wrapper.remove_css_class("is-drop-target");
        });
    }

    {
        let state = Rc::clone(state);
        let autohide = Rc::clone(autohide);
        let items_box = items_box.clone();
        let picker_search = picker_search.clone();
        let picker_list = picker_list.clone();
        let target_pin = pin_id.to_string();
        let wrapper = wrapper.clone();

        drop_target.connect_drop(move |_, value, x, _| {
            wrapper.remove_css_class("is-drop-target");

            let Ok(dragged_pin) = value.get::<String>() else {
                return false;
            };

            let insert_after = x > f64::from(wrapper.allocated_width()) / 2.0;
            let changed = {
                let mut dock_state = state.borrow_mut();
                reorder_pins(
                    &mut dock_state.pins,
                    &dragged_pin,
                    &target_pin,
                    insert_after,
                )
            };

            if changed {
                config::save_pins(&state.borrow().pins);
                render_dock(&state, &items_box, &picker_search, &picker_list, &autohide);
            }

            changed
        });
    }

    wrapper.add_controller(drop_target);
}

fn reorder_pins(
    pins: &mut Vec<String>,
    dragged_pin: &str,
    target_pin: &str,
    insert_after: bool,
) -> bool {
    if dragged_pin == target_pin {
        return false;
    }

    let Some(source_idx) = pins.iter().position(|pin| pin == dragged_pin) else {
        return false;
    };
    let Some(mut target_idx) = pins.iter().position(|pin| pin == target_pin) else {
        return false;
    };

    let dragged = pins.remove(source_idx);
    if source_idx < target_idx {
        target_idx -= 1;
    }
    if insert_after {
        target_idx += 1;
    }

    pins.insert(target_idx.min(pins.len()), dragged);
    true
}

fn sanitize_pins(catalog: &AppCatalog, pins: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut sanitized = Vec::new();

    for pin in pins {
        let Some(app) = catalog.app(&pin) else {
            continue;
        };

        if seen.insert(app.id.clone()) {
            sanitized.push(app.id);
        }
    }

    sanitized
}

fn window_menu_label(window: &WindowState, multiple: bool) -> String {
    let title = window.title.as_deref().unwrap_or("Untitled Window");

    if multiple {
        if window.active {
            format!("Focus {title} (active)")
        } else {
            format!("Focus {title}")
        }
    } else if window.active {
        format!("Focus {title} (active)")
    } else {
        "Focus Window".to_string()
    }
}

fn modified_time(path: Option<&std::path::Path>) -> Option<SystemTime> {
    path.and_then(|path| std::fs::metadata(path).ok())
        .and_then(|metadata| metadata.modified().ok())
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

.dock-item.is-drop-target {
    border-color: rgba(255, 214, 120, 0.48);
    background: rgba(255, 214, 120, 0.12);
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

.item-menu-title {
    font-weight: 700;
}

.item-menu-subtitle,
.item-menu-section {
    opacity: 0.68;
}

.item-menu button {
    border-radius: 14px;
}
"#;

const LAUNCH_TIMEOUT: Duration = Duration::from_secs(6);
