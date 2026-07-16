use gtk::gdk;
use gtk::prelude::*;
use gtk4 as gtk;

use crate::config;

use super::{RenderContext, render_all};

pub(crate) fn install_pin_drag_and_drop(
    wrapper: &gtk::Box,
    highlight: &gtk::Button,
    ctx: &RenderContext,
    pin_id: &str,
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
        let highlight = highlight.downgrade();
        drop_target.connect_enter(move |_, _, _| {
            if let Some(highlight) = highlight.upgrade() {
                highlight.add_css_class("is-drop-target");
            }
            gdk::DragAction::MOVE
        });
    }

    {
        let highlight = highlight.downgrade();
        drop_target.connect_leave(move |_| {
            if let Some(highlight) = highlight.upgrade() {
                highlight.remove_css_class("is-drop-target");
            }
        });
    }

    {
        let ctx = ctx.clone();
        let target_pin = pin_id.to_string();
        let wrapper = wrapper.downgrade();
        let highlight = highlight.downgrade();
        let orientation = ctx.items_box.orientation();

        drop_target.connect_drop(move |_, value, x, y| {
            let Some(wrapper) = wrapper.upgrade() else {
                return false;
            };
            if let Some(highlight) = highlight.upgrade() {
                highlight.remove_css_class("is-drop-target");
            }

            let Ok(dragged_pin) = value.get::<String>() else {
                return false;
            };

            let insert_after = match orientation {
                gtk::Orientation::Horizontal => x > f64::from(wrapper.allocated_width()) / 2.0,
                gtk::Orientation::Vertical => y > f64::from(wrapper.allocated_height()) / 2.0,
                _ => false,
            };
            let changed = {
                let mut dock_state = ctx.state.borrow_mut();
                reorder_pins(
                    &mut dock_state.pins,
                    &dragged_pin,
                    &target_pin,
                    insert_after,
                )
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

#[cfg(test)]
mod tests {
    use super::reorder_pins;

    fn pins() -> Vec<String> {
        ["a", "b", "c"].map(str::to_string).to_vec()
    }

    #[test]
    fn moves_pin_before_target() {
        let mut pins = pins();
        assert!(reorder_pins(&mut pins, "c", "a", false));
        assert_eq!(pins, ["c", "a", "b"]);
    }

    #[test]
    fn moves_pin_after_target() {
        let mut pins = pins();
        assert!(reorder_pins(&mut pins, "a", "b", true));
        assert_eq!(pins, ["b", "a", "c"]);
    }

    #[test]
    fn ignores_self_and_unknown_pins() {
        let mut pins = pins();
        assert!(!reorder_pins(&mut pins, "a", "a", false));
        assert!(!reorder_pins(&mut pins, "missing", "a", false));
        assert_eq!(pins, ["a", "b", "c"]);
    }
}
