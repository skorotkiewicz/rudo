use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use gtk::prelude::*;
use gtk4 as gtk;

use crate::config;

use super::{DockState, clear_children, icon_widget};

pub(crate) fn render_picker(state: &Rc<RefCell<DockState>>, picker_list: &gtk::Box, query: &str) {
    clear_children(picker_list);

    let borrow = state.borrow();
    let exclude: HashSet<&str> = borrow.pins.iter().map(|s| s.as_str()).collect();
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
