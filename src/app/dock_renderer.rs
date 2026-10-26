//! Reconciles dock entries without tearing down unaffected GTK widgets.

use gtk::prelude::{BoxExt, Cast, OrientableExt, WidgetExt};
use gtk4 as gtk;

use super::{DockItem, RenderContext, cleanup_widget_tree, item};

struct RenderedItem {
    model: DockItem,
    widget: gtk::Box,
}

/// Retained widgets for one dock view.
#[derive(Default)]
pub(super) struct DockRenderer {
    items: Vec<RenderedItem>,
    separator: Option<gtk::Separator>,
    picker_pins: Vec<String>,
}

impl DockRenderer {
    /// Reuses unchanged entries and leaves open context menus intact.
    /// Updates are deferred during pin drags to preserve sources and drop targets.
    pub(super) fn render(
        &mut self,
        ctx: &RenderContext,
        pinned: &[DockItem],
        running: &[DockItem],
    ) {
        if ctx.autohide.borrow().is_dragging() {
            return;
        }
        let mut old = std::mem::take(&mut self.items);
        let mut previous = None::<gtk::Widget>;
        let show_separator = !pinned.is_empty() && !running.is_empty();
        if !show_separator
            && let Some(separator) = self.separator.as_ref()
            && separator.parent().is_some()
        {
            ctx.items_box.remove(separator);
        }

        for (index, model) in pinned.iter().chain(running).enumerate() {
            if show_separator && index == pinned.len() {
                let separator = self.separator.get_or_insert_with(|| {
                    let orientation = if ctx.items_box.orientation() == gtk::Orientation::Horizontal
                    {
                        gtk::Orientation::Vertical
                    } else {
                        gtk::Orientation::Horizontal
                    };
                    let separator = gtk::Separator::new(orientation);
                    separator.add_css_class("dock-separator");
                    if orientation == gtk::Orientation::Horizontal {
                        separator.add_css_class("is-vertical");
                    }
                    separator
                });
                place_after(&ctx.items_box, separator.upcast_ref(), previous.as_ref());
                previous = Some(separator.clone().upcast());
            }

            let existing = old
                .iter()
                .position(|entry| same_identity(&entry.model, model))
                .map(|index| old.swap_remove(index));
            let entry = match existing {
                Some(entry)
                    if entry.model == *model || has_open_popover(entry.widget.upcast_ref()) =>
                {
                    entry
                }
                Some(entry) => {
                    remove_entry(&ctx.items_box, &entry);
                    RenderedItem {
                        model: model.clone(),
                        widget: item::build_item_widget(ctx, model),
                    }
                }
                None => RenderedItem {
                    model: model.clone(),
                    widget: item::build_item_widget(ctx, model),
                },
            };
            place_after(&ctx.items_box, entry.widget.upcast_ref(), previous.as_ref());
            previous = Some(entry.widget.clone().upcast());
            self.items.push(entry);
        }
        for entry in old {
            remove_entry(&ctx.items_box, &entry);
        }
    }

    /// Reports whether the picker exclusions changed, retaining a snapshot for comparison.
    pub(super) fn pins_changed(&mut self, pins: &[String]) -> bool {
        if self.picker_pins == pins {
            return false;
        }
        self.picker_pins = pins.to_vec();
        true
    }

    /// Releases cached widgets before their view is dismantled.
    pub(super) fn clear(&mut self) {
        self.items.clear();
        self.separator = None;
    }
}

fn same_identity(left: &DockItem, right: &DockItem) -> bool {
    if left.pinned != right.pinned {
        return false;
    }
    match (left.app.as_ref(), right.app.as_ref()) {
        (Some(left), Some(right)) => left.id == right.id,
        (None, None) => left.label == right.label,
        _ => false,
    }
}

fn has_open_popover(widget: &gtk::Widget) -> bool {
    if let Some(popover) = widget.downcast_ref::<gtk::Popover>()
        && popover.is_visible()
    {
        return true;
    }
    let mut child = widget.first_child();
    while let Some(current) = child {
        if has_open_popover(&current) {
            return true;
        }
        child = current.next_sibling();
    }
    false
}

fn place_after(parent: &gtk::Box, child: &gtk::Widget, previous: Option<&gtk::Widget>) {
    if child.parent().is_none() {
        parent.insert_child_after(child, previous);
    } else if child.prev_sibling().as_ref() != previous {
        parent.reorder_child_after(child, previous);
    }
}

fn remove_entry(parent: &gtk::Box, entry: &RenderedItem) {
    cleanup_widget_tree(entry.widget.upcast_ref());
    parent.remove(&entry.widget);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{DockState, autohide, render_dock};
    use crate::desktop_entries::DesktopEntries;
    use crate::model::WindowState;
    use gtk::glib;
    use gtk::prelude::*;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;
    use std::time::Duration;

    #[test]
    fn picker_refreshes_only_when_pin_exclusions_change() {
        let mut renderer = DockRenderer::default();
        assert!(!renderer.pins_changed(&[]), "empty pins are unchanged");
        let pins = vec!["notes.desktop".to_string()];
        assert!(
            renderer.pins_changed(&pins),
            "new pins must refresh exclusions"
        );
        assert!(
            !renderer.pins_changed(&pins),
            "window updates must not rebuild picker rows"
        );
        assert!(
            renderer.pins_changed(&[]),
            "unpinning must refresh exclusions"
        );
    }

    fn settle() {
        let main_loop = glib::MainLoop::new(None, false);
        let done = main_loop.clone();
        glib::timeout_add_local_once(Duration::from_millis(300), move || done.quit());
        main_loop.run();
    }

    fn entry(ctx: &RenderContext, label: &str) -> gtk::Box {
        ctx.renderer
            .borrow()
            .items
            .iter()
            .find(|entry| entry.model.label == label)
            .expect("fixture entry must exist")
            .widget
            .clone()
    }

    fn assert_drop_marker(ctx: &RenderContext) {
        let wrapper = entry(ctx, "Notes");
        let overlay = wrapper
            .first_child()
            .and_downcast::<gtk::Overlay>()
            .expect("dock item must contain its overlay");
        let marker = overlay
            .child()
            .and_then(|child| child.next_sibling())
            .expect("pinned item must have an insertion marker");
        let target = wrapper
            .observe_controllers()
            .iter::<glib::Object>()
            .filter_map(Result::ok)
            .find_map(|controller| controller.downcast::<gtk::DropTarget>().ok())
            .expect("pinned item must accept drops");
        let width = wrapper.allocated_width();
        let height = wrapper.allocated_height();
        assert!(!marker.is_visible(), "insertion marker starts hidden");
        target.emit_by_name::<gtk::gdk::DragAction>("enter", &[&0.0_f64, &0.0_f64]);
        settle();
        assert!(marker.is_visible(), "entering a target must show the line");
        assert!(marker.has_css_class("before"), "first half inserts before");
        let horizontal = ctx.items_box.orientation() == gtk::Orientation::Horizontal;
        assert_eq!(
            if horizontal {
                marker.halign()
            } else {
                marker.valign()
            },
            gtk::Align::Start,
            "the line must mark the leading edge"
        );
        target.emit_by_name::<gtk::gdk::DragAction>(
            "motion",
            &[&f64::from(width), &f64::from(height)],
        );
        settle();
        assert!(marker.has_css_class("after"), "second half inserts after");
        assert!(!marker.has_css_class("before"), "only one edge is marked");
        assert_eq!(
            if horizontal {
                marker.halign()
            } else {
                marker.valign()
            },
            gtk::Align::End,
            "moving across an icon must move the insertion line"
        );
        assert_eq!(
            (wrapper.allocated_width(), wrapper.allocated_height()),
            (width, height),
            "showing the line must not resize icons or move drop targets"
        );
        assert_eq!(marker.allocated_width().min(marker.allocated_height()), 2);
        target.emit_by_name::<()>("leave", &[]);
        assert!(
            !marker.is_visible(),
            "leaving the target must hide the line"
        );
    }

    fn find_popover(widget: &gtk::Widget) -> Option<gtk::Popover> {
        if let Some(popover) = widget.downcast_ref::<gtk::Popover>() {
            return Some(popover.clone());
        }
        let mut child = widget.first_child();
        while let Some(current) = child {
            if let Some(popover) = find_popover(&current) {
                return Some(popover);
            }
            child = current.next_sibling();
        }
        None
    }

    #[test]
    #[ignore = "requires a GTK display; run explicitly during graphical smoke tests"]
    fn gtk_reuses_widgets_and_preserves_open_menus() {
        gtk::init().expect("graphical regression test requires a GTK display");
        let provider = gtk::CssProvider::new();
        let css_errors = Rc::new(RefCell::new(Vec::new()));
        let errors = Rc::clone(&css_errors);
        provider.connect_parsing_error(move |_, _, error| {
            errors.borrow_mut().push(error.to_string());
        });
        provider.load_from_data(include_str!("style.css"));
        assert!(
            css_errors.borrow().is_empty(),
            "dock CSS must parse: {css_errors:?}"
        );
        let display = gtk::gdk::Display::default().expect("GTK test requires a display");
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        let entries = DesktopEntries::new(&[
            ("notes.desktop", "Name=Notes\nExec=/bin/true"),
            ("tools.desktop", "Name=Tools\nExec=/bin/true"),
        ]);
        let contexts = Rc::new(RefCell::new(Vec::new()));
        let items_box = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let revealer = gtk::Revealer::new();
        let ctx = RenderContext {
            state: Rc::new(RefCell::new(DockState {
                catalog: entries.catalog(),
                pins: vec!["notes.desktop".to_string(), "tools.desktop".to_string()],
                windows: ["alpha", "beta"]
                    .map(|id| WindowState {
                        id: id.to_string(),
                        app_id: Some(id.to_string()),
                        title: Some(id.to_string()),
                        active: false,
                        badge_count: None,
                    })
                    .to_vec(),
                backend: None,
                launching: HashMap::new(),
                icon_size: 24,
            })),
            items_box: items_box.clone(),
            picker_search: gtk::SearchEntry::new(),
            picker_list: gtk::Box::new(gtk::Orientation::Vertical, 0),
            picker_popover: gtk::Popover::new(),
            popover_position: gtk::PositionType::Top,
            autohide: Rc::new(RefCell::new(autohide::AutoHideState::new(
                &revealer,
                false, // Autohide is disabled for deterministic widget assertions.
                Duration::from_secs(1),
            ))),
            renderer: Rc::new(RefCell::new(DockRenderer::default())),
            contexts: Rc::downgrade(&contexts),
        };
        contexts.borrow_mut().push(ctx.clone());
        let window = gtk::Window::builder()
            .title("Rudo rendering regression")
            .build();
        window.set_child(Some(&items_box));
        render_dock(&ctx);
        window.present();
        settle();
        assert_drop_marker(&ctx);
        let notes = entry(&ctx, "Notes");
        let alpha = entry(&ctx, "alpha");
        let beta = entry(&ctx, "beta");
        let separator = ctx
            .renderer
            .borrow()
            .separator
            .clone()
            .expect("mixed dock has a separator");

        render_dock(&ctx);
        assert_eq!(
            entry(&ctx, "alpha"),
            alpha,
            "identical snapshots must retain widgets"
        );
        ctx.state.borrow_mut().windows[0].title = Some("Updated title".to_string());
        render_dock(&ctx);
        let updated_alpha = entry(&ctx, "alpha");
        assert_ne!(updated_alpha, alpha, "changed entries must refresh");
        assert_eq!(
            entry(&ctx, "beta"),
            beta,
            "unrelated entries must survive title updates"
        );
        assert_eq!(ctx.renderer.borrow().separator.as_ref(), Some(&separator));

        ctx.state.borrow_mut().pins.reverse();
        render_dock(&ctx);
        assert_eq!(
            entry(&ctx, "Notes"),
            notes,
            "pin reordering must not recreate buttons"
        );
        assert_eq!(items_box.first_child(), Some(entry(&ctx, "Tools").upcast()));

        let popover =
            find_popover(updated_alpha.upcast_ref()).expect("entry must own its context menu");
        popover.popup();
        settle();
        assert!(popover.is_visible(), "context menu must actually open");
        ctx.state.borrow_mut().windows[0].title = Some("Changed while menu is open".to_string());
        render_dock(&ctx);
        assert_eq!(
            entry(&ctx, "alpha"),
            updated_alpha,
            "open menus must not lose their parent widget"
        );
        assert!(
            popover.is_visible(),
            "window updates must not dismiss open menus"
        );
        popover.popdown();
        settle();
        assert_ne!(
            entry(&ctx, "alpha"),
            updated_alpha,
            "deferred updates must apply after closing the menu"
        );

        ctx.state
            .borrow_mut()
            .windows
            .retain(|window| window.id != "beta");
        render_dock(&ctx);
        assert!(beta.parent().is_none(), "closed groups must be detached");
        ctx.state.borrow_mut().windows.clear();
        render_dock(&ctx);
        assert!(
            separator.parent().is_none(),
            "separator must disappear without running groups"
        );
        assert_eq!(
            entry(&ctx, "Notes"),
            notes,
            "pinned entries survive the last window closing"
        );

        {
            let mut state = ctx.autohide.borrow_mut();
            state.enabled = true;
            state.delay = Duration::from_millis(20);
        }
        autohide::schedule_hide(&ctx.autohide);
        autohide::begin_drag(&ctx.autohide);
        assert!(
            ctx.autohide.borrow().hide_source.is_none(),
            "starting a drag must cancel any pending hide"
        );
        autohide::schedule_hide(&ctx.autohide);
        settle();
        assert!(
            revealer.reveals_child(),
            "leaving the dock during a slow drag must not hide it"
        );
        assert!(
            ctx.autohide.borrow().hide_source.is_none(),
            "pointer leave events must not arm a hide timer during a drag"
        );
        ctx.state.borrow_mut().pins.reverse();
        ctx.state.borrow_mut().windows.push(WindowState {
            id: "notes-window".to_string(),
            app_id: Some("notes".to_string()),
            title: Some("Changed during drag".to_string()),
            active: true,
            badge_count: None,
        });
        render_dock(&ctx);
        assert_eq!(
            entry(&ctx, "Notes"),
            notes,
            "window updates must not replace an active drag source"
        );
        assert_eq!(
            items_box.first_child(),
            Some(entry(&ctx, "Tools").upcast()),
            "drop targets must remain in place until the drag ends"
        );
        autohide::end_drag(&ctx.autohide);
        render_dock(&ctx);
        assert_ne!(
            entry(&ctx, "Notes"),
            notes,
            "deferred window updates must apply when the drag ends"
        );
        assert_eq!(
            items_box.first_child(),
            Some(entry(&ctx, "Notes").upcast()),
            "deferred pin reordering must apply when the drag ends"
        );
        settle();
        assert!(
            !revealer.reveals_child(),
            "autohide must resume with a fresh delay after the drag ends"
        );
        ctx.autohide.borrow_mut().enabled = false;
        autohide::begin_drag(&ctx.autohide);
        autohide::end_drag(&ctx.autohide);
        settle();
        assert!(
            revealer.reveals_child(),
            "finishing a drag must respect disabled autohide"
        );

        ctx.renderer.borrow_mut().clear();
        super::super::clear_children(&items_box);
        items_box.set_orientation(gtk::Orientation::Vertical);
        render_dock(&ctx);
        settle();
        assert_drop_marker(&ctx);

        ctx.renderer.borrow_mut().clear();
        contexts.borrow_mut().clear();
        super::super::clear_children(&items_box);
        window.close();
        gtk::style_context_remove_provider_for_display(&display, &provider);
    }
}
