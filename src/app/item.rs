use std::cell::RefCell;
use std::rc::Rc;

use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use gtk4 as gtk;

use crate::catalog::AppRecord;
use crate::config;
use crate::model::WindowState;

use super::{DockItem, DockState, RenderContext, autohide, icon_widget};

pub(crate) fn build_item_widget(ctx: &RenderContext, item: &DockItem) -> gtk::Box {
    let state = &ctx.state;
    let autohide = &ctx.autohide;

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
        let windows = item.windows.clone();
        let app = item.app.clone();
        let ctx = ctx.clone();
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
                        super::render_dock(&ctx);
                    }
                    Err(error) => eprintln!("failed to launch {}: {error}", app.id),
                }
            }
        });
    }

    {
        let state = Rc::clone(state);
        let ctx = ctx.clone();
        let popover = build_context_menu(Rc::clone(&state), &button, item, autohide, move || {
            super::render_dock(&ctx)
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

    let button_overlay = gtk::Overlay::new();
    button_overlay.set_child(Some(&button));
    if let Some(badge) = badge_widget(item.badge_count) {
        button_overlay.add_overlay(&badge);
    }

    wrapper.append(&button_overlay);
    wrapper.append(&indicator);

    if item.pinned
        && let Some(app) = item.app.as_ref()
    {
        super::dnd::install_pin_drag_and_drop(&wrapper, ctx, &app.id);
    }

    wrapper
}

#[allow(clippy::too_many_lines)]
pub(crate) fn build_context_menu(
    state: Rc<RefCell<DockState>>,
    parent: &impl gtk::prelude::IsA<gtk::Widget>,
    item: &DockItem,
    autohide: &Rc<RefCell<autohide::AutoHideState>>,
    rerender: impl Fn() + 'static,
) -> gtk::Popover {
    let popover = gtk::Popover::new();
    popover.set_has_arrow(false);
    popover.set_position(gtk::PositionType::Top);
    popover.set_parent(parent);

    autohide::install_hover(&popover, autohide);

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

    if let Some(app) = item.app.as_ref() {
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

fn badge_widget(count: Option<u32>) -> Option<gtk::Box> {
    let count = count?;
    if count == 0 {
        return None;
    }

    let badge = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    badge.add_css_class("dock-badge");
    badge.set_halign(gtk::Align::End);
    badge.set_valign(gtk::Align::Start);

    let label_text = if count > 99 {
        "99+".to_string()
    } else {
        count.to_string()
    };

    let label = gtk::Label::new(Some(&label_text));
    label.add_css_class("dock-badge-label");
    badge.append(&label);

    Some(badge)
}

fn window_menu_label(window: &WindowState, multiple: bool) -> String {
    let title = window.title.as_deref().unwrap_or("Untitled Window");
    let active_suffix = if window.active { " (active)" } else { "" };

    if multiple || window.active {
        format!("Focus {title}{active_suffix}")
    } else {
        "Focus Window".to_string()
    }
}

pub(crate) struct MenuButton {
    pub(crate) button: gtk::Button,
    pub(crate) popover: gtk::Popover,
}

pub(crate) fn build_menu_button(menu_config: &config::MenuConfig, icon_size: i32) -> MenuButton {
    let button = gtk::Button::new();
    button.add_css_class("dock-item");
    button.add_css_class("menu-button");
    button.set_tooltip_text(Some("Menu"));
    button.set_child(Some(&icon_from_name(&menu_config.icon, icon_size)));

    let popover = gtk::Popover::new();
    popover.set_has_arrow(false);
    popover.set_position(gtk::PositionType::Top);
    popover.set_parent(&button);

    let layout = gtk::Box::new(gtk::Orientation::Vertical, 6);
    layout.add_css_class("dock-menu");
    layout.set_margin_top(6);
    layout.set_margin_bottom(6);
    layout.set_margin_start(6);
    layout.set_margin_end(6);

    for item in &menu_config.items {
        let menu_item = gtk::Button::new();
        menu_item.add_css_class("dock-menu-item");

        let item_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        if let Some(ref icon_name) = item.icon {
            let icon = icon_from_name(icon_name, 16);
            item_box.append(&icon);
        }
        let label = gtk::Label::new(Some(&item.label));
        label.set_xalign(0.0);
        label.set_hexpand(true);
        item_box.append(&label);

        menu_item.set_child(Some(&item_box));

        let command = item.command.clone();
        let confirm = item.confirm;
        let label_text = item.label.clone();

        menu_item.connect_clicked(glib::clone!(
            #[weak]
            popover,
            move |_| {
                popover.popdown();
                if confirm {
                    show_confirmation_dialog(&label_text, &command, &popover);
                } else {
                    execute_command(&command);
                }
            }
        ));

        layout.append(&menu_item);
    }

    popover.set_child(Some(&layout));

    let popover_clone = popover.clone();
    button.connect_clicked(move |_| {
        popover_clone.popup();
    });

    MenuButton { button, popover }
}

fn icon_from_name(icon_name: &str, icon_size: i32) -> gtk::Image {
    let image = gtk::Image::from_icon_name(icon_name);
    image.set_pixel_size(icon_size);
    image
}

fn execute_command(command: &str) {
    if let Err(e) = std::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .spawn()
    {
        eprintln!("Failed to execute command '{command}': {e}");
    }
}

fn show_confirmation_dialog(label: &str, command: &str, parent: &impl IsA<gtk::Widget>) {
    let window = gtk::Window::new();
    window.set_title(Some(&format!("Confirm {label}")));
    window.set_default_width(300);
    window.set_default_height(120);
    window.set_modal(true);
    window.set_transient_for(parent.root().and_downcast_ref::<gtk::Window>());

    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.set_margin_top(18);
    content.set_margin_bottom(18);
    content.set_margin_start(18);
    content.set_margin_end(18);

    let message = gtk::Label::new(Some(&format!(
        "Are you sure you want to {}?",
        label.to_lowercase()
    )));
    content.append(&message);

    let button_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    button_box.set_halign(gtk::Align::End);
    button_box.set_hexpand(true);

    let cancel = gtk::Button::with_label("Cancel");
    cancel.connect_clicked(glib::clone!(
        #[weak]
        window,
        move |_| {
            window.close();
        }
    ));

    let confirm = gtk::Button::with_label(label);
    confirm.add_css_class("destructive-action");
    let cmd = command.to_string();
    confirm.connect_clicked(glib::clone!(
        #[weak]
        window,
        move |_| {
            execute_command(&cmd);
            window.close();
        }
    ));

    button_box.append(&cancel);
    button_box.append(&confirm);
    content.append(&button_box);

    window.set_child(Some(&content));
    window.present();
}
