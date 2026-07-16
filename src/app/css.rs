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

    let user_provider = gtk::CssProvider::new();
    match config::load_style_css() {
        Ok(css) => user_provider.load_from_data(css.as_deref().unwrap_or_default()),
        Err(error) => eprintln!("failed to load user stylesheet: {error}"),
    }
    gtk::style_context_add_provider_for_display(
        &display,
        &user_provider,
        gtk::STYLE_PROVIDER_PRIORITY_USER,
    );

    Some(user_provider)
}
