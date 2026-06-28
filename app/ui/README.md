# UI Module - macOS Hold Badge & Caret Tracking

This module provides native macOS functionality for displaying a floating badge indicator and tracking text caret positions.

## Features

### 1. Hold Badge Indicator

A small, floating circular window that follows the cursor/caret position during recording:

- **Appearance**: Customizable circular badge (default: 12px red circle)
- **Behavior**: Floats above all windows, ignores mouse events
- **Positioning**: Tracks cursor or text caret with configurable offset
- **Update Rate**: Configurable position update interval (default: 150ms)
- **Multi-Space**: Can appear on all macOS spaces

### 2. Caret Tracking

Uses macOS Accessibility API to find the text insertion point:

- Queries focused UI element
- Checks if element accepts text input (text fields, text areas, etc.)
- Returns screen coordinates of caret position
- Falls back to cursor position if caret unavailable

### 3. Cursor Position

Simple mouse cursor tracking via NSEvent.

## API

### Basic Usage

```rust
use codescribe::{show_hold_badge, hide_hold_badge};

// Show badge with default settings
show_hold_badge();

// Hide badge
hide_hold_badge();
```

### Custom Configuration

```rust
use codescribe::{show_hold_badge_with_config, HoldBadgeConfig};

let config = HoldBadgeConfig {
    diameter: 20.0,                    // Badge size in pixels
    offset: (-10.0, 10.0),             // Offset from caret/cursor (x, y)
    update_interval_ms: 100,           // Position update frequency
    color: (0.0, 1.0, 0.0, 0.9),      // RGBA color (green, 90% opacity)
};

show_hold_badge_with_config(config);
```

### Utility Functions

```rust
use codescribe::{
    focused_element_accepts_text,
    get_caret_position,
    get_cursor_position,
};

// Check if current focus is on a text input
if focused_element_accepts_text() {
    println!("User is in a text field");
}

// Get caret position (returns None if not in text field)
if let Some((x, y)) = get_caret_position() {
    println!("Caret at screen position: ({}, {})", x, y);
}

// Get mouse cursor position (always available)
let (x, y) = get_cursor_position();
println!("Cursor at: ({}, {})", x, y);
```

## Configuration Options

### HoldBadgeConfig

```rust
pub struct HoldBadgeConfig {
    /// Diameter of the badge circle in pixels
    pub diameter: f64,

    /// Offset from caret/cursor position (x, y)
    pub offset: (f64, f64),

    /// Update interval in milliseconds
    pub update_interval_ms: u64,

    /// Badge color (R, G, B, A) - values 0.0 to 1.0
    pub color: (f64, f64, f64, f64),
}
```

**Defaults:**

- `diameter`: 12.0 pixels
- `offset`: (10.0, -10.0) - right and above
- `update_interval_ms`: 150ms
- `color`: (1.0, 0.0, 0.0, 0.8) - red with 80% opacity

## Implementation Details

### Thread Safety

- Uses `Arc<Mutex<>>` for shared state
- Window pointer stored as `usize` for `Send` compatibility
- Safe to call from any thread

### Window Behavior

- **Level**: NSStatusWindowLevel (25) - floats above regular windows
- **Style**: Borderless, transparent background
- **Mouse**: Ignores all mouse events (pass-through)
- **Spaces**: Can join all macOS spaces/desktops

### Position Tracking

The badge position is updated in a background thread:

1. Check if caret position available via Accessibility API
2. Fall back to cursor position if caret not found
3. Apply configured offset
4. Update window position
5. Sleep for configured interval
6. Repeat while badge is visible

### Accessibility API Usage

The module queries:

- **AXFocusedUIElement**: Get currently focused UI element
- **AXRole**: Check element type (text field, text area, etc.)
- **AXSelectedTextRange**: Get text selection/caret position
- **AXPosition** & **AXSize**: Convert to screen coordinates

## Requirements

### Dependencies

```toml
[target.'cfg(target_os = "macos")'.dependencies]
cocoa = "0.25"
core-graphics = "0.23"
core-foundation = "0.10"
objc = "0.2"
lazy_static = "1.4"
```

### Permissions

The application needs **Accessibility** permissions to track caret positions:

1. System Settings → Privacy & Security → Accessibility
2. Add your application to the allowed list

Without these permissions:

- `focused_element_accepts_text()` returns `false`
- `get_caret_position()` returns `None`
- Badge falls back to cursor tracking

## Examples

### Recording Indicator

```rust
// Show badge when recording starts
fn on_recording_start() {
    show_hold_badge();
}

// Hide when recording stops
fn on_recording_stop() {
    hide_hold_badge();
}
```

### Smart Positioning

```rust
// Use different offsets based on text field presence
let config = if focused_element_accepts_text() {
    HoldBadgeConfig {
        offset: (5.0, -20.0),  // Near caret
        ..Default::default()
    }
} else {
    HoldBadgeConfig {
        offset: (15.0, 15.0),  // Near cursor
        ..Default::default()
    }
};

show_hold_badge_with_config(config);
```

### Visual Feedback States

```rust
// Different colors for different states
fn show_recording_badge() {
    let config = HoldBadgeConfig {
        color: (1.0, 0.0, 0.0, 0.8),  // Red = recording
        ..Default::default()
    };
    show_hold_badge_with_config(config);
}

fn show_processing_badge() {
    let config = HoldBadgeConfig {
        color: (1.0, 0.5, 0.0, 0.8),  // Orange = processing
        ..Default::default()
    };
    show_hold_badge_with_config(config);
}
```

## Demo

Run the included example:

```bash
cargo run --example hold_badge_demo
```

This will:

1. Check focused element text acceptance
2. Display cursor and caret positions
3. Show default badge for 3 seconds
4. Show custom colored badge with different size/offset
5. Clean up and exit

## Limitations

1. **macOS Only**: Uses Cocoa/AppKit APIs
2. **Accessibility Required**: Caret tracking needs system permissions
3. **No Window Retention**: Window pointer not retained across app restarts
4. **Single Badge**: Only one badge can be shown at a time (new one replaces old)

## Future Enhancements

Potential improvements:

- [ ] Multiple simultaneous badges
- [ ] Animation support (pulse, fade, etc.)
- [ ] Custom shapes (not just circles)
- [ ] Smarter caret prediction in complex text layouts
- [ ] Cache focused element checks for performance

## License

FSL-1.1-ALv2 (same as parent project)

---

Created by vetcoders (c)2025
