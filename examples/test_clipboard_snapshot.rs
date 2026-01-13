// Test program for clipboard snapshot and smart paste functionality
//
// This example demonstrates:
// 1. ClipboardSnapshot - capturing and restoring clipboard state
// 2. paste_text_smart - paste with configurable restore
// 3. paste_and_restore - high-level smart paste
//
// Usage:
//   cargo run --example test_clipboard_snapshot

use codescribe::clipboard::{
    copy, get_clipboard, paste_and_restore, paste_text_smart, snapshot_clipboard,
};
use std::thread;
use std::time::Duration;

fn main() {
    println!("=== Clipboard Snapshot Test ===\n");

    // Test 1: Basic snapshot and restore
    println!("Test 1: Snapshot and Restore");
    println!("Setting clipboard to: 'Original Content'");
    copy("Original Content").expect("Failed to copy");

    println!("Capturing snapshot...");
    let snapshot = snapshot_clipboard().expect("Failed to capture snapshot");
    println!("  Snapshot has text: {}", snapshot.text.is_some());
    println!("  Snapshot has image: {}", snapshot.image.is_some());
    println!("  Snapshot is empty: {}", snapshot.is_empty());

    println!("\nChanging clipboard to: 'Modified Content'");
    copy("Modified Content").expect("Failed to copy");

    let current = get_clipboard().expect("Failed to get clipboard");
    println!("  Current clipboard: '{}'", current);

    println!("\nRestoring snapshot...");
    snapshot.restore().expect("Failed to restore");

    let restored = get_clipboard().expect("Failed to get clipboard");
    println!("  Restored clipboard: '{}'", restored);
    assert_eq!(restored, "Original Content");
    println!("✓ Snapshot restore successful!\n");

    // Test 2: Smart paste without restore
    println!("Test 2: Smart paste (restore=false)");
    println!("Setting clipboard to: 'Preserved Content'");
    copy("Preserved Content").expect("Failed to copy");

    println!("Note: This will paste 'Pasted Text' to your active window");
    println!("Press Enter when ready (make sure you have a text field focused)...");
    let mut input = String::new();
    std::io::stdin().read_line(&mut input).ok();

    paste_text_smart("Pasted Text", false).expect("Failed to paste");

    thread::sleep(Duration::from_millis(500));

    let after_paste = get_clipboard().expect("Failed to get clipboard");
    println!("  Clipboard after paste: '{}'", after_paste);
    assert_eq!(
        after_paste, "Pasted Text",
        "Clipboard should be 'Pasted Text' since restore=false"
    );
    println!("✓ Smart paste (no restore) successful!\n");

    // Test 3: Smart paste with restore
    println!("Test 3: Smart paste (restore=true)");
    println!("Setting clipboard to: 'Will Be Restored'");
    copy("Will Be Restored").expect("Failed to copy");

    println!("Note: This will paste 'Temporary Text' to your active window");
    println!("Press Enter when ready...");
    input.clear();
    std::io::stdin().read_line(&mut input).ok();

    paste_text_smart("Temporary Text", true).expect("Failed to paste");

    println!("Waiting for clipboard restore (default 200ms + 500ms buffer)...");
    thread::sleep(Duration::from_millis(700));

    let restored_after = get_clipboard().expect("Failed to get clipboard");
    println!("  Clipboard after restore: '{}'", restored_after);
    assert_eq!(
        restored_after, "Will Be Restored",
        "Clipboard should be restored"
    );
    println!("✓ Smart paste (with restore) successful!\n");

    // Test 4: paste_and_restore convenience function
    println!("Test 4: paste_and_restore()");
    println!("Setting clipboard to: 'Important Data'");
    copy("Important Data").expect("Failed to copy");

    println!("Note: This will paste 'Quick Paste' to your active window");
    println!("Press Enter when ready...");
    input.clear();
    std::io::stdin().read_line(&mut input).ok();

    paste_and_restore("Quick Paste").expect("Failed to paste and restore");

    println!("Waiting for clipboard restore...");
    thread::sleep(Duration::from_millis(700));

    let final_clipboard = get_clipboard().expect("Failed to get clipboard");
    println!("  Final clipboard: '{}'", final_clipboard);
    assert_eq!(
        final_clipboard, "Important Data",
        "Clipboard should be restored to 'Important Data'"
    );
    println!("✓ paste_and_restore() successful!\n");

    println!("=== All Tests Passed! ===");
}
