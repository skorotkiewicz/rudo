# Spec: Badge Notifications for Rudo Dock

## Objective
Add visual badge indicators to dock items showing unread notification counts from applications (Discord, Telegram, Element, etc.). This helps users quickly identify which apps need attention without opening them.

**Success Criteria:**
- [ ] Badge counts display on dock items with unread notifications
- [ ] Badges update in real-time as notifications arrive/are cleared
- [ ] Badges are visually distinct and don't interfere with icons
- [ ] Zero-count badges are hidden (not shown as "0")
- [ ] Feature works with Wayland foreign toplevel protocol
- [ ] Graceful degradation when compositor doesn't support badges

## Tech Stack
- **Language**: Rust (edition 2024)
- **UI Framework**: GTK4 + gtk4-layer-shell
- **Wayland Protocol**: zwlr_foreign_toplevel_management_v1 with badge extension
- **Data Flow**: Backend → Model → UI

## Architecture Overview

### Data Flow
```
Wayland Compositor (sway, Hyprland, etc.)
    ↓ (zwlr_foreign_toplevel_handle_v1.set_badge event)
Backend (wayland.rs)
    ↓ (BackendEvent::BadgeUpdate { id, count })
Model (model.rs)
    ↓ (WindowState with badge_count field)
App State (app.rs)
    ↓ (render_dock with badge display)
UI (badge widget overlay on dock items)
```

## Implementation Plan

### Phase 1: Model Updates
Add badge support to data structures
- Files: `src/model.rs`, `src/catalog.rs`
- Changes:
  - Add `badge_count: Option<u32>` to `WindowState`
  - Add `badge_count` to `ToplevelState` in wayland.rs
  - Consider adding app-level badges for apps without windows

### Phase 2: Backend Protocol Support
Extend Wayland backend to receive badge events
- Files: `src/backend/wayland.rs`
- Changes:
  - Handle badge-related events from foreign_toplevel protocol
  - Emit `BackendEvent::BadgeUpdate` when badge count changes
  - Update `ToplevelState` with current badge count

### Phase 3: UI Rendering
Add badge display to dock items
- Files: `src/app.rs`
- Changes:
  - Create `build_badge_widget()` helper
  - Modify `build_item_widget()` to include badge overlay
  - Style badges with CSS (red dot with number, hide when 0)
  - Update signature generation to include badge state

### Phase 4: CSS Styling
Add visual styling for badges
- Files: `src/app.rs` (CSS const)
- Changes:
  - Badge container with positioning
  - Number styling (font size, color, background)
  - Animation for badge appearance (optional)

## Project Structure

```
src/
├── app.rs          ← UI changes for badge display
├── model.rs        ← Add badge_count field
├── backend/
│   ├── wayland.rs  ← Handle badge protocol events
│   └── niri.rs     ← Stub implementation (unsupported)
└── catalog.rs      ← Optional: app-level badge support
```

## Code Style

**Idiomatic Rust patterns:**
- Use `Option<u32>` for badge counts (None = no badge, Some(0) = should be hidden)
- Propagate errors with `?` operator
- Avoid unwrap() in production code
- Use pattern matching for protocol event handling

**Example pattern:**
```rust
// Model change
pub struct WindowState {
    pub id: String,
    pub app_id: Option<String>,
    pub title: Option<String>,
    pub active: bool,
    pub badge_count: Option<u32>, // NEW
}

// Protocol handling
zwlr_foreign_toplevel_handle_v1::Event::Badge { count } => {
    window.badge_count = Some(count);
    state.publish_snapshot();
}
```

## Testing Strategy

**Manual testing:**
1. Launch app that supports badges (Discord, Telegram with tray)
2. Send message to trigger notification
3. Verify badge appears on dock item
4. Open app and clear notifications
5. Verify badge disappears

**Verification:**
- Run: `cargo build` - compiles without errors
- Run: `cargo clippy -- -D warnings` - passes linting
- Test on compositor with badge support (Hyprland, sway with patches)

## Boundaries

**Always:**
- Preserve exact existing behavior when badges not supported
- Hide badge widget when count is None or Some(0)
- Follow existing code patterns in backend/wayland.rs

**Ask first:**
- Adding new dependencies
- Changing public API of model structs
- Supporting additional notification protocols beyond Wayland

**Never:**
- Break existing dock functionality
- Show "0" badge counts
- Add blocking I/O in async contexts

## Open Questions

1. **Badge protocol support**: Not all compositors implement the badge extension. Should we:
   - A) Check for protocol support at startup and disable feature if unavailable?
   - B) Try to use it and gracefully ignore errors?
   - C) Support alternative badge sources (D-Bus, files)?

2. **App-level vs Window-level badges**: Some apps (Discord) have one window but app-level notifications. Should we:
   - A) Add badge support to `AppRecord` in catalog?
   - B) Associate badge with "primary" window?
   - C) Show badge on all windows of same app?

3. **Badge aggregation**: If multiple windows of same app have different badge counts:
   - A) Sum them?
   - B) Show highest?
   - C) Show "!" indicator instead of number?

## Decisions

**Pending human input on open questions.** Default approach:
1. Check protocol support at runtime, silently disable if unavailable
2. Window-level only for MVP (app-level in future enhancement)
3. Sum badge counts across all windows of same app for display
