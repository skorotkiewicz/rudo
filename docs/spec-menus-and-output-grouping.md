# Spec: Menu System and Output-Based Window Grouping

## Objective
Add two new features to Rudo dock:
1. **Custom Menu System** - Configurable dock menus (e.g., power menu with shutdown, restart, logout) similar to rofi-style power menus
2. **Output/Workspace Grouping** - Group and order dock items by output (monitor) and window coordinates, showing windows in spatial order

**Success Criteria:**
- [ ] Menu configuration in settings.json supports custom menus with commands
- [ ] Menu button appears on dock with configurable position
- [ ] Menu popover displays with clickable menu items
- [ ] Menu items execute configured shell commands
- [ ] Windows are grouped by output (monitor) they appear on
- [ ] Within each output group, windows are ordered by their spatial coordinates
- [ ] Output groups show separator or label in dock
- [ ] Feature works with Wayland foreign toplevel protocol

## Tech Stack
- **Language**: Rust (edition 2024)
- **UI Framework**: GTK4 + gtk4-layer-shell
- **Wayland Protocol**: zwlr_foreign_toplevel_management_v1 + wl_output
- **Configuration**: JSON-based settings (existing)

## Architecture Overview

### Menu System

```
settings.json
    ↓
config.rs (load_settings)
    ↓
app.rs (build_ui - create menu button)
    ↓
Menu widget with popover
    ↓
Shell command execution via std::process::Command
```

### Output Grouping

```
Wayland Compositor
    ↓ (wl_output events + foreign_toplevel coordinates)
Backend (wayland.rs)
    ↓ (WindowState with output_id and coordinates)
Model (model.rs)
    ↓
App State (app.rs)
    ↓ (collect_items_by_output - group and sort)
UI (dock items ordered by output then coordinates)
```

## Implementation Plan

### Phase 1: Menu System

**Config Structure:**
```rust
// In config.rs Settings
pub struct MenuConfig {
    pub enabled: bool,
    pub icon: String,           // Icon name for menu button
    pub position: MenuPosition,   // Start or End of dock
    pub items: Vec<MenuItem>,
}

pub struct MenuItem {
    pub label: String,
    pub icon: Option<String>,
    pub command: String,          // Shell command to execute
    pub confirm: bool,            // Require confirmation dialog
}
```

**UI Changes:**
- Add menu button to dock (next to picker button)
- Create menu popover with styled buttons
- Execute commands via `std::process::Command`
- Optional confirmation dialogs for destructive actions

**Settings Schema:**
```json
{
  "menu": {
    "enabled": true,
    "icon": "system-shutdown-symbolic",
    "position": "end",
    "items": [
      {"label": "Shutdown", "icon": "system-shutdown-symbolic", "command": "systemctl poweroff", "confirm": true},
      {"label": "Restart", "icon": "system-restart-symbolic", "command": "systemctl reboot", "confirm": true},
      {"label": "Logout", "icon": "system-log-out-symbolic", "command": "loginctl terminate-user $USER", "confirm": true}
    ]
  }
}
```

### Phase 2: Output/Workspace Grouping

**Model Changes:**
```rust
// In model.rs WindowState
pub struct WindowState {
    pub id: String,
    pub app_id: Option<String>,
    pub title: Option<String>,
    pub active: bool,
    pub badge_count: Option<u32>,
    pub output_id: Option<u32>,      // Output/monitor ID
    pub coordinates: Option<(i32, i32)>, // Window x, y coordinates
}
```

**Backend Changes (wayland.rs):**
- Track wl_output globals with IDs
- Store output ID in ToplevelState when window appears
- Update coordinates from compositor when available

**Sorting Logic (app.rs):**
```rust
fn collect_items_grouped_by_output(state: &DockState) -> Vec<(OutputInfo, Vec<DockItem>)> {
    // Group windows by output_id
    // Sort each group by coordinates (y then x for top-to-bottom, left-to-right)
    // Return ordered groups with output labels
}
```

**UI Changes:**
- Show output name/label before each group
- Separator between output groups
- Maintain pin behavior (pins shown before running windows in each group?)

## Project Structure

```
src/
├── app.rs          ← Menu UI, output grouping in collect_items
├── model.rs        ← output_id and coordinates fields
├── config.rs       ← MenuConfig, MenuItem structs
└── backend/
    ├── wayland.rs  ← Track outputs and window coordinates
    └── niri.rs     ← Stub implementation for output features
```

## Code Style

**Idiomatic Rust patterns:**
- Use Option<T> for fields that may not be available
- Propagate errors with ? operator
- Use pattern matching for event handling
- Shell command execution uses `std::process::Command`

**Example patterns:**
```rust
// Menu execution
fn execute_menu_item(item: &MenuItem) {
    if item.confirm {
        show_confirmation_dialog(&item.label, || {
            let _ = std::process::Command::new("sh")
                .arg("-c")
                .arg(&item.command)
                .spawn();
        });
    } else {
        let _ = std::process::Command::new("sh")
            .arg("-c")
            .arg(&item.command)
            .spawn();
    }
}

// Output grouping
windows.iter()
    .filter_map(|w| w.output_id.map(|id| (id, w)))
    .into_group_map()
    .into_iter()
    .sorted_by_key(|(id, _)| *id)
    .map(|(id, group)| {
        let sorted = group.into_iter()
            .sorted_by_key(|w| w.coordinates.unwrap_or((0, 0)))
            .collect();
        (id, sorted)
    })
    .collect()
```

## Testing Strategy

**Manual testing:**
1. Add menu configuration to settings.json
2. Restart dock, verify menu button appears
3. Click menu button, verify popover with items
4. Click menu item, verify command executes
5. Open windows on multiple monitors
6. Verify windows grouped by output in dock
7. Verify spatial ordering within groups

**Verification:**
- Run: `cargo build` - compiles without errors
- Run: `cargo clippy -- -D warnings` - passes linting
- Test menu command with: `echo "test" > /tmp/rudo-test`

## Boundaries

**Always:**
- Preserve existing behavior when menu is disabled
- Handle missing output info gracefully (fall back to current behavior)
- Escape shell commands properly to prevent injection
- Validate config on load (warn on invalid menu items)

**Ask first:**
- Adding new dependencies (e.g., for confirmation dialogs)
- Changing the default dock behavior (pins vs output grouping)
- Supporting X11 backend (out of scope for now)

**Never:**
- Execute commands without user interaction (except from explicit clicks)
- Show destructive actions without confirmation when configured
- Block main thread on command execution (use spawn)

## Open Questions

1. **Menu position**: Should menu button be:
   - A) Always at start of dock (left/bottom depending on orientation)?
   - B) Always at end of dock (right/top)?
   - C) Configurable position relative to pins vs running apps?
   - D) Separate from dock entirely (standalone widget)?

2. **Pin behavior with output grouping**: When output grouping is enabled:
   - A) Show pins at start of each output group (pins appear multiple times)?
   - B) Show pins in a separate section before all output groups?
   - C) Show pins only on their "home" output?
   - D) Disable output grouping for pins entirely?

3. **Output identification**: Wayland outputs have numeric IDs but not names:
   - A) Show "Monitor 1", "Monitor 2" labels?
   - B) Show no labels, just separators between groups?
   - C) Try to get output names from xrandr or similar?

4. **Menu confirmation**: For destructive actions:
   - A) Built-in GTK confirmation dialogs?
   - B) External command (e.g., `wlogout`, `wofi --dmenu`)?
   - C) Simple inline confirmation in the popover?

## Decisions

**Pending human input on open questions.** Default approach:
1. Configurable menu position (Start/End enum)
2. Pins shown in separate section before output groups (maintain current pin behavior)
3. Simple separators between output groups (no labels for now)
4. Built-in GTK confirmation dialogs (menu pauses until confirmed)
