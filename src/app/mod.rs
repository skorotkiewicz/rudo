use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::time::{Duration, Instant};

use gtk::gdk;
use gtk::gio;
use gtk::glib::{self, ControlFlow};
use gtk::prelude::*;
use gtk4 as gtk;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};

use crate::backend::{self, BackendController, EventMailbox};
use crate::catalog::{AppCatalog, AppRecord};
use crate::config;
use crate::model::WindowState;

mod autohide;
mod css;
mod dnd;
mod item;
mod picker;

pub fn run() -> glib::ExitCode {
    let args = std::env::args().collect::<Vec<_>>();
    if args.iter().any(|arg| arg == "-V" || arg == "--version") {
        println!("rudo {}", env!("CARGO_PKG_VERSION"));
        return glib::ExitCode::SUCCESS;
    }
    if let Err(error) = validate_visibility_args(&args) {
        eprintln!("rudo: {error}");
        return glib::ExitCode::FAILURE;
    }

    let app = gtk::Application::builder()
        .application_id("dev.rudo.dock")
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();

    register_command_line_options(&app);

    let runtime = Rc::new(RefCell::new(None::<Rc<AppRuntime>>));

    {
        let runtime = Rc::clone(&runtime);
        app.connect_activate(move |app| {
            ensure_runtime(app, &runtime).show();
        });
    }

    {
        let runtime = Rc::clone(&runtime);
        app.connect_command_line(move |app, command_line| {
            let options = command_line.options_dict();
            let command = match command_from_options(&options) {
                Ok(command) => command,
                Err(error) => {
                    eprintln!("rudo: {error}");
                    return glib::ExitCode::FAILURE;
                }
            };

            match command {
                AppCommand::Show => ensure_runtime(app, &runtime).show(),
                AppCommand::Toggle => {
                    if let Some(existing) = runtime.borrow().as_ref().cloned() {
                        existing.toggle();
                    } else {
                        ensure_runtime(app, &runtime).show();
                    }
                }
                AppCommand::Hide => {
                    if let Some(existing) = runtime.borrow().as_ref() {
                        existing.hide();
                    }
                }
            }

            glib::ExitCode::SUCCESS
        });
    }

    app.run()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AppCommand {
    Show,
    Toggle,
    Hide,
}

fn register_command_line_options(app: &gtk::Application) {
    for (name, short, description) in [
        ("toggle", b't', "Toggle dock visibility"),
        ("show", b's', "Show the dock"),
        ("hide", b'H', "Hide the dock"),
        ("version", b'V', "Print version information"),
    ] {
        app.add_main_option(
            name,
            short.into(),
            glib::OptionFlags::NONE,
            glib::OptionArg::None,
            description,
            None,
        );
    }
}

fn command_from_options(options: &glib::VariantDict) -> Result<AppCommand, String> {
    let selected = [
        ("toggle", AppCommand::Toggle),
        ("show", AppCommand::Show),
        ("hide", AppCommand::Hide),
    ]
    .into_iter()
    .filter_map(|(name, command)| option_enabled(options, name).then_some(command))
    .collect::<Vec<_>>();

    match selected.as_slice() {
        [] => Ok(AppCommand::Show),
        [command] => Ok(*command),
        _ => Err("visibility options are mutually exclusive".to_string()),
    }
}

fn validate_visibility_args(args: &[String]) -> Result<(), String> {
    let selected = args
        .iter()
        .filter(|arg| {
            matches!(
                arg.as_str(),
                "-t" | "--toggle" | "-s" | "--show" | "-H" | "--hide"
            )
        })
        .count();
    if selected > 1 {
        Err("visibility options are mutually exclusive".to_string())
    } else {
        Ok(())
    }
}

fn option_enabled(options: &glib::VariantDict, name: &str) -> bool {
    options.lookup::<bool>(name).ok().flatten().unwrap_or(false)
}

fn ensure_runtime(
    app: &gtk::Application,
    slot: &Rc<RefCell<Option<Rc<AppRuntime>>>>,
) -> Rc<AppRuntime> {
    if let Some(runtime) = slot.borrow().as_ref().cloned() {
        return runtime;
    }

    let runtime = AppRuntime::new(app);
    *slot.borrow_mut() = Some(Rc::clone(&runtime));
    runtime
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
        let opened_apps: HashSet<String> = self
            .windows
            .iter()
            .filter_map(|window| {
                window
                    .app_id
                    .as_deref()
                    .and_then(|id| self.catalog.resolve_id(id))
            })
            .collect();

        self.launching
            .retain(|app_id, _| !opened_apps.contains(app_id.as_str()));
    }
}

#[derive(Clone)]
pub(crate) struct RenderContext {
    pub(crate) state: Rc<RefCell<DockState>>,
    pub(crate) items_box: gtk::Box,
    pub(crate) picker_search: gtk::SearchEntry,
    pub(crate) picker_list: gtk::Box,
    pub(crate) picker_popover: gtk::Popover,
    pub(crate) autohide: Rc<RefCell<autohide::AutoHideState>>,
    contexts: Weak<RefCell<Vec<RenderContext>>>,
}

struct DockView {
    window: gtk::ApplicationWindow,
    ctx: RenderContext,
}

struct AppRuntime {
    app: gtk::Application,
    state: Rc<RefCell<DockState>>,
    settings: RefCell<config::Settings>,
    views: RefCell<Vec<DockView>>,
    contexts: Rc<RefCell<Vec<RenderContext>>>,
    visible: Cell<bool>,
    backend_events: EventMailbox,
    config_changes: ConfigChanges,
    user_css_provider: Option<gtk::CssProvider>,
    monitor_model: Option<gio::ListModel>,
    _config_watch: Option<ConfigWatchState>,
}

impl AppRuntime {
    fn new(app: &gtk::Application) -> Rc<Self> {
        if let Err(error) = config::ensure_settings() {
            eprintln!("failed to prepare dock settings: {error}");
        }
        if let Err(error) = config::ensure_style_css() {
            eprintln!("failed to prepare dock stylesheet: {error}");
        }

        let settings = config::load_settings().unwrap_or_else(|error| {
            eprintln!(
                "failed to load settings; using defaults without overwriting the file: {error}"
            );
            config::Settings::default()
        });
        let user_css_provider = css::install();

        let catalog = AppCatalog::load();
        let pins = match config::load_pins() {
            Ok(pins) => sanitize_pins(&catalog, pins),
            Err(error) => {
                eprintln!(
                    "failed to load pins; preserving the file and starting with no pins: {error}"
                );
                Vec::new()
            }
        };

        let backend_events = EventMailbox::default();
        let backend = backend::spawn(backend_events.clone());
        let state = Rc::new(RefCell::new(DockState {
            catalog,
            pins,
            windows: Vec::new(),
            backend,
            launching: HashMap::new(),
            icon_size: settings.icon_size,
        }));

        let config_changes = ConfigChanges::default();
        let config_watch = ConfigWatchState::new(config_changes.clone())
            .map_err(|error| eprintln!("live config reload is unavailable: {error}"))
            .ok();
        let monitor_model = gdk::Display::default().map(|display| display.monitors());

        let runtime = Rc::new(Self {
            app: app.clone(),
            state,
            settings: RefCell::new(settings),
            views: RefCell::new(Vec::new()),
            contexts: Rc::new(RefCell::new(Vec::new())),
            visible: Cell::new(true),
            backend_events,
            config_changes,
            user_css_provider,
            monitor_model,
            _config_watch: config_watch,
        });

        runtime.install_sources();
        runtime.rebuild_views();
        runtime
    }

    fn install_sources(self: &Rc<Self>) {
        {
            let runtime = Rc::downgrade(self);
            glib::timeout_add_local(Duration::from_millis(80), move || {
                let Some(runtime) = runtime.upgrade() else {
                    return ControlFlow::Break;
                };
                runtime.process_backend_snapshot();
                ControlFlow::Continue
            });
        }

        {
            let runtime = Rc::downgrade(self);
            glib::timeout_add_local(Duration::from_millis(200), move || {
                let Some(runtime) = runtime.upgrade() else {
                    return ControlFlow::Break;
                };
                runtime.process_config_changes();
                ControlFlow::Continue
            });
        }

        if let Some(monitors) = self.monitor_model.as_ref() {
            let runtime = Rc::downgrade(self);
            monitors.connect_items_changed(move |_, _, _, _| {
                if let Some(runtime) = runtime.upgrade() {
                    runtime.rebuild_views();
                }
            });
        }
    }

    fn process_backend_snapshot(&self) {
        let Some(snapshot) = self.backend_events.take_latest() else {
            return;
        };

        let changed = {
            let mut state = self.state.borrow_mut();
            if state.windows == snapshot {
                false
            } else {
                state.windows = snapshot;
                state.reconcile_launching();
                true
            }
        };

        if changed {
            render_contexts(&self.contexts);
        }
    }

    fn process_config_changes(&self) {
        let changes = self.config_changes.take();
        if changes == 0 {
            return;
        }

        let mut rerender = false;
        let mut rebuild_views = false;

        if changes & ConfigChanges::PINS != 0 {
            match config::load_pins() {
                Ok(pins) => {
                    let mut state = self.state.borrow_mut();
                    let pins = sanitize_pins(&state.catalog, pins);
                    if state.pins != pins {
                        state.pins = pins;
                        rerender = true;
                    }
                }
                Err(error) => {
                    eprintln!("ignored invalid pins update and kept the last valid state: {error}");
                }
            }
        }

        if changes & ConfigChanges::SETTINGS != 0 {
            match config::load_settings() {
                Ok(settings) if settings != *self.settings.borrow() => {
                    self.state.borrow_mut().icon_size = settings.icon_size;
                    *self.settings.borrow_mut() = settings;
                    rebuild_views = true;
                }
                Ok(_) => {}
                Err(error) => {
                    eprintln!(
                        "ignored invalid settings update and kept the last valid state: {error}"
                    );
                }
            }
        }

        if changes & ConfigChanges::STYLE != 0 {
            match config::load_style_css() {
                Ok(css) => {
                    if let Some(provider) = self.user_css_provider.as_ref() {
                        provider.load_from_data(css.as_deref().unwrap_or_default());
                    }
                }
                Err(error) => eprintln!("ignored stylesheet update: {error}"),
            }
        }

        if rebuild_views {
            self.rebuild_views();
        } else if rerender {
            render_contexts(&self.contexts);
        }
    }

    fn monitor_targets(&self) -> Vec<Option<gdk::Monitor>> {
        let mut monitors = self
            .monitor_model
            .as_ref()
            .map(|model| {
                (0..model.n_items())
                    .filter_map(|index| model.item(index))
                    .filter_map(|item| item.downcast::<gdk::Monitor>().ok())
                    .map(Some)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        if self.settings.borrow().outputs == config::OutputMode::First
            || !gtk4_layer_shell::is_supported()
        {
            monitors.truncate(1);
        }
        if monitors.is_empty() {
            monitors.push(None);
        }
        monitors
    }

    fn rebuild_views(&self) {
        let settings = self.settings.borrow().clone();
        let contexts = Rc::downgrade(&self.contexts);
        let new_views = self
            .monitor_targets()
            .into_iter()
            .map(|monitor| {
                build_view(
                    &self.app,
                    Rc::clone(&self.state),
                    &settings,
                    contexts.clone(),
                    monitor.as_ref(),
                )
            })
            .collect::<Vec<_>>();

        *self.contexts.borrow_mut() = new_views.iter().map(|view| view.ctx.clone()).collect();
        render_contexts(&self.contexts);

        if self.visible.get() {
            for view in &new_views {
                view.window.present();
            }
        }

        let old_views = self.views.replace(new_views);
        for view in old_views {
            autohide::cancel_pending_hide(&view.ctx.autohide);
            clear_children(&view.ctx.items_box);
            view.window.close();
        }
    }

    fn show(&self) {
        self.visible.set(true);
        for view in self.views.borrow().iter() {
            autohide::show_dock(&view.ctx.autohide);
            autohide::schedule_hide(&view.ctx.autohide);
            view.window.present();
        }
    }

    fn hide(&self) {
        self.visible.set(false);
        for view in self.views.borrow().iter() {
            view.window.set_visible(false);
        }
    }

    fn toggle(&self) {
        if self.visible.get() {
            self.hide();
        } else {
            self.show();
        }
    }
}

fn build_view(
    app: &gtk::Application,
    state: Rc<RefCell<DockState>>,
    settings: &config::Settings,
    contexts: Weak<RefCell<Vec<RenderContext>>>,
    monitor: Option<&gdk::Monitor>,
) -> DockView {
    let autohide_enabled = settings.autohide.enabled;
    let show_pin_button = settings.show_pin_button;
    let position = settings.position;

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
        window.set_monitor(monitor);
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
        picker_popover: picker_popover.clone(),
        autohide: Rc::clone(&autohide),
        contexts,
    };

    autohide::apply_settings(&window, &hover_strip, &autohide, settings);
    autohide::install_hover(&picker_popover, &autohide);

    if let Some(ref menu) = menu_button {
        autohide::install_hover(&menu.popover, &autohide);
    }

    {
        let state = Rc::clone(&state);
        let picker_list = picker_list.clone();
        let picker_search = picker_search.clone();
        let picker_popover = picker_popover.clone();
        let window_for_open = window.downgrade();
        let contexts = ctx.contexts.clone();
        picker_button.connect_clicked(move |_| {
            if let Some(window) = window_for_open.upgrade() {
                window.set_keyboard_mode(KeyboardMode::OnDemand);
            }
            picker_search.set_text("");
            picker::render_picker(&state, &picker_list, &picker_search, &contexts, "");
            picker_popover.popup();
            picker_search.grab_focus();
        });
    }

    {
        let state = Rc::clone(&state);
        let picker_list = picker_list.clone();
        let contexts = ctx.contexts.clone();
        picker_search.connect_search_changed(move |entry| {
            picker::render_picker(
                &state,
                &picker_list,
                entry,
                &contexts,
                entry.text().as_ref(),
            );
        });
    }

    {
        let window_for_close = window.downgrade();
        picker_popover.connect_closed(move |_| {
            if let Some(window) = window_for_close.upgrade() {
                window.set_keyboard_mode(KeyboardMode::None);
            }
        });
    }

    autohide::install_hover(&dock_surface, &autohide);
    autohide::install_hover(&hover_strip, &autohide);

    autohide::schedule_hide(&autohide);

    DockView { window, ctx }
}

pub(crate) fn render_all(ctx: &RenderContext) {
    if let Some(contexts) = ctx.contexts.upgrade() {
        render_contexts(&contexts);
    } else {
        render_dock(ctx);
    }
}

pub(crate) fn render_registered(contexts: &Weak<RefCell<Vec<RenderContext>>>) {
    if let Some(contexts) = contexts.upgrade() {
        render_contexts(&contexts);
    }
}

fn render_contexts(contexts: &Rc<RefCell<Vec<RenderContext>>>) {
    let contexts = contexts.borrow().clone();
    if let Some(ctx) = contexts.first() {
        let mut state = ctx.state.borrow_mut();
        state.prune_launching();
        state.reconcile_launching();
    }
    for ctx in &contexts {
        render_dock(ctx);
    }
}

pub(crate) fn schedule_launch_expiry(ctx: &RenderContext, app_id: &str) {
    let state = Rc::clone(&ctx.state);
    let contexts = ctx.contexts.clone();
    let app_id = app_id.to_string();
    glib::timeout_add_local_once(LAUNCH_TIMEOUT, move || {
        if state.borrow_mut().launching.remove(&app_id).is_some()
            && let Some(contexts) = contexts.upgrade()
        {
            render_contexts(&contexts);
        }
    });
}

fn render_dock(ctx: &RenderContext) {
    let (pinned_items, running_items) = {
        let dock_state = ctx.state.borrow();
        collect_items(&dock_state)
    };
    let show_separator = !pinned_items.is_empty() && !running_items.is_empty();

    clear_children(&ctx.items_box);

    for item in &pinned_items {
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

    for item in &running_items {
        ctx.items_box.append(&item::build_item_widget(ctx, item));
    }

    if ctx.picker_popover.is_visible() {
        picker::render_picker(
            &ctx.state,
            &ctx.picker_list,
            &ctx.picker_search,
            &ctx.contexts,
            ctx.picker_search.text().as_ref(),
        );
    }
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
    collect_items_flat(state)
}

fn group_windows(
    windows: &[WindowState],
    catalog: &AppCatalog,
) -> (
    BTreeMap<String, Vec<WindowState>>,
    BTreeMap<String, Vec<WindowState>>,
) {
    let mut known: BTreeMap<String, Vec<WindowState>> = BTreeMap::new();
    let mut unknown: BTreeMap<String, Vec<WindowState>> = BTreeMap::new();

    for window in windows {
        if let Some(canonical) = window
            .app_id
            .as_deref()
            .and_then(|id| catalog.resolve_id(id))
        {
            known.entry(canonical).or_default().push(window.clone());
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
            let app = state.catalog.app(id)?.clone();
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
    let mut items: Vec<_> = known
        .into_iter()
        .filter_map(|(id, windows)| {
            let app = state.catalog.app(&id)?.clone();
            let launching = state.is_launching(&app.id);
            Some(build_known_item(app, windows, false, launching))
        })
        .collect();
    items.sort_by_cached_key(|item| (!item.active, item.label.to_lowercase()));
    items
}

fn build_unknown_items(unknown: BTreeMap<String, Vec<WindowState>>) -> Vec<DockItem> {
    let mut items: Vec<_> = unknown
        .into_iter()
        .map(|(label, windows)| build_unknown_item(label, windows))
        .collect();
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
        .filter_map(|id| catalog.resolve_id(&id))
        .filter(|id| seen.insert(id.clone()))
        .collect()
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

#[derive(Clone, Default)]
struct ConfigChanges {
    flags: Arc<AtomicU8>,
}

impl ConfigChanges {
    const PINS: u8 = 1;
    const SETTINGS: u8 = 1 << 1;
    const STYLE: u8 = 1 << 2;
    const ALL: u8 = Self::PINS | Self::SETTINGS | Self::STYLE;

    fn mark_event(&self, event: &Event) {
        let mut flags = 0;
        for path in &event.paths {
            match path.file_name().and_then(|name| name.to_str()) {
                Some("pins.json") => flags |= Self::PINS,
                Some("settings.json") => flags |= Self::SETTINGS,
                Some("style.css") => flags |= Self::STYLE,
                _ => {}
            }
        }
        if event.paths.is_empty() {
            flags = Self::ALL;
        }
        if flags != 0 {
            self.flags.fetch_or(flags, Ordering::Release);
        }
    }

    fn take(&self) -> u8 {
        self.flags.swap(0, Ordering::AcqRel)
    }
}

struct ConfigWatchState {
    watcher: RecommendedWatcher,
    directory: std::path::PathBuf,
}

impl ConfigWatchState {
    fn new(changes: ConfigChanges) -> Result<Self, String> {
        let directory = config::config_dir()
            .ok_or_else(|| "configuration directory is unavailable".to_string())?;
        std::fs::create_dir_all(&directory)
            .map_err(|error| format!("failed to create {}: {error}", directory.display()))?;

        let logged_error = Arc::new(AtomicBool::new(false));
        let callback_error = Arc::clone(&logged_error);
        let mut watcher = RecommendedWatcher::new(
            move |result| match result {
                Ok(event) => {
                    callback_error.store(false, Ordering::Release);
                    changes.mark_event(&event);
                }
                Err(error) if !callback_error.swap(true, Ordering::AcqRel) => {
                    eprintln!("config watcher error (further errors will be suppressed): {error}");
                }
                Err(_) => {}
            },
            Config::default(),
        )
        .map_err(|error| format!("failed to create config watcher: {error}"))?;
        watcher
            .watch(&directory, RecursiveMode::NonRecursive)
            .map_err(|error| format!("failed to watch {}: {error}", directory.display()))?;

        Ok(Self { watcher, directory })
    }
}

impl Drop for ConfigWatchState {
    fn drop(&mut self) {
        let _ = self.watcher.unwatch(&self.directory);
    }
}

#[cfg(test)]
mod tests {
    use super::{AppCommand, command_from_options, validate_visibility_args};
    use gtk4::glib;

    fn options(names: &[&str]) -> glib::VariantDict {
        let options = glib::VariantDict::new(None);
        for name in names {
            options.insert(name, true);
        }
        options
    }

    #[test]
    fn no_visibility_option_means_show() {
        assert_eq!(
            command_from_options(&options(&[])).unwrap(),
            AppCommand::Show
        );
    }

    #[test]
    fn toggle_option_is_recognized() {
        assert_eq!(
            command_from_options(&options(&["toggle"])).unwrap(),
            AppCommand::Toggle
        );
    }

    #[test]
    fn visibility_options_are_mutually_exclusive() {
        let args = ["rudo", "--show", "--hide"].map(str::to_string);
        assert!(validate_visibility_args(&args).is_err());
        assert!(command_from_options(&options(&["show", "hide"])).is_err());
    }
}
