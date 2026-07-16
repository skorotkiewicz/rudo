mod app;
mod backend;
mod catalog;
mod config;
mod model;

fn main() -> std::process::ExitCode {
    app::run().into()
}
