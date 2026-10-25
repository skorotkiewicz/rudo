mod app;
mod backend;
mod catalog;
mod config;
mod model;

#[cfg(test)]
mod desktop_entries;

fn main() -> std::process::ExitCode {
    app::run().into()
}
