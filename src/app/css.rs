use gtk::gdk;
use gtk4 as gtk;

use crate::config;

const CSS: &str = include_str!("style.css");

pub(crate) fn install() -> Option<gtk::CssProvider> {
    let display = gdk::Display::default()?;

    let provider = gtk::CssProvider::new();
    provider.load_from_data(CSS);
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    config::ensure_style_css();
    let user_provider = gtk::CssProvider::new();
    user_provider.load_from_data(&config::load_style_css().unwrap_or_default());
    gtk::style_context_add_provider_for_display(
        &display,
        &user_provider,
        gtk::STYLE_PROVIDER_PRIORITY_USER,
    );

    Some(user_provider)
}
