# rudo

A small, elegant dock for Wayland.

`rudo` is built for a clean desktop: pinned apps, live running windows, gentle autohide, and simple user configuration. It is designed to feel at home on `niri`, while still working with other Wayland compositors that expose the right protocols.

## Features

- Wayland-first dock UI with GTK4 + layer-shell
- Persistent pinned apps
- Running window tracking
- Launch feedback to prevent double-click spam
- Optional autohide with hover-to-reveal
- User theming via CSS
- User behavior settings via JSON

## Compositor Support

- `niri`: best experience, using `NIRI_SOCKET` integration
- Other Wayland compositors: works through `wlr-foreign-toplevel-management` when available

## Build

```sh
cargo build --release
```

Or with `just`:

```sh
just build
```

## Run

```sh
cargo run --release
```

Or:

```sh
just run
```

## Configuration

`rudo` stores its user files in `~/.config/rudo/`.

- `pins.json`: pinned applications
- `settings.json`: behavior settings
- `style.css`: visual overrides

Default `settings.json`:

```json
{
  "autohide": {
    "enabled": true,
    "delay_secs": 3
  }
}
```

`style.css` is loaded on every start after the built-in theme, so you can override the dock without rebuilding.

## Development

```sh
just fmt
just check
```

## Status

`rudo` is intentionally small. The codebase is structured to stay easy to change as more dock behavior is added over time.
