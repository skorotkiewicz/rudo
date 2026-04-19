mod app;
mod backend;
mod catalog;
mod config;
mod model;

fn main() {
    if std::env::args().any(|arg| arg == "-V" || arg == "--version") {
        println!("rudo {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    app::run();
}
