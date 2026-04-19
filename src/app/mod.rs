use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

use gtk::glib::{self, ControlFlow};
use gtk::prelude::*;
use gtk4 as gtk;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use crate::backend::{self, BackendController};
use crate::catalog::{AppCatalog, AppRecord};
use crate::config;
use crate::model::{BackendEvent, WindowState};

mod autohide;
mod css;
mod dnd;
mod item;
mod picker;

pub fn run() {
    let app = gtk::Application::builder()
        .application_id("dev.rudo.dock")
        .build();

    app.connect_activate(build_ui);
    app.run();
}

#[derive(Clone)]
pub(crate) struct DockItem {
    pub(crate) label: String,
    pub(crate) tooltip: String,
    pub(crate) app: Option<AppRecord>,
    pub(crate) windows: Vec<WindowState>,
    pub(crate) pinned: bool,
    pub(crate) active: bool,
    pub(crate) launching: bool,
    pub(crate) badge_count: Option<u32>,
}

pub(crate) struct DockState {
    pub(crate) catalog: AppCatalog,
    pub(crate) pins: Vec<String>,
    pub(crate) windows: Vec<WindowState>,
    pub(crate) backend: Option<BackendController>,
    pub(crate) launching: HashMap<String, Instant>,
    pub(crate) icon_size: i32,
    last_rendered_items: Vec<(String, bool, u32)>,
    pub(crate) group_by_output: bool,
}

const LAUNCH_TIMEOUT: Duration = Duration::from_secs(6);

impl DockState {
    pub(crate) fn mark_launching(&mut self, app_id: &str) {
        self.launching.insert(app_id.to_string(), Instant::now());
    }

    pub(crate) fn is_launching(&self, app_id: &str) -> bool {
        self.launching.contains_key(app_id)
    }

    fn prune_launching(&mut self) {
        self.launching
            .retain(|_, started_at| started_at.elapsed() < LAUNCH_TIMEOUT);
    }

    pub(crate) fn reconcile_launching(&mut self) {
        let opened_apps = self
            .windows
            .iter()
            .filter_map(|window| self.catalog.resolve(window.app_id.as_deref()))
            .map(|app| app.id)
            .collect::<HashSet<_>>();

        self.launching
            .retain(|app_id, _| !opened_apps.contains(app_id.as_str()));
    }

    fn needs_render(&mut self) -> bool {
        let capacity = self.pins.len() + self.windows.len() + self.launching.len();
        let mut sig = Vec::with_capacity(capacity);
        sig.extend(self.pins.iter().map(|id| (format!("pin:{id}"), false, 0)));
        sig.extend(self.windows.iter().map(|w| {
            (
                format!("{}:{}", w.id, w.app_id.as_deref().unwrap_or("")),
                w.active,
                w.badge_count.unwrap_or(0),
            )
        }));
        sig.extend(
            self.launching
                .keys()
                .map(|id| (format!("launch:{id}"), false, 0)),
        );

        if sig != self.last_rendered_items {
            self.last_rendered_items = sig;
            true
        } else {
            false
        }
    }
}

#[derive(Clone)]
pub(crate) struct RenderContext {
    pub(crate) state: Rc<RefCell<DockState>>,
    pub(crate) items_box: gtk::Box,
    pub(crate) picker_search: gtk::SearchEntry,
    pub(crate) picker_list: gtk::Box,
    pub(crate) autohide: Rc<RefCell<autohide::AutoHideState>>,
}

fn build_ui(app: &gtk::Application) {
    let user_css_provider = css::install();
    config::ensure_settings();
    let settings = config::load_settings();
    let autohide_enabled = settings.autohide.enabled;
    let show_pin_button = settings.show_pin_button;
    let position = settings.position;

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
        last_rendered_items: Vec::new(),
        group_by_output: settings.group_by_output,
    }));

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Rudo")
        .default_width(10)
        .default_height(10)
        .resizable(false)
        .build();
    window.add_css_class("rudo-window");

    let layout = DockLayout::from_position(position);

    if gtk4_layer_shell::is_supported() {
        window.init_layer_shell();
        window.set_namespace(Some("rudo-dock"));
        window.set_layer(Layer::Top);
        window.set_keyboard_mode(KeyboardMode::None);
        window.set_anchor(layout.margin_edge, true);
        window.set_margin(layout.margin_edge, 6);
    } else {
        window.set_decorated(false);
    }

    let outer = gtk::Box::new(layout.orientation, 0);
    outer.set_halign(layout.halign);
    outer.set_valign(layout.valign);

    let dock_revealer = gtk::Revealer::builder()
        .transition_type(layout.transition_type)
        .transition_duration(settings.animation_duration_ms)
        .reveal_child(true)
        .build();

    let dock_surface = gtk::Box::new(layout.items_orientation, 12);
    dock_surface.add_css_class("dock-surface");

    let items_box = gtk::Box::new(layout.items_orientation, 10);
    items_box.set_valign(gtk::Align::Center);
    items_box.set_halign(gtk::Align::Center);

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

    let menu_button = settings
        .menu
        .enabled
        .then(|| item::build_menu_button(&settings.menu, settings.icon_size));

    if let Some(ref menu) = menu_button
        && settings.menu.position == config::MenuPosition::Start
    {
        dock_surface.append(&menu.button);
    }

    dock_surface.append(&items_box);

    if let Some(ref menu) = menu_button
        && settings.menu.position == config::MenuPosition::End
    {
        dock_surface.append(&menu.button);
    }

    dock_surface.append(&picker_button);
    dock_revealer.set_child(Some(&dock_surface));
    outer.append(&dock_revealer);

    let hover_strip = gtk::Box::new(layout.items_orientation, 0);
    hover_strip.add_css_class("dock-hover-strip");
    hover_strip.set_halign(layout.halign);
    hover_strip.set_valign(layout.valign);
    hover_strip.set_hexpand(layout.strip_expand_horizontal);
    hover_strip.set_vexpand(layout.strip_expand_vertical);
    hover_strip.set_visible(autohide_enabled);
    if layout.strip_expand_vertical {
        hover_strip.add_css_class("is-vertical");
    }
    outer.append(&hover_strip);
    window.set_child(Some(&outer));

    let autohide = Rc::new(RefCell::new(autohide::AutoHideState::new(
        &dock_revealer,
        autohide_enabled,
        Duration::from_secs(settings.autohide.delay_secs.max(1)),
    )));

    let ctx = RenderContext {
        state: Rc::clone(&state),
        items_box: items_box.clone(),
        picker_search: picker_search.clone(),
        picker_list: picker_list.clone(),
        autohide: Rc::clone(&autohide),
    };

    render_dock(&ctx);
    autohide::apply_settings(&window, &hover_strip, &autohide, &settings);
    autohide::install_hover(&picker_popover, &autohide);

    if let Some(ref menu) = menu_button {
        autohide::install_hover(&menu.popover, &autohide);
    }

    let config_watch = Rc::new(RefCell::new(ConfigWatchState::new(settings.clone())));

    {
        let ctx = ctx.clone();
        let picker_popover = picker_popover.clone();
        let window_for_open = window.clone();
        picker_button.connect_clicked(move |_| {
            window_for_open.set_keyboard_mode(KeyboardMode::OnDemand);
            ctx.picker_search.set_text("");
            picker::render_picker(&ctx.state, &ctx.picker_list, "");
            picker_popover.popup();
            ctx.picker_search.grab_focus();
            render_dock(&ctx);
        });
    }

    {
        let state = Rc::clone(&state);
        let picker_list = picker_list.clone();
        picker_search.connect_search_changed(move |entry| {
            picker::render_picker(&state, &picker_list, entry.text().as_ref());
        });
    }

    {
        let window_for_close = window.clone();
        picker_popover.connect_closed(move |_| {
            window_for_close.set_keyboard_mode(KeyboardMode::None);
        });
    }

    {
        let ctx = ctx.clone();
        let backend_rx = backend_rx;

        glib::timeout_add_local(Duration::from_millis(80), move || {
            let mut changed = false;
            while let Ok(event) = backend_rx.try_recv() {
                match event {
                    BackendEvent::Snapshot(snapshot) => {
                        let mut dock_state = ctx.state.borrow_mut();
                        dock_state.windows = snapshot;
                        dock_state.reconcile_launching();
                        changed = true;
                    }
                    BackendEvent::BadgeUpdate { id, count } => {
                        let mut dock_state = ctx.state.borrow_mut();
                        if let Some(window) = dock_state.windows.iter_mut().find(|w| w.id == id) {
                            window.badge_count = count;
                            changed = true;
                        }
                    }
                }
            }

            if changed {
                render_dock(&ctx);
            }

            ControlFlow::Continue
        });
    }

    {
        let ctx = ctx.clone();
        let config_watch = Rc::clone(&config_watch);
        let picker_button = picker_button.clone();
        let hover_strip = hover_strip.clone();
        let window = window.clone();
        let user_css_provider = user_css_provider.clone();

        glib::timeout_add_local(Duration::from_millis(700), move || {
            let mut rerender = false;
            let mut settings_to_apply = None;

            {
                let mut watch = config_watch.borrow_mut();

                if watch
                    .pins
                    .check_stable(modified_time(config::pins_path().as_deref()))
                {
                    let mut dock_state = ctx.state.borrow_mut();
                    let pins = sanitize_pins(&dock_state.catalog, config::load_pins());

                    if dock_state.pins != pins {
                        dock_state.pins = pins.clone();
                        config::save_pins(&pins);
                        rerender = true;
                    }
                }

                if watch
                    .settings
                    .check_stable(modified_time(config::settings_path().as_deref()))
                {
                    let new_settings = config::load_settings();
                    if new_settings != watch.current_settings {
                        watch.current_settings = new_settings.clone();
                        settings_to_apply = Some(new_settings);
                    }
                }

                if watch
                    .style
                    .check_stable(modified_time(config::style_path().as_deref()))
                {
                    if let Some(provider) = user_css_provider.as_ref() {
                        provider.load_from_data(&config::load_style_css().unwrap_or_default());
                    }
                    rerender = true;
                }
            }

            if let Some(new_settings) = settings_to_apply {
                picker_button.set_visible(new_settings.show_pin_button);
                autohide::apply_settings(&window, &hover_strip, &ctx.autohide, &new_settings);
                rerender = true;
            }

            if rerender {
                render_dock(&ctx);
            }

            ControlFlow::Continue
        });
    }

    autohide::install_hover(&dock_surface, &autohide);
    autohide::install_hover(&hover_strip, &autohide);

    autohide::schedule_hide(&autohide);

    window.present();
}

fn render_dock(ctx: &RenderContext) {
    if !ctx.state.borrow_mut().needs_render() {
        return;
    }

    clear_children(&ctx.items_box);

    let (pinned_items, running_items) = {
        let mut dock_state = ctx.state.borrow_mut();
        dock_state.prune_launching();
        dock_state.reconcile_launching();
        collect_items(&dock_state)
    };
    let show_separator = !pinned_items.is_empty() && !running_items.is_empty();

    for item in pinned_items {
        ctx.items_box.append(&item::build_item_widget(ctx, item));
    }

    if show_separator {
        let sep_orientation = match ctx.items_box.orientation() {
            gtk::Orientation::Horizontal => gtk::Orientation::Vertical,
            gtk::Orientation::Vertical => gtk::Orientation::Horizontal,
            _ => unreachable!(),
        };
        let separator = gtk::Separator::new(sep_orientation);
        separator.add_css_class("dock-separator");
        if sep_orientation == gtk::Orientation::Horizontal {
            separator.add_css_class("is-vertical");
        }
        ctx.items_box.append(&separator);
    }

    for item in running_items {
        ctx.items_box.append(&item::build_item_widget(ctx, item));
    }

    picker::render_picker(
        &ctx.state,
        &ctx.picker_list,
        ctx.picker_search.text().as_ref(),
    );
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

pub(crate) fn icon_widget(app: Option<&AppRecord>, icon_size: i32) -> gtk::Image {
    let image = if let Some(icon) = app.and_then(|app| app.icon.as_ref()) {
        gtk::Image::from_gicon(icon)
    } else {
        gtk::Image::from_icon_name("grid-view-symbolic")
    };
    image.set_pixel_size(icon_size);
    image
}

fn collect_items(state: &DockState) -> (Vec<DockItem>, Vec<DockItem>) {
    if state.group_by_output {
        collect_items_by_output(state)
    } else {
        collect_items_flat(state)
    }
}

fn group_windows(
    windows: &[WindowState],
    catalog: &AppCatalog,
) -> (
    BTreeMap<String, Vec<WindowState>>,
    BTreeMap<String, Vec<WindowState>>,
) {
    let mut known = BTreeMap::<String, Vec<WindowState>>::new();
    let mut unknown = BTreeMap::<String, Vec<WindowState>>::new();

    for window in windows {
        if let Some(app) = catalog.resolve(window.app_id.as_deref()) {
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

    (known, unknown)
}

fn build_pinned_items(
    state: &DockState,
    known: &mut BTreeMap<String, Vec<WindowState>>,
) -> Vec<DockItem> {
    state
        .pins
        .iter()
        .filter_map(|id| {
            let app = state.catalog.app(id)?;
            let windows = known.remove(id).unwrap_or_default();
            let launching = state.is_launching(&app.id);
            Some(build_known_item(app, windows, true, launching))
        })
        .collect()
}

fn build_running_items(
    known: BTreeMap<String, Vec<WindowState>>,
    state: &DockState,
) -> Vec<DockItem> {
    let mut items = known
        .into_iter()
        .filter_map(|(id, windows)| {
            let app = state.catalog.app(&id)?;
            let launching = state.is_launching(&app.id);
            Some(build_known_item(app, windows, false, launching))
        })
        .collect::<Vec<_>>();
    items.sort_by_cached_key(|item| (!item.active, item.label.to_lowercase()));
    items
}

fn build_unknown_items(unknown: BTreeMap<String, Vec<WindowState>>) -> Vec<DockItem> {
    let mut items = unknown
        .into_iter()
        .map(|(label, windows)| build_unknown_item(label, windows))
        .collect::<Vec<_>>();
    items.sort_by_cached_key(|item| (!item.active, item.label.to_lowercase()));
    items
}

fn collect_items_flat(state: &DockState) -> (Vec<DockItem>, Vec<DockItem>) {
    let (mut known, unknown) = group_windows(&state.windows, &state.catalog);
    let pinned = build_pinned_items(state, &mut known);
    let mut running = build_running_items(known, state);
    running.extend(build_unknown_items(unknown));
    (pinned, running)
}

fn collect_items_by_output(state: &DockState) -> (Vec<DockItem>, Vec<DockItem>) {
    let mut by_output: BTreeMap<Option<u32>, Vec<WindowState>> = BTreeMap::new();
    for window in &state.windows {
        by_output
            .entry(window.output_id)
            .or_default()
            .push(window.clone());
    }

    let mut global_known = BTreeMap::<String, Vec<WindowState>>::new();
    for windows in by_output.values() {
        let (known, _) = group_windows(windows, &state.catalog);
        for (id, wins) in known {
            global_known.entry(id).or_default().extend(wins);
        }
    }

    let pinned = build_pinned_items(state, &mut global_known);
    let mut running = Vec::new();

    for (_output_id, windows) in by_output {
        let (known, unknown) = group_windows(&windows, &state.catalog);
        let mut output_items = build_running_items(known, state);
        output_items.extend(build_unknown_items(unknown));
        running.extend(output_items);
    }

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
    let badge_count = aggregate_badges(&windows);
    DockItem {
        label: app.name.clone(),
        tooltip,
        app: Some(app),
        windows,
        pinned,
        active,
        launching,
        badge_count,
    }
}

fn build_unknown_item(label: String, windows: Vec<WindowState>) -> DockItem {
    let active = windows.iter().any(|window| window.active);
    let tooltip = tooltip_for(&label, &windows, false);
    let badge_count = aggregate_badges(&windows);
    DockItem {
        label,
        tooltip,
        app: None,
        windows,
        pinned: false,
        active,
        launching: false,
        badge_count,
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

fn aggregate_badges(windows: &[WindowState]) -> Option<u32> {
    let total: u32 = windows.iter().filter_map(|w| w.badge_count).sum();
    if total > 0 { Some(total) } else { None }
}

fn sanitize_pins(catalog: &AppCatalog, pins: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    pins.into_iter()
        .filter_map(|id| catalog.app(&id))
        .filter(|app| seen.insert(app.id.clone()))
        .map(|app| app.id)
        .collect()
}

fn modified_time(path: Option<&std::path::Path>) -> Option<SystemTime> {
    path.and_then(|path| std::fs::metadata(path).ok())
        .and_then(|metadata| metadata.modified().ok())
}

struct DockLayout {
    margin_edge: Edge,
    transition_type: gtk::RevealerTransitionType,
    orientation: gtk::Orientation,
    items_orientation: gtk::Orientation,
    halign: gtk::Align,
    valign: gtk::Align,
    strip_expand_horizontal: bool,
    strip_expand_vertical: bool,
}

impl DockLayout {
    fn from_position(position: config::Position) -> Self {
        match position {
            config::Position::Top => Self {
                margin_edge: Edge::Top,
                transition_type: gtk::RevealerTransitionType::SlideDown,
                orientation: gtk::Orientation::Vertical,
                items_orientation: gtk::Orientation::Horizontal,
                halign: gtk::Align::Center,
                valign: gtk::Align::Start,
                strip_expand_horizontal: true,
                strip_expand_vertical: false,
            },
            config::Position::Left => Self {
                margin_edge: Edge::Left,
                transition_type: gtk::RevealerTransitionType::SlideRight,
                orientation: gtk::Orientation::Horizontal,
                items_orientation: gtk::Orientation::Vertical,
                halign: gtk::Align::Start,
                valign: gtk::Align::Center,
                strip_expand_horizontal: false,
                strip_expand_vertical: true,
            },
            config::Position::Right => Self {
                margin_edge: Edge::Right,
                transition_type: gtk::RevealerTransitionType::SlideLeft,
                orientation: gtk::Orientation::Horizontal,
                items_orientation: gtk::Orientation::Vertical,
                halign: gtk::Align::End,
                valign: gtk::Align::Center,
                strip_expand_horizontal: false,
                strip_expand_vertical: true,
            },
            config::Position::Bottom => Self {
                margin_edge: Edge::Bottom,
                transition_type: gtk::RevealerTransitionType::SlideUp,
                orientation: gtk::Orientation::Vertical,
                items_orientation: gtk::Orientation::Horizontal,
                halign: gtk::Align::Center,
                valign: gtk::Align::End,
                strip_expand_horizontal: true,
                strip_expand_vertical: false,
            },
        }
    }
}

struct FileDebouncer {
    mtime: Option<SystemTime>,
    stable_checks: u8,
}

impl FileDebouncer {
    fn new(mtime: Option<SystemTime>) -> Self {
        Self {
            mtime,
            stable_checks: 0,
        }
    }

    fn check_stable(&mut self, current_mtime: Option<SystemTime>) -> bool {
        if current_mtime != self.mtime {
            self.mtime = current_mtime;
            self.stable_checks = 0;
        }
        self.stable_checks = self.stable_checks.saturating_add(1);
        if self.stable_checks >= 2 {
            self.stable_checks = 0;
            return true;
        }
        false
    }
}

struct ConfigWatchState {
    pins: FileDebouncer,
    settings: FileDebouncer,
    style: FileDebouncer,
    current_settings: config::Settings,
}

impl ConfigWatchState {
    fn new(settings: config::Settings) -> Self {
        Self {
            pins: FileDebouncer::new(modified_time(config::pins_path().as_deref())),
            settings: FileDebouncer::new(modified_time(config::settings_path().as_deref())),
            style: FileDebouncer::new(modified_time(config::style_path().as_deref())),
            current_settings: settings,
        }
    }
}
