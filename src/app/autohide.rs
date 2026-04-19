use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gtk::glib::{self, ControlFlow};
use gtk::prelude::*;
use gtk4 as gtk;
use gtk4_layer_shell::LayerShell;

use crate::config;

pub(crate) struct AutoHideState {
    pub(crate) revealer: gtk::Revealer,
    pub(crate) hide_source: Option<glib::SourceId>,
    pub(crate) enabled: bool,
    pub(crate) delay: Duration,
}

impl AutoHideState {
    pub(crate) fn new(revealer: &gtk::Revealer, enabled: bool, delay: Duration) -> Self {
        Self {
            revealer: revealer.clone(),
            hide_source: None,
            enabled,
            delay,
        }
    }
}

pub(crate) fn show_dock(autohide: &Rc<RefCell<AutoHideState>>) {
    let mut state = autohide.borrow_mut();
    if let Some(source) = state.hide_source.take() {
        source.remove();
    }
    state.revealer.set_reveal_child(true);
}

pub(crate) fn install_hover(
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

pub(crate) fn schedule_hide(autohide: &Rc<RefCell<AutoHideState>>) {
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

pub(crate) fn apply_settings(
    window: &gtk::ApplicationWindow,
    hover_strip: &gtk::Box,
    autohide: &Rc<RefCell<AutoHideState>>,
    settings: &config::Settings,
) {
    let enabled = settings.autohide.enabled;
    let delay = Duration::from_secs(settings.autohide.delay_secs.max(1));

    if gtk4_layer_shell::is_supported() {
        if enabled {
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
