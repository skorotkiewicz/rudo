# Contributing to Rudo

## Set up a development build

Fork the repository and clone your fork. Install stable Rust with Cargo, rustfmt, and Clippy. Rudo also needs a C toolchain, GTK4 and GTK4 Layer Shell development files, and a `pkg-config` implementation.

On Arch Linux, install the native build dependencies with `pacman`.

```sh
sudo pacman -S --needed base-devel pkgconf gtk4 gtk4-layer-shell
```

Run the development build from the repository root in a Wayland session.

```sh
cargo run --locked
```

Stop any running Rudo instance before launching your build. Rudo forwards commands from subsequent launches to the existing instance. To test with fresh settings and pins, set `XDG_CONFIG_HOME` to an absolute path to a temporary directory for the development run.

See the [README](README.md) for supported compositors, settings, and CLI options.

## Find the code

- [`src/app/`](src/app/) contains the GTK dock, including rendering, autohide, menus, and pin drag and drop.
- [`src/backend/`](src/backend/) tracks windows and sends window actions through Niri IPC or the Wayland foreign-toplevel protocol.
- [`src/catalog.rs`](src/catalog.rs) loads desktop entries and matches window identities to launchers.
- [`src/config.rs`](src/config.rs) defines settings and handles configuration files.

## Run the checks

Pull request CI runs these checks.

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

If you have `just`, use `just fmt` to format and lint or `just check` to check formatting and lint.

The default test suite runs without a display. With a GTK display available, run the dock renderer's graphical test separately.

```sh
cargo test gtk_reuses_widgets_and_preserves_open_menus -- --ignored
```

For dock behavior changes, also try the affected workflow in a Wayland session. For changes to window tracking or actions, record which compositor and backend you tested.

## Report an issue or open a pull request

- For a bug report, include your Rudo version (`rudo --version`), compositor version, steps to reproduce, and what you expected to happen. Include relevant settings or log excerpts with personal information removed.
- Keep pull requests focused on one change. Describe the behavior that changed and the checks you ran. Add a regression test when the existing suite misses the failure, and update the README when a change affects settings or CLI usage.
- For a larger feature, you can open an issue to discuss how it fits the dock.
