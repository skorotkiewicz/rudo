use std::rc::Rc;

use gtk::gdk;
use gtk::prelude::*;
use gtk4 as gtk;

use crate::config;

use super::{RenderContext, render_dock};

pub(crate) fn install_pin_drag_and_drop(wrapper: &gtk::Box, ctx: &RenderContext, pin_id: &str) {
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
        let state = Rc::clone(&ctx.state);
        let autohide = Rc::clone(&ctx.autohide);
        let items_box = ctx.items_box.clone();
        let picker_search = ctx.picker_search.clone();
        let picker_list = ctx.picker_list.clone();
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
                let ctx = RenderContext {
                    state: Rc::clone(&state),
                    items_box: items_box.clone(),
                    picker_search: picker_search.clone(),
                    picker_list: picker_list.clone(),
                    autohide: Rc::clone(&autohide),
                };
                render_dock(&ctx);
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
