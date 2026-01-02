# Clipboard Quick Reference

## TL;DR - Just Use This

```rust
use codescribe::clipboard::paste_and_restore;

paste_and_restore("Your text here").unwrap();
```

This will paste your text and restore the previous clipboard after 200ms.

---

## All Functions at a Glance

| Function | Description | Restores Clipboard? |
|----------|-------------|---------------------|
| `paste_and_restore(text)` | **Recommended** - Smart paste | ✅ Always |
| `paste_text(text)` | Legacy - respects env var | ⚙️ Configurable |
| `paste_text_smart(text, restore)` | Explicit control | ✅/❌ As specified |
| `paste(text)` | Simple paste only | ❌ Never |
| `copy(text)` | Copy without paste | N/A |
| `get_clipboard()` | Read clipboard | N/A |
| `snapshot_clipboard()` | Capture clipboard state | N/A |

---

## Common Patterns

### Pattern 1: Single Smart Paste
```rust
use codescribe::clipboard::paste_and_restore;

paste_and_restore("Hello, world!").unwrap();
// Clipboard restored automatically after 200ms
```

### Pattern 2: Multiple Pastes
```rust
use codescribe::clipboard::{paste_text_smart, snapshot_clipboard};

// Capture once
let snapshot = snapshot_clipboard().unwrap();

// Paste multiple times
paste_text_smart("First", false).unwrap();
paste_text_smart("Second", false).unwrap();
paste_text_smart("Third", false).unwrap();

// Restore once
snapshot.restore().unwrap();
```

### Pattern 3: Manual Control
```rust
use codescribe::clipboard::{snapshot_clipboard, paste};

// Full control over timing
let snapshot = snapshot_clipboard().unwrap();
paste("Temporary text").unwrap();

// Do other work...

snapshot.restore().unwrap();
```

### Pattern 4: Copy Only
```rust
use codescribe::clipboard::copy;

copy("Just copy, no paste").unwrap();
```

---

## Configuration

### Disable Auto-Restore
```bash
export RESTORE_CLIPBOARD=false
```

### Change Restore Delay
```bash
export RESTORE_CLIPBOARD_DELAY_MS=500
```

---

## API Details

### ClipboardSnapshot

```rust
pub struct ClipboardSnapshot {
    pub text: Option<String>,
    pub html: Option<String>,
    pub image: Option<ImageData<'static>>,
}

impl ClipboardSnapshot {
    pub fn capture() -> Result<Self>
    pub fn restore(&self) -> Result<()>
    pub fn is_empty(&self) -> bool
}
```

**What it captures:**
- ✅ Plain text
- ✅ Images (screenshots, etc.)
- 🚧 HTML (placeholder, arboard limitation)

### Smart Paste Functions

```rust
// Always restore
pub fn paste_and_restore(text: &str) -> Result<()>

// Configurable restore
pub fn paste_text_smart(text: &str, restore: bool) -> Result<()>

// Legacy (env-controlled)
pub fn paste_text(text: &str) -> Result<()>
```

### Simple Functions

```rust
// Copy to clipboard
pub fn copy(text: &str) -> Result<()>

// Simple paste (no restore)
pub fn paste(text: &str) -> Result<()>

// Read clipboard
pub fn get_clipboard() -> Result<String>

// Snapshot clipboard
pub fn snapshot_clipboard() -> Result<ClipboardSnapshot>
```

---

## Error Handling

All functions return `Result<()>` or `Result<T>`:

```rust
use anyhow::Result;

// Handle errors
match paste_and_restore("text") {
    Ok(()) => println!("Success!"),
    Err(e) => eprintln!("Failed to paste: {}", e),
}

// Or use unwrap() for quick tests
paste_and_restore("text").unwrap();

// Or use ? in functions returning Result
fn my_function() -> Result<()> {
    paste_and_restore("text")?;
    Ok(())
}
```

---

## Platform Notes

### macOS ✅
- Fully supported
- Uses Cmd+V for paste
- Image clipboard works perfectly

### Linux 🚧
- Clipboard snapshot works
- Keyboard simulation needs testing (Ctrl+V)

### Windows 🚧
- Clipboard snapshot works
- Keyboard simulation needs testing (Ctrl+V)

---

## Performance

| Operation | Time |
|-----------|------|
| Snapshot capture | ~5-10ms |
| Paste simulation | ~70ms |
| Restore delay | 200ms (default) |
| **Total** | ~300ms |

---

## Common Issues

### Issue: Clipboard not restoring
**Solution:** Check `RESTORE_CLIPBOARD` env var isn't set to `false`

### Issue: Restoration too slow/fast
**Solution:** Adjust `RESTORE_CLIPBOARD_DELAY_MS` env var

### Issue: Paste not working
**Solution:** Make sure target application has focus and text field is active

### Issue: Image not restoring
**Solution:** Check if source application copies images to clipboard (some don't)

---

## Testing

### Interactive Test
```bash
cargo run --example test_clipboard_snapshot
```

### Unit Tests
```bash
cargo test clipboard
```

---

## See Also

- Full documentation: `CLIPBOARD_ENHANCEMENTS.md`
- Source code: `src/clipboard.rs`
- Example: `examples/test_clipboard_snapshot.rs`

---

Created by M&K (c)2025 The LibraxisAI Team
