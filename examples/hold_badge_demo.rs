//! Demo of the hold badge UI functionality
//!
//! Run with: cargo run --example hold_badge_demo

#[cfg(target_os = "macos")]
fn main() {
    use codescribe::{
        focused_element_accepts_text, get_caret_position, get_cursor_position, hide_hold_badge,
        show_hold_badge, HoldBadgeConfig,
    };
    use std::thread;
    use std::time::Duration;

    println!("=== Hold Badge Demo ===\n");

    // Test 1: Check if focused element accepts text
    println!("1. Checking if focused element accepts text...");
    let accepts_text = focused_element_accepts_text();
    println!("   Accepts text: {}\n", accepts_text);

    // Test 2: Get cursor position
    println!("2. Getting cursor position...");
    let (cursor_x, cursor_y) = get_cursor_position();
    println!("   Cursor: ({}, {})\n", cursor_x, cursor_y);

    // Test 3: Get caret position (if available)
    println!("3. Getting caret position...");
    match get_caret_position() {
        Some((x, y)) => println!("   Caret: ({}, {})\n", x, y),
        None => println!("   Caret not available (no text field focused)\n"),
    }

    // Test 4: Show default badge
    println!("4. Showing default badge (12px red circle)...");
    println!("   Move your mouse to see the badge follow it!");
    show_hold_badge();
    thread::sleep(Duration::from_secs(3));

    // Test 5: Hide badge
    println!("\n5. Hiding badge...");
    hide_hold_badge();
    thread::sleep(Duration::from_secs(1));

    // Test 6: Show custom badge
    println!("\n6. Showing custom badge (20px, blue-ish, different offset)...");
    let custom_config = HoldBadgeConfig {
        diameter: 20.0,
        offset: (-15.0, 15.0),
        update_interval_ms: 100,
        color: (0.2, 0.4, 1.0, 0.9), // Blue-ish
    };
    codescribe::show_hold_badge_with_config(custom_config);
    println!("   Badge will be larger and to the left/below cursor");
    thread::sleep(Duration::from_secs(3));

    // Test 7: Final cleanup
    println!("\n7. Cleaning up...");
    hide_hold_badge();

    println!("\n=== Demo Complete ===");
}

#[cfg(not(target_os = "macos"))]
fn main() {
    println!("This example only works on macOS");
}
