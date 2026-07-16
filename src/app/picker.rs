use std::collections::HashSet;

use gtk::prelude::*;
use gtk4 as gtk;

use crate::config;

use super::{RenderContext, clear_children, icon_widget, render_all};

pub(crate) fn render_picker(ctx: &RenderContext, query: &str) {
    let state = &ctx.state;
    let picker_list = &ctx.picker_list;
    clear_children(picker_list);

    let borrow = state.borrow();
    let exclude: HashSet<&str> = borrow.pins.iter().map(|s| s.as_str()).collect();
    let icon_size = borrow.icon_size;
    let matches: Vec<_> = borrow
        .catalog
        .search(query, 40, &exclude)
        .into_iter()
        .cloned()
        .collect();
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
            let ctx = ctx.clone();
            row_button.connect_clicked(move |_| {
                let mut dock_state = ctx.state.borrow_mut();
                if !dock_state.pins.iter().any(|pin| pin == &app.id) {
                    dock_state.pins.push(app.id.clone());
                    if let Err(error) = config::save_pins(&dock_state.pins) {
                        eprintln!("failed to save dock pins: {error}");
                    }
                }
                drop(dock_state);
                ctx.picker_search.set_text("");
                render_all(&ctx);
            });
        }

        picker_list.append(&row_button);
    }
}
