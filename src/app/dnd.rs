use std::rc::Rc;

use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use gtk4 as gtk;

use crate::config;

use super::{RenderContext, autohide, render_all};

pub(crate) fn install_pin_drag_and_drop(
    wrapper: &gtk::Box,
    overlay: &gtk::Overlay,
    ctx: &RenderContext,
    pin_id: &str,
) {
    let drag_source = gtk::DragSource::new();
    drag_source.set_actions(gdk::DragAction::MOVE);
    let source_pin = pin_id.to_string();
    drag_source.connect_prepare(move |_, _, _| {
        Some(gdk::ContentProvider::for_value(&source_pin.to_value()))
    });
    {
        let autohide = Rc::downgrade(&ctx.autohide);
        drag_source.connect_drag_begin(move |_, _| {
            if let Some(autohide) = autohide.upgrade() {
                autohide::begin_drag(&autohide);
            }
        });
    }
    {
        let ctx = ctx.clone();
        // drag-end also runs for cancelled drags. Wait until GTK finishes its
        // signal handlers before applying updates that may replace the source.
        drag_source.connect_drag_end(move |_, _, _| {
            let ctx = ctx.clone();
            glib::idle_add_local_once(move || {
                autohide::end_drag(&ctx.autohide);
                render_all(&ctx);
            });
        });
    }
    wrapper.add_controller(drag_source);

    let orientation = ctx.items_box.orientation();
    let marker = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    marker.add_css_class("dock-drop-marker");
    marker.set_can_target(false);
    marker.set_visible(false);
    if orientation == gtk::Orientation::Horizontal {
        marker.set_valign(gtk::Align::Fill);
    } else {
        marker.add_css_class("is-horizontal");
        marker.set_halign(gtk::Align::Fill);
    }
    // Overlay the line in the gap without moving icons or their drop targets.
    overlay.set_overflow(gtk::Overflow::Visible);
    overlay.add_overlay(&marker);

    let drop_target = gtk::DropTarget::new(String::static_type(), gdk::DragAction::MOVE);
    let update_marker = {
        let wrapper = wrapper.downgrade();
        let marker = marker.downgrade();
        move |_: &gtk::DropTarget, x, y| {
            let (Some(wrapper), Some(marker)) = (wrapper.upgrade(), marker.upgrade()) else {
                return gdk::DragAction::empty();
            };
            let side = insertion_side(&wrapper, orientation, x, y);
            marker.remove_css_class("before");
            marker.remove_css_class("after");
            let align = match side {
                InsertionSide::Before => {
                    marker.add_css_class("before");
                    gtk::Align::Start
                }
                InsertionSide::After => {
                    marker.add_css_class("after");
                    gtk::Align::End
                }
            };
            if orientation == gtk::Orientation::Horizontal {
                marker.set_halign(align);
            } else {
                marker.set_valign(align);
            }
            marker.set_visible(true);
            gdk::DragAction::MOVE
        }
    };
    drop_target.connect_enter(update_marker.clone());
    drop_target.connect_motion(update_marker);

    {
        let marker = marker.downgrade();
        drop_target.connect_leave(move |_| {
            if let Some(marker) = marker.upgrade() {
                marker.set_visible(false);
            }
        });
    }

    {
        let ctx = ctx.clone();
        let target_pin = pin_id.to_string();
        let wrapper = wrapper.downgrade();
        let marker = marker.downgrade();

        drop_target.connect_drop(move |_, value, x, y| {
            let Some(wrapper) = wrapper.upgrade() else {
                return false;
            };
            if let Some(marker) = marker.upgrade() {
                marker.set_visible(false);
            }

            let Ok(dragged_pin) = value.get::<String>() else {
                return false;
            };

            let side = insertion_side(&wrapper, orientation, x, y);
            let changed = {
                let mut dock_state = ctx.state.borrow_mut();
                reorder_pins(&mut dock_state.pins, &dragged_pin, &target_pin, side)
            };

            if changed {
                if let Err(error) = config::save_pins(&ctx.state.borrow().pins) {
                    eprintln!("failed to save dock pins: {error}");
                }
                render_all(&ctx);
            }

            changed
        });
    }

    wrapper.add_controller(drop_target);
}

#[derive(Clone, Copy)]
enum InsertionSide {
    Before,
    After,
}

fn insertion_side(
    wrapper: &gtk::Box,
    orientation: gtk::Orientation,
    x: f64,
    y: f64,
) -> InsertionSide {
    let (position, extent) = match orientation {
        gtk::Orientation::Horizontal => (x, wrapper.allocated_width()),
        gtk::Orientation::Vertical => (y, wrapper.allocated_height()),
        _ => return InsertionSide::Before,
    };
    if position > f64::from(extent) / 2.0 {
        InsertionSide::After
    } else {
        InsertionSide::Before
    }
}

fn reorder_pins(
    pins: &mut Vec<String>,
    dragged_pin: &str,
    target_pin: &str,
    side: InsertionSide,
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
    if matches!(side, InsertionSide::After) {
        target_idx += 1;
    }

    pins.insert(target_idx.min(pins.len()), dragged);
    true
}

#[cfg(test)]
mod tests {
    use super::{InsertionSide, reorder_pins};

    fn pins() -> Vec<String> {
        ["a", "b", "c"].map(str::to_string).to_vec()
    }

    #[test]
    fn moves_pin_before_target() {
        let mut pins = pins();
        assert!(reorder_pins(&mut pins, "c", "a", InsertionSide::Before));
        assert_eq!(pins, ["c", "a", "b"]);
    }

    #[test]
    fn moves_pin_after_target() {
        let mut pins = pins();
        assert!(reorder_pins(&mut pins, "a", "b", InsertionSide::After));
        assert_eq!(pins, ["b", "a", "c"]);
    }

    #[test]
    fn ignores_self_and_unknown_pins() {
        let mut pins = pins();
        assert!(!reorder_pins(&mut pins, "a", "a", InsertionSide::Before));
        assert!(!reorder_pins(
            &mut pins,
            "missing",
            "a",
            InsertionSide::Before
        ));
        assert_eq!(pins, ["a", "b", "c"]);
    }
}
