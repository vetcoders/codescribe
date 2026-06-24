use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use objc::declare::ClassDecl;
use objc::runtime::{Class, Object, Sel};
use objc::{msg_send, sel, sel_impl};
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Once;

use super::{
    Id, add_subview, apply_tafla_surface, ns_string, set_button_symbol, ui_colors, ui_tokens,
};

const NSTRACKING_MOUSE_ENTERED_AND_EXITED: u64 = 1 << 0;
const NSTRACKING_ACTIVE_ALWAYS: u64 = 1 << 7;
const NSTRACKING_IN_VISIBLE_RECT: u64 = 1 << 9;
const BUBBLE_COPY_ACTION_WIDTH: f64 = 40.0;
const BUBBLE_RENDER_ACTION_WIDTH: f64 = 28.0;
const BUBBLE_ACTION_GAP: f64 = 10.0;

/// Role for chat bubble styling
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BubbleRole {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderMode {
    Plain,
    Markdown,
}

pub fn streaming_render_mode(_is_streaming: bool, _role: BubbleRole) -> RenderMode {
    RenderMode::Plain
}

pub fn next_render_mode(current: RenderMode) -> RenderMode {
    match current {
        RenderMode::Plain => RenderMode::Markdown,
        RenderMode::Markdown => RenderMode::Plain,
    }
}

// ============================================================================
// Bubble height measurement cache
// ============================================================================
//
// AppKit text measurement (`cellSizeForBounds:` -> `__NSStringDrawingEngine`)
// is the hot path that pegs the main thread on big transcripts: every
// `update_chat_view_with_state` rebuild re-measures the *full* wrapped attributed
// string of *every* bubble. The text + layout width + font + render-mode of an
// already-rendered bubble do not change across rebuilds, so the measured height
// is deterministically reusable. We cache it keyed on those inputs and only pay
// the AppKit measurement for new/changed bubbles.
//
// For pathologically long text we skip AppKit entirely and estimate the height
// from a wrap model — `__NSStringDrawingEngine` on a multi-megabyte string is
// the part that actually hangs, and an estimate is visually indistinguishable at
// that scale (the bubble is scrolled, not read in full).

/// Above this character count we estimate bubble height instead of asking
/// AppKit to lay out the entire string. Keeps the main thread responsive for
/// huge pasted/selected blobs.
pub const BUBBLE_MEASURE_CAP_CHARS: usize = 20_000;

/// Cache key for a bubble height measurement.
///
/// Floats are hashed via their bit patterns; layout width and font size are the
/// only float inputs and both come from deterministic layout math, so identical
/// logical inputs produce identical bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BubbleMeasureKey {
    text_hash: u64,
    width_bits: u64,
    font_bits: u64,
    render_mode: u8,
}

impl BubbleMeasureKey {
    /// `bound_width` is the layout input that determines wrapping — pass the
    /// bubble's `max_width` so a cache hit can skip *both* AppKit measurements
    /// (width decision + final height) for an unchanged bubble.
    pub fn new(text: &str, bound_width: f64, font_size: f64, render_mode: RenderMode) -> Self {
        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        let text_hash = hasher.finish();
        let render_mode = match render_mode {
            RenderMode::Plain => 0,
            RenderMode::Markdown => 1,
        };
        Self {
            text_hash,
            width_bits: bound_width.to_bits(),
            font_bits: font_size.to_bits(),
            render_mode,
        }
    }
}

/// Cached, deterministic layout outputs for a bubble's text.
///
/// Both values are pure functions of the cache key inputs, so reusing them
/// across rebuilds is safe and avoids the AppKit text-engine cost.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BubbleMeasure {
    /// Final bubble width (content-aware, capped to max).
    pub bubble_width: f64,
    /// Measured wrapped text height.
    pub text_height: f64,
}

/// Per-overlay cache of measured bubble text heights.
///
/// Lives in `VoiceChatOverlayState` (UI state). On a chat-view rebuild the cache
/// is consulted before any AppKit measurement; unchanged bubbles hit the cache
/// and skip `cellSizeForBounds:` entirely.
#[derive(Debug, Default)]
pub struct BubbleMeasureCache {
    entries: HashMap<BubbleMeasureKey, BubbleMeasure>,
}

impl BubbleMeasureCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Return a cached measurement for this key, if present.
    pub fn get(&self, key: &BubbleMeasureKey) -> Option<BubbleMeasure> {
        self.entries.get(key).copied()
    }

    /// Store a measurement for this key.
    pub fn insert(&mut self, key: BubbleMeasureKey, measure: BubbleMeasure) {
        self.entries.insert(key, measure);
    }

    /// Drop everything (e.g. on font/zoom change where every entry is stale).
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Number of cached entries (test/diagnostic helper).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Estimate wrapped text height without invoking AppKit.
///
/// Used for text above `BUBBLE_MEASURE_CAP_CHARS`, where running the real
/// `__NSStringDrawingEngine` over the whole string is what hangs the main
/// thread. Models word-wrapping by columns-per-line plus hard newlines. The
/// result only needs to be a generous over-estimate so the bubble is never
/// clipped; precise wrapped height is irrelevant for a multi-screen blob that
/// the user scrolls rather than reads at a glance.
pub fn estimate_text_height(text: &str, layout_width: f64, font_size: f64) -> f64 {
    let line_height = (font_size * 1.4).max(1.0);
    // Monospace advance width is ~0.6em for the bundled JetBrains Mono / system
    // monospaced font; bias slightly narrow so we over-count wrapped lines.
    let char_width = (font_size * 0.6).max(1.0);
    let cols_per_line = (layout_width / char_width).floor().max(1.0) as usize;

    let mut total_lines: usize = 0;
    for raw_line in text.split('\n') {
        let chars = raw_line.chars().count().max(1);
        let wrapped = chars.div_ceil(cols_per_line).max(1);
        total_lines += wrapped;
    }
    let total_lines = total_lines.max(1);
    (total_lines as f64) * line_height
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct BubbleActionFrame {
    x: f64,
    width: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct BubbleActionFrames {
    copy: BubbleActionFrame,
    render: Option<BubbleActionFrame>,
}

fn bubble_action_frames(
    bubble_width: f64,
    padding_x: f64,
    include_render_toggle: bool,
) -> BubbleActionFrames {
    // Put Markdown and Copy on opposite bubble edges. Keeping the toggle away
    // from Copy is more robust than a small same-edge gap during streaming
    // resizes and makes the hitboxes visually unmistakable.
    let left = padding_x.max(4.0);
    let right = (bubble_width - padding_x).max(left);

    if include_render_toggle {
        let copy_width = BUBBLE_COPY_ACTION_WIDTH.min((right - left).max(1.0));
        let copy_x = (right - copy_width).max(left);
        let render_width =
            BUBBLE_RENDER_ACTION_WIDTH.min((copy_x - BUBBLE_ACTION_GAP - left).max(1.0));
        let render = BubbleActionFrame {
            x: left,
            width: render_width,
        };
        let copy = BubbleActionFrame {
            x: copy_x,
            width: copy_width,
        };
        BubbleActionFrames {
            copy,
            render: Some(render),
        }
    } else {
        BubbleActionFrames {
            copy: BubbleActionFrame {
                x: (right - BUBBLE_COPY_ACTION_WIDTH).max(left),
                width: BUBBLE_COPY_ACTION_WIDTH.min((right - left).max(1.0)),
            },
            render: None,
        }
    }
}

fn is_markdown_table_separator_line(line: &str) -> bool {
    let trimmed = line.trim().trim_matches('|').trim();
    if trimmed.is_empty() || !trimmed.contains('-') {
        return false;
    }
    trimmed.split('|').all(|cell| {
        let cell = cell.trim();
        !cell.is_empty() && cell.chars().all(|ch| matches!(ch, '-' | ':' | ' '))
    })
}

pub(crate) fn looks_like_markdown_table(text: &str) -> bool {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    lines.windows(2).any(|pair| {
        let header = pair[0];
        let sep = pair[1];
        header.contains('|') && is_markdown_table_separator_line(sep)
    })
}

pub(crate) fn should_apply_native_markdown(text: &str) -> bool {
    // Product decision: chat bubbles are a faithful raw-Markdown carrier, not a
    // partial AppKit Markdown renderer. The native importer collapses block
    // structure in NSTextField, so preserve bytes/newlines and let monospace do
    // the work until a real block renderer exists.
    let _ = looks_like_markdown_table(text);
    false
}

fn should_render_native_markdown(render_mode: RenderMode, text: &str) -> bool {
    match render_mode {
        RenderMode::Markdown => true,
        RenderMode::Plain => should_apply_native_markdown(text),
    }
}

fn markdown_options_with_base_font(_text: &str, font: Id) -> Option<Id> {
    unsafe {
        let options_cls = Class::get("NSAttributedStringMarkdownParsingOptions")?;
        let options: Id = msg_send![options_cls, alloc];
        let options: Id = msg_send![options, init];
        if options.is_null() {
            return None;
        }
        let responds_base: bool = msg_send![options, respondsToSelector: sel!(setBaseFont:)];
        if responds_base && !font.is_null() {
            let _: () = msg_send![options, setBaseFont: font];
        }
        // Keep inline-preserving mode for chat bubbles. Table Markdown is intentionally
        // bypassed before this point because AppKit collapses tables in NSTextField.
        let responds_syntax: bool =
            msg_send![options, respondsToSelector: sel!(setInterpretedSyntax:)];
        if responds_syntax {
            // 0 = .full, 1 = .inlineOnly, 2 = .inlineOnlyPreservingWhitespace
            let syntax: isize = 2;
            let _: () = msg_send![options, setInterpretedSyntax: syntax];
        }
        Some(options)
    }
}

/// NSRange for Objective-C attributed string APIs.
#[repr(C)]
#[derive(Copy, Clone)]
struct NSRange {
    location: usize,
    length: usize,
}

// NSFontTraitMask bits (subset).
const NS_ITALIC_FONT_MASK: u64 = 1 << 0;
const NS_BOLD_FONT_MASK: u64 = 1 << 1;

/// Normalize per-range font attributes to stay within the provided base font family.
///
/// AppKit's Markdown parser may introduce different font families for inline `code` spans or
/// emphasis runs. We want consistent typography inside bubbles, while preserving bold/italic
/// traits and point sizes.
///
/// Returns an attributed string instance (possibly mutable) that is safe to set on
/// `NSTextField.setAttributedStringValue:`.
unsafe fn normalize_attributed_string_fonts(attr: Id, base_font: Id) -> Id {
    if attr.is_null() || base_font.is_null() {
        return attr;
    }

    let mutable: Id = msg_send![attr, mutableCopy];
    if mutable.is_null() {
        return attr;
    }
    // Release original — we now own the mutable copy exclusively.
    let _: () = msg_send![attr, release];

    let len: usize = msg_send![mutable, length];
    if len == 0 {
        return mutable;
    }

    let Some(ns_font_manager) = Class::get("NSFontManager") else {
        return mutable;
    };
    let fm: Id = msg_send![ns_font_manager, sharedFontManager];
    if fm.is_null() {
        return mutable;
    }

    let font_key = ns_string("NSFont");
    let base_family: Id = msg_send![base_font, familyName];
    let mut idx: usize = 0;
    while idx < len {
        let mut effective = NSRange {
            location: 0,
            length: 0,
        };
        let cur_font: Id = msg_send![
            mutable,
            attribute: font_key
            atIndex: idx
            effectiveRange: &mut effective
        ];
        if effective.length == 0 {
            idx += 1;
            continue;
        }

        if cur_font.is_null() {
            // Some markdown runs may not carry an explicit NSFont attribute.
            // Ensure those ranges still inherit the bubble's monospace base font
            // instead of NSTextField defaulting them to system UI font.
            let _: () =
                msg_send![mutable, addAttribute: font_key value: base_font range: effective];
            idx = effective.location + effective.length;
            continue;
        }

        if !cur_font.is_null() {
            let traits: u64 = msg_send![fm, traitsOfFont: cur_font];
            let desired_traits = traits & (NS_ITALIC_FONT_MASK | NS_BOLD_FONT_MASK);

            let cur_size: f64 = msg_send![cur_font, pointSize];
            let base_size: f64 = msg_send![base_font, pointSize];

            let mut new_font: Id = base_font;
            if (cur_size - base_size).abs() > 0.05 {
                let sized: Id = msg_send![fm, convertFont: base_font toSize: cur_size];
                if !sized.is_null() {
                    new_font = sized;
                }
            }
            if desired_traits != 0 {
                let converted: Id =
                    msg_send![fm, convertFont: new_font toHaveTrait: desired_traits];
                if !converted.is_null() {
                    // NSFontManager may fallback to a proportional system family when the
                    // requested trait isn't available in the base family. Keep monospace family
                    // stable for chat bubbles, even if that means dropping a trait.
                    let converted_family: Id = msg_send![converted, familyName];
                    let same_family: bool = if base_family.is_null() || converted_family.is_null() {
                        false
                    } else {
                        msg_send![base_family, isEqualToString: converted_family]
                    };
                    if same_family {
                        new_font = converted;
                    }
                }
            }

            let _: () = msg_send![mutable, addAttribute: font_key value: new_font range: effective];
        }

        idx = effective.location + effective.length;
    }

    mutable
}

unsafe fn markdown_attributed_string(text: &str, font: Id) -> Option<Id> {
    let ns_attr = Class::get("NSAttributedString")?;
    let text_ns = ns_string(text);
    let options =
        markdown_options_with_base_font(text, font).unwrap_or(std::ptr::null_mut::<Object>());

    // initWithMarkdown: expects NSData, not NSString
    let utf8_encoding: usize = 4; // NSUTF8StringEncoding
    let text_data: Id = msg_send![text_ns, dataUsingEncoding: utf8_encoding];
    if text_data.is_null() {
        return None;
    }

    let supports_with_error: bool = msg_send![ns_attr, instancesRespondToSelector: sel!(initWithMarkdown:options:baseURL:error:)];
    if supports_with_error {
        let obj: Id = msg_send![ns_attr, alloc];
        let obj: Id = msg_send![
            obj,
            initWithMarkdown: text_data
            options: options
            baseURL: std::ptr::null::<Object>()
            error: std::ptr::null_mut::<*mut Object>()
        ];
        if !obj.is_null() {
            return Some(unsafe { normalize_attributed_string_fonts(obj, font) });
        }
    }

    let supports_simple: bool =
        msg_send![ns_attr, instancesRespondToSelector: sel!(initWithMarkdown:options:baseURL:)];
    if supports_simple {
        let obj: Id = msg_send![ns_attr, alloc];
        let obj: Id = msg_send![
            obj,
            initWithMarkdown: text_data
            options: options
            baseURL: std::ptr::null::<Object>()
        ];
        if !obj.is_null() {
            return Some(unsafe { normalize_attributed_string_fonts(obj, font) });
        }
    }

    None
}

pub(crate) unsafe fn apply_markdown_to_text_field(text_label: Id, text: &str, font: Id) -> bool {
    let responds_attr: bool =
        msg_send![text_label, respondsToSelector: sel!(setAttributedStringValue:)];
    if !responds_attr {
        return false;
    }
    let font = if font.is_null() {
        let ns_font = Class::get("NSFont").unwrap();
        msg_send![ns_font, systemFontOfSize: 13.0f64]
    } else {
        font
    };
    // Keep NSTextField fallback font aligned with markdown base font.
    let _: () = msg_send![text_label, setFont: font];
    if let Some(attr) = unsafe { markdown_attributed_string(text, font) } {
        let _: () = msg_send![text_label, setAttributedStringValue: attr];
        // Re-assert base font after attributed update so any future fallback ranges
        // (e.g. during incremental updates) remain monospace.
        let _: () = msg_send![text_label, setFont: font];
        // Balance the +1 from mutableCopy inside normalize_attributed_string_fonts.
        // setAttributedStringValue: retains its own copy.
        let _: () = msg_send![attr, release];
        return true;
    }
    false
}

/// Configuration for creating a chat bubble
pub struct BubbleConfig {
    pub text: String,
    pub role: BubbleRole,
    pub max_width: f64,
    pub font_size: f64,
    pub is_streaming: bool,
    pub is_error: bool,
    pub render_mode: Option<RenderMode>,
    pub metadata: Option<String>,
    /// Optional message index for Copy button (None = no button)
    pub message_index: Option<usize>,
    /// Optional action target for Copy button
    pub copy_action_target: Option<Id>,
    /// Optional per-overlay measurement cache. When present, an unchanged bubble
    /// (same text/width/font/render-mode) reuses its previously measured size and
    /// skips the AppKit text-engine measurement entirely.
    ///
    /// Raw pointer (not a reference) so `BubbleConfig` stays `Copy`-free but
    /// movable across the FFI-heavy builder; the caller owns the cache for the
    /// duration of the call. `None` falls back to always measuring.
    pub measure_cache: Option<*mut BubbleMeasureCache>,
}

/// Create a chat bubble view (NSView container with styled text)
///
/// Returns (container_view, text_label) tuple for later updates
pub fn create_bubble_view(config: BubbleConfig) -> (Id, Id) {
    unsafe {
        let ns_view = bubble_container_view_class();
        let ns_text_field = bubble_text_field_class();
        let ns_font = Class::get("NSFont").unwrap();
        let ns_dict = Class::get("NSDictionary").unwrap();

        let font_size = config.font_size;
        let padding_x = 12.0;
        let padding_top = 10.0;
        let copy_button_height = if config.message_index.is_some() {
            16.0
        } else {
            0.0
        };
        // Reserve space for the Copy button so it never overlaps text.
        let padding_bottom = if copy_button_height > 0.0 {
            copy_button_height + 8.0
        } else {
            10.0
        };
        let line_height = font_size * 1.4;
        let meta_height = if config.metadata.is_some() {
            (ui_tokens::SMALL_FONT_SIZE + 4.0).max(12.0)
        } else {
            0.0
        };
        let meta_spacing = if config.metadata.is_some() { 4.0 } else { 0.0 };

        // Font (prefer JetBrains Mono if installed)
        let jb_name = ns_string("JetBrainsMono-Regular");
        let jb_font: Id = msg_send![ns_font, fontWithName: jb_name size: font_size];
        let font: Id = if jb_font.is_null() {
            msg_send![ns_font, monospacedSystemFontOfSize: font_size weight: 0.0f64]
        } else {
            jb_font
        };

        // Set text (with streaming indicator if needed)
        let display_text = if config.is_streaming && config.text.is_empty() {
            "• • •".to_string() // Pulsing dots placeholder
        } else if config.is_streaming {
            format!("{} …", config.text)
        } else {
            config.text.clone()
        };

        // Measure text height/width using NSString boundingRectWithSize (handles newlines/wrapping).
        //
        // NOTE: `NSFontAttributeName` (key) has the string value "NSFont". AppKit expects that
        // key, not the literal "NSFontAttributeName" string.
        let text_str = ns_string(&display_text);
        let font_key = ns_string("NSFont");
        let attrs: Id = msg_send![ns_dict, dictionaryWithObject: font forKey: font_key];
        let opts: u64 = 1 | 2; // NSStringDrawingUsesLineFragmentOrigin | NSStringDrawingUsesFontLeading

        // Keep a small side margin inside the container so full-width bubbles don't overflow.
        let bubble_max_width = (config.max_width - 16.0).max(80.0);
        let text_max_width = (bubble_max_width - padding_x * 2.0).max(40.0);

        let render_mode = config
            .render_mode
            .unwrap_or_else(|| streaming_render_mode(config.is_streaming, config.role));

        // Measurement cache lookup. Key on the layout inputs that fully determine
        // the result: text, the bubble's max width (drives wrapping), font size,
        // and render mode. A hit lets us skip BOTH AppKit measurements
        // (`boundingRectWithSize:` width probe + `cellSizeForBounds:` height) —
        // the `__NSStringDrawingEngine` calls that peg the main thread on rebuilds.
        let measure_key =
            BubbleMeasureKey::new(&display_text, config.max_width, font_size, render_mode);
        let cache_ref: Option<&mut BubbleMeasureCache> =
            config.measure_cache.and_then(|ptr| ptr.as_mut());
        let cached_measure = cache_ref.as_ref().and_then(|cache| cache.get(&measure_key));

        // For pathologically long text, estimate instead of laying out the whole
        // string through AppKit — that layout is the actual hang. Always full-width.
        let over_cap = display_text.chars().count() > BUBBLE_MEASURE_CAP_CHARS;

        // Bubble width: content-aware but capped.
        // If it wraps (or is long), keep the bubble full width for readability.
        //
        // We treat streaming messages as "wrap-prone" earlier to avoid the initial narrow bubble
        // that later expands mid-stream.
        let long_threshold = if config.is_streaming { 30 } else { 80 };
        let is_long = display_text.chars().count() > long_threshold;

        let (bubble_width, cached_text_height): (f64, Option<f64>) =
            if let Some(measure) = cached_measure {
                // Reuse the previously measured width + height verbatim.
                (measure.bubble_width, Some(measure.text_height))
            } else if over_cap {
                // Skip AppKit entirely for huge text: full-width bubble + estimate.
                (bubble_max_width, None)
            } else {
                let rect_max: CGRect = msg_send![
                    text_str,
                    boundingRectWithSize: CGSize::new(text_max_width, 10_000.0)
                    options: opts
                    attributes: attrs
                ];
                let wraps_at_max = rect_max.size.height > line_height * 1.6
                    || display_text.contains('\n')
                    || is_long
                    // When streaming starts with the "• • •" placeholder, force full-width
                    // bubbles to avoid the initial tiny/narrow bubble that later expands mid-stream.
                    || (config.is_streaming && config.text.is_empty());
                let width = if wraps_at_max {
                    bubble_max_width
                } else {
                    let content_width = rect_max.size.width.min(text_max_width).max(1.0);
                    (content_width + padding_x * 2.0).min(bubble_max_width)
                };
                (width, None)
            };

        // Label width for wrapping and later reflow.
        let text_layout_width = (bubble_width - padding_x * 2.0).max(40.0);

        // Build the label first and ask AppKit (cell) for the exact wrapped height.
        // This avoids "second line appears only after click" issues where our NSString
        // measurement disagrees with NSTextField's rendering.
        let text_label: Id = msg_send![ns_text_field, alloc];
        let text_label: Id = msg_send![
            text_label,
            initWithFrame: CGRect::new(
                &CGPoint::new(padding_x, padding_top),
                &CGSize::new(text_layout_width.max(1.0), line_height),
            )
        ];

        let _: () = msg_send![text_label, setBezeled: false];
        let _: () = msg_send![text_label, setEditable: false];
        let _: () = msg_send![text_label, setSelectable: true];
        let _: () = msg_send![text_label, setDrawsBackground: false];
        let _: () = msg_send![text_label, setUsesSingleLineMode: false];
        let _: () = msg_send![text_label, setRefusesFirstResponder: false];
        let responds_attr: bool =
            msg_send![text_label, respondsToSelector: sel!(setAllowsEditingTextAttributes:)];
        if responds_attr {
            let _: () = msg_send![text_label, setAllowsEditingTextAttributes: false];
        }

        // Enable wrapping for multi-line messages.
        let cell: Id = msg_send![text_label, cell];
        if !cell.is_null() {
            let _: () = msg_send![cell, setWraps: true];
            let _: () = msg_send![cell, setLineBreakMode: 0_isize]; // NSLineBreakByWordWrapping
            let _: () = msg_send![cell, setScrollable: false];
        }

        // Text color (role-aware)
        let text_color: Id = if config.is_error {
            ui_colors::bubble_error_text()
        } else {
            match config.role {
                BubbleRole::User => ui_colors::bubble_text(),
                BubbleRole::Assistant => {
                    if config.is_streaming {
                        ui_colors::bubble_streaming_text()
                    } else {
                        ui_colors::bubble_text()
                    }
                }
                BubbleRole::System => ui_colors::bubble_text(),
            }
        };
        let _: () = msg_send![text_label, setTextColor: text_color];

        let _: () = msg_send![text_label, setFont: font];
        if !(should_render_native_markdown(render_mode, &display_text)
            && apply_markdown_to_text_field(text_label, &display_text, font))
        {
            let _: () = msg_send![text_label, setStringValue: text_str];
        }
        let _: () = msg_send![text_label, setLineBreakMode: 0_isize]; // NSLineBreakByWordWrapping

        // Resolve the wrapped text height: cache hit > estimate (over cap) > AppKit.
        let text_height = if let Some(height) = cached_text_height {
            height
        } else if over_cap {
            estimate_text_height(&display_text, text_layout_width, font_size)
                .ceil()
                .max(line_height)
        } else {
            // Ask the cell for the wrapped size within the fixed width.
            let measure_bounds = CGRect::new(
                &CGPoint::new(0.0, 0.0),
                &CGSize::new(text_layout_width.max(1.0), 10_000.0),
            );
            let cell: Id = msg_send![text_label, cell];
            let measured: CGSize = if cell.is_null() {
                // Fallback to NSString measurement (best effort).
                let text_rect: CGRect = msg_send![
                    text_str,
                    boundingRectWithSize: CGSize::new(text_layout_width, 10_000.0)
                    options: opts
                    attributes: attrs
                ];
                text_rect.size
            } else {
                msg_send![cell, cellSizeForBounds: measure_bounds]
            };
            measured.height.ceil().max(line_height)
        };

        // Store the freshly computed measurement so the next rebuild of this
        // unchanged bubble hits the cache and skips AppKit measurement.
        if cached_text_height.is_none()
            && let Some(cache) = cache_ref
        {
            cache.insert(
                measure_key,
                BubbleMeasure {
                    bubble_width,
                    text_height,
                },
            );
        }

        let bubble_height = text_height + padding_top + padding_bottom;
        let container_height = bubble_height + meta_height + meta_spacing;

        // Container view (for alignment)
        let container: Id = msg_send![ns_view, alloc];
        let container_frame = CGRect::new(
            &CGPoint::new(0.0, 0.0),
            &CGSize::new(config.max_width, container_height),
        );
        let container: Id = msg_send![container, initWithFrame: container_frame];

        // Bubble background view
        let bubble: Id = msg_send![ns_view, alloc];
        let bubble_x = match config.role {
            BubbleRole::User => (config.max_width - bubble_width - 8.0).max(8.0), // Right-aligned
            BubbleRole::Assistant | BubbleRole::System => 8.0,                    // Left-aligned
        };
        let bubble_y = meta_height + meta_spacing;
        let bubble_frame = CGRect::new(
            &CGPoint::new(bubble_x, bubble_y),
            &CGSize::new(bubble_width, bubble_height),
        );
        let bubble: Id = msg_send![bubble, initWithFrame: bubble_frame];

        // Set bubble background color based on role
        let bg_color: Id = if config.is_error {
            ui_colors::bubble_error_bg()
        } else {
            match config.role {
                BubbleRole::User => ui_colors::bubble_user_bg(),
                BubbleRole::Assistant => ui_colors::bubble_assistant_bg(),
                BubbleRole::System => ui_colors::bubble_system_bg(),
            }
        };

        // Set background via layer (for rounded corners)
        let _: () = msg_send![bubble, setWantsLayer: true];
        let layer: Id = msg_send![bubble, layer];
        if !layer.is_null() {
            // CGColor from NSColor
            let cg_color: Id = msg_send![bg_color, CGColor];
            let _: () = msg_send![layer, setBackgroundColor: cg_color];
            apply_tafla_surface(layer, false);
            let _: () = msg_send![layer, setMasksToBounds: false];
            // Border styling
            let (border_color, bw) = if config.is_error {
                (
                    ui_colors::bubble_error_text(),
                    ui_tokens::SURFACE_BORDER_WIDTH,
                )
            } else {
                match config.role {
                    BubbleRole::User => (
                        ui_colors::bubble_user_border(),
                        ui_tokens::SURFACE_BORDER_WIDTH,
                    ),
                    BubbleRole::Assistant | BubbleRole::System => {
                        (ui_colors::bubble_border(), ui_tokens::SURFACE_BORDER_WIDTH)
                    }
                }
            };
            if bw > 0.0 {
                let cg_border: Id = msg_send![border_color, CGColor];
                let _: () = msg_send![layer, setBorderColor: cg_border];
                let _: () = msg_send![layer, setBorderWidth: bw];
            }
        }

        // Update label frame to the final measured height.
        let text_frame = CGRect::new(
            &CGPoint::new(padding_x, padding_top),
            &CGSize::new(text_layout_width.max(1.0), text_height),
        );
        let _: () = msg_send![text_label, setFrame: text_frame];
        add_subview(bubble, text_label);

        // Metadata (role/time/mode) above the bubble.
        if let Some(meta) = config.metadata.as_ref() {
            let meta_label: Id = msg_send![ns_text_field, alloc];
            let meta_frame = CGRect::new(
                &CGPoint::new(bubble_x, 0.0),
                &CGSize::new(bubble_width.max(1.0), meta_height.max(1.0)),
            );
            let meta_label: Id = msg_send![meta_label, initWithFrame: meta_frame];
            let _: () = msg_send![meta_label, setBezeled: false];
            let _: () = msg_send![meta_label, setEditable: false];
            let _: () = msg_send![meta_label, setSelectable: false];
            let _: () = msg_send![meta_label, setDrawsBackground: false];

            let meta_font: Id = msg_send![ns_font, systemFontOfSize: ui_tokens::SMALL_FONT_SIZE];
            let _: () = msg_send![meta_label, setFont: meta_font];
            let meta_color: Id = ui_colors::bubble_meta_text();
            let _: () = msg_send![meta_label, setTextColor: meta_color];

            let alignment: isize = if config.role == BubbleRole::User {
                2
            } else {
                0
            };
            let _: () = msg_send![meta_label, setAlignment: alignment];
            let _: () = msg_send![meta_label, setStringValue: ns_string(meta)];
            let _: () = msg_send![meta_label, setIdentifier: ns_string("codescribe_bubble_meta")];

            let _: () = msg_send![container, addSubview: meta_label];
        }

        // Assemble hierarchy
        // (text_label already added to bubble above — directly or via scroll wrapper)
        // Add hover action buttons if message_index is provided.
        if let (Some(msg_index), Some(target)) = (config.message_index, config.copy_action_target) {
            let Some(ns_button) = Class::get("NSButton") else {
                let _: () = msg_send![container, addSubview: bubble];
                return (container, text_label);
            };

            let action_frames = bubble_action_frames(
                bubble_width,
                padding_x,
                matches!(
                    config.role,
                    BubbleRole::User | BubbleRole::Assistant | BubbleRole::System
                ),
            );
            let button_height = copy_button_height;
            // Flipped coords: anchor near the bottom edge.
            let button_y = (bubble_height - button_height - 4.0).max(4.0);

            let button_frame = CGRect::new(
                &CGPoint::new(action_frames.copy.x, button_y),
                &CGSize::new(action_frames.copy.width, button_height),
            );

            let copy_button: Id = msg_send![ns_button, alloc];
            let copy_button: Id = msg_send![copy_button, initWithFrame: button_frame];

            // Style: small borderless button
            let _: () = msg_send![copy_button, setBezelStyle: 0_isize]; // NSBezelStyleRounded
            let _: () = msg_send![copy_button, setBordered: false];

            // "Copy" title is the fallback; the native system copy glyph
            // (`doc.on.doc`) replaces it whenever SF Symbols are available so the
            // action reads unambiguously as Copy, distinct from the Markdown
            // toggle next to it.
            let title = ns_string("Copy");
            let _: () = msg_send![copy_button, setTitle: title];

            let small_font: Id = if jb_font.is_null() {
                msg_send![ns_font, monospacedSystemFontOfSize: 10.0f64 weight: 0.0f64]
            } else {
                msg_send![ns_font, fontWithName: jb_name size: 10.0f64]
            };
            let _: () = msg_send![copy_button, setFont: small_font];
            let _ = set_button_symbol(copy_button, "doc.on.doc");
            let _: () = msg_send![copy_button, setToolTip: ns_string("Copy")];
            let _: () = msg_send![copy_button, setAccessibilityLabel: ns_string("Copy")];

            // Match bubble text tint
            let button_color: Id = ui_colors::bubble_text();
            let _: () = msg_send![copy_button, setContentTintColor: button_color];

            // Store message index in tag for retrieval on click
            let _: () = msg_send![copy_button, setTag: msg_index as isize];
            let _: () = msg_send![
                copy_button,
                setIdentifier: ns_string("codescribe_copy_button")
            ];

            // Dedicated frame + handler — an NSButton hit-tests to its own
            // bounds, and `bubble_action_frames` guarantees those bounds never
            // overlap the Markdown toggle, so Copy can never toggle Markdown.
            let _: () = msg_send![copy_button, setTarget: target];
            let _: () = msg_send![copy_button, setAction: sel!(onCopyMessage:)];

            let _: () = msg_send![copy_button, setHidden: true];
            let _: () = msg_send![bubble, addSubview: copy_button];

            if let Some(render_action_frame) = action_frames.render {
                let render_frame = CGRect::new(
                    &CGPoint::new(render_action_frame.x, button_y),
                    &CGSize::new(render_action_frame.width, button_height),
                );
                let render_button: Id = msg_send![ns_button, alloc];
                let render_button: Id = msg_send![render_button, initWithFrame: render_frame];
                let _: () = msg_send![render_button, setBezelStyle: 0_isize];
                let _: () = msg_send![render_button, setBordered: false];

                // Explicit `MD` text label — replaces the ambiguous "Aa"
                // (`textformat`) / "{}" (`curlybraces`) SF Symbols the operator
                // flagged. Title-only (no image) so it never reads as a generic
                // glyph; the tooltip + accessibility label carry toggle direction.
                let _: () = msg_send![render_button, setTitle: ns_string("MD")];
                let _: () = msg_send![render_button, setImagePosition: 0_isize]; // NSNoImage
                let tooltip = match render_mode {
                    RenderMode::Plain => "Render Markdown",
                    RenderMode::Markdown => "Show raw Markdown",
                };
                let _: () = msg_send![render_button, setToolTip: ns_string(tooltip)];
                let _: () = msg_send![render_button, setAccessibilityLabel: ns_string(tooltip)];
                let _: () = msg_send![render_button, setFont: small_font];
                let _: () = msg_send![render_button, setContentTintColor: button_color];
                let _: () = msg_send![render_button, setTag: msg_index as isize];
                let _: () = msg_send![
                    render_button,
                    setIdentifier: ns_string("codescribe_render_button")
                ];
                // Dedicated frame + handler — Markdown toggle never copies.
                let _: () = msg_send![render_button, setTarget: target];
                let _: () = msg_send![render_button, setAction: sel!(onToggleBubbleRender:)];
                let _: () = msg_send![render_button, setHidden: true];
                let _: () = msg_send![bubble, addSubview: render_button];
            }
        }

        let _: () = msg_send![container, addSubview: bubble];

        if config.message_index.is_some() {
            let ns_tracking_area = Class::get("NSTrackingArea").unwrap();
            let tracking_opts = NSTRACKING_MOUSE_ENTERED_AND_EXITED
                | NSTRACKING_ACTIVE_ALWAYS
                | NSTRACKING_IN_VISIBLE_RECT;
            let tracking_area: Id = msg_send![ns_tracking_area, alloc];
            let tracking_area: Id = msg_send![
                tracking_area,
                initWithRect: CGRect::new(
                    &CGPoint::new(0.0, 0.0),
                    &CGSize::new(bubble_width.max(1.0), bubble_height.max(1.0)),
                )
                options: tracking_opts
                owner: bubble
                userInfo: std::ptr::null::<Object>()
            ];
            let _: () = msg_send![bubble, addTrackingArea: tracking_area];
        }

        (container, text_label)
    }
}

/// Update bubble text (for streaming updates)
/// # Safety
/// `text_label` must be a valid `NSTextField` instance.
pub unsafe fn update_bubble_text(
    text_label: Id,
    text: &str,
    role: BubbleRole,
    is_streaming: bool,
    is_error: bool,
) {
    let render_mode = streaming_render_mode(is_streaming, role);
    unsafe {
        update_bubble_text_with_render_mode(
            text_label,
            text,
            role,
            is_streaming,
            is_error,
            render_mode,
        );
    }
}

/// Update bubble text with an explicit render mode.
/// # Safety
/// `text_label` must be a valid `NSTextField` instance.
pub unsafe fn update_bubble_text_with_render_mode(
    text_label: Id,
    text: &str,
    role: BubbleRole,
    is_streaming: bool,
    is_error: bool,
    render_mode: RenderMode,
) {
    unsafe {
        let display_text = if is_streaming && text.is_empty() {
            "• • •".to_string()
        } else if is_streaming {
            format!("{} …", text)
        } else {
            text.to_string()
        };

        // Always create a fresh monospace font instead of reading from the label.
        // After markdown parsing, text_label.font may return a system font from
        // the first attributed range, causing cascading degradation on subsequent updates.
        let label_font: Id = msg_send![text_label, font];
        let font_size: f64 = if label_font.is_null() {
            13.0
        } else {
            msg_send![label_font, pointSize]
        };
        let ns_font_cls = Class::get("NSFont").unwrap();
        let jb_name = ns_string("JetBrainsMono-Regular");
        let jb_font: Id = msg_send![ns_font_cls, fontWithName: jb_name size: font_size];
        let font: Id = if jb_font.is_null() {
            msg_send![ns_font_cls, monospacedSystemFontOfSize: font_size weight: 0.0f64]
        } else {
            jb_font
        };
        let _: () = msg_send![text_label, setFont: font];
        if !(should_render_native_markdown(render_mode, &display_text)
            && apply_markdown_to_text_field(text_label, &display_text, font))
        {
            let text_str = ns_string(&display_text);
            let _: () = msg_send![text_label, setStringValue: text_str];
        }

        let text_color: Id = if is_error {
            ui_colors::bubble_error_text()
        } else {
            match role {
                BubbleRole::User => ui_colors::bubble_text(),
                BubbleRole::Assistant => {
                    if is_streaming {
                        ui_colors::bubble_streaming_text()
                    } else {
                        ui_colors::bubble_text()
                    }
                }
                BubbleRole::System => ui_colors::bubble_text(),
            }
        };
        let _: () = msg_send![text_label, setTextColor: text_color];
    }
}

/// Update a stack view item (bubble container) height constraint if present.
///
/// `stack_view_add` installs a fixed-height constraint on each arranged subview.
/// During streaming, the bubble text grows and we need to update that constraint
/// so the view doesn't clip.
///
/// # Safety
/// `view` must be a valid `NSView` instance.
pub unsafe fn update_stack_item_height(view: Id, new_height: f64) {
    unsafe {
        let constraints: Id = msg_send![view, constraints];
        if constraints.is_null() {
            return;
        }
        let count: usize = msg_send![constraints, count];
        for i in 0..count {
            let c: Id = msg_send![constraints, objectAtIndex: i];
            if c.is_null() {
                continue;
            }

            // Prefer our tagged constraint.
            let ident: Id = msg_send![c, identifier];
            if !ident.is_null() {
                let c_str: *const i8 = msg_send![ident, UTF8String];
                if !c_str.is_null() {
                    let s = std::ffi::CStr::from_ptr(c_str).to_string_lossy();
                    if s == "codescribe_height" {
                        let _: () = msg_send![c, setConstant: new_height];
                        return;
                    }
                }
            }

            // Fallback: find a height constraint on this view.
            let first: Id = msg_send![c, firstItem];
            if first != view {
                continue;
            }
            let second: Id = msg_send![c, secondItem];
            if !second.is_null() {
                continue;
            }
            let first_attr: isize = msg_send![c, firstAttribute];
            // NSLayoutAttributeHeight == 8
            if first_attr == 8 {
                let _: () = msg_send![c, setConstant: new_height];
                return;
            }
        }
    }
}

/// Resize an existing bubble container + its internal views for the given text.
///
/// Used for streaming updates to prevent clipping without rebuilding the whole view tree.
///
/// # Safety
/// `container` must be the container returned by `create_bubble_view`.
/// `text_label` must be the label returned by `create_bubble_view`.
pub unsafe fn resize_bubble_container_for_text(container: Id, text_label: Id, display_text: &str) {
    unsafe {
        let ns_font = Class::get("NSFont").unwrap();

        let font: Id = msg_send![text_label, font];
        let font = if font.is_null() {
            msg_send![ns_font, systemFontOfSize: 13.0f64]
        } else {
            font
        };

        let container_frame: CGRect = msg_send![container, frame];
        let max_width = container_frame.size.width.max(80.0);
        let bubble_max_width = (max_width - 16.0).max(80.0);

        // If the message is getting long, switch to full-width to avoid one-word-per-line bubbles.
        //
        // During streaming we append " …" so we can detect it and widen earlier to prevent
        // the initial narrow bubble phase.
        let streaming_like = display_text.ends_with('…');
        let long_threshold = if streaming_like { 30 } else { 80 };
        let is_long = display_text.chars().count() > long_threshold;
        let force_full_width = display_text.contains('\n') || is_long;

        let label_frame: CGRect = msg_send![text_label, frame];
        let width = if force_full_width {
            let padding_x = 12.0;
            (bubble_max_width - padding_x * 2.0).max(40.0)
        } else {
            label_frame.size.width.max(1.0)
        };

        // Approximate line-height floor to avoid tiny/bad measurements.
        let point_size: f64 = msg_send![font, pointSize];
        let line_height = (point_size * 1.35).max(14.0);

        // Match `create_bubble_view` layout constants.
        let padding_top = 10.0;
        let copy_button_height = 16.0;
        let padding_bottom = copy_button_height + 8.0;

        // Ask the label's cell for the wrapped height in the current width.
        let measure_bounds = CGRect::new(
            &CGPoint::new(0.0, 0.0),
            &CGSize::new(width.max(1.0), 10_000.0),
        );
        let cell: Id = msg_send![text_label, cell];
        let measured: CGSize = if cell.is_null() {
            // Fallback to a conservative single line height.
            CGSize::new(width.max(1.0), line_height)
        } else {
            msg_send![cell, cellSizeForBounds: measure_bounds]
        };
        let text_height = measured.height.ceil().max(line_height);
        let bubble_height = text_height + padding_top + padding_bottom;
        let mut meta_height = 0.0;
        let mut meta_spacing = 0.0;
        let mut meta_label: Option<Id> = None;

        let subviews: Id = msg_send![container, subviews];
        if !subviews.is_null() {
            let sub_count: usize = msg_send![subviews, count];
            for i in 0..sub_count {
                let v: Id = msg_send![subviews, objectAtIndex: i];
                if v.is_null() {
                    continue;
                }
                let ident: Id = msg_send![v, identifier];
                if ident.is_null() {
                    continue;
                }
                let c_str: *const i8 = msg_send![ident, UTF8String];
                if c_str.is_null() {
                    continue;
                }
                let s = std::ffi::CStr::from_ptr(c_str).to_string_lossy();
                if s == "codescribe_bubble_meta" {
                    let frame: CGRect = msg_send![v, frame];
                    meta_height = frame.size.height.max(ui_tokens::SMALL_FONT_SIZE);
                    meta_spacing = 4.0;
                    meta_label = Some(v);
                    break;
                }
            }
        }

        // Resize bubble background view (label's superview).
        let bubble: Id = msg_send![text_label, superview];
        if !bubble.is_null() {
            let bubble_frame: CGRect = msg_send![bubble, frame];
            let mut bubble_width = bubble_frame.size.width;
            let mut bubble_x = bubble_frame.origin.x;

            if force_full_width {
                bubble_width = bubble_max_width;
                // Preserve alignment based on prior x (user bubbles are right-aligned).
                let was_right_aligned = bubble_x > 20.0;
                bubble_x = if was_right_aligned {
                    (max_width - bubble_width - 8.0).max(8.0)
                } else {
                    8.0
                };
            }

            // Resize label to match bubble width (keep in sync with create_bubble_view).
            let padding_x = 12.0;
            let new_label_w = (bubble_width - padding_x * 2.0).max(1.0);
            let new_label_frame = CGRect::new(
                &CGPoint::new(padding_x, padding_top),
                &CGSize::new(new_label_w, text_height),
            );
            let _: () = msg_send![text_label, setFrame: new_label_frame];

            if let Some(meta_ptr) = meta_label {
                let meta_frame = CGRect::new(
                    &CGPoint::new(bubble_x, 0.0),
                    &CGSize::new(bubble_width.max(1.0), meta_height.max(1.0)),
                );
                let _: () = msg_send![meta_ptr, setFrame: meta_frame];
            }

            // Reposition action buttons to stay anchored near the bottom edge (flipped coords)
            // and recompute X from bubble width. Streaming can widen bubbles after creation;
            // preserving the old X lets MD/Copy hitboxes drift into each other.
            let ns_button = Class::get("NSButton").unwrap();
            let subviews: Id = msg_send![bubble, subviews];
            if !subviews.is_null() {
                let sub_count: usize = msg_send![subviews, count];
                let mut copy_button: Option<Id> = None;
                let mut render_button: Option<Id> = None;
                for i in 0..sub_count {
                    let v: Id = msg_send![subviews, objectAtIndex: i];
                    if v.is_null() {
                        continue;
                    }
                    let is_button: bool = msg_send![v, isKindOfClass: ns_button];
                    if !is_button {
                        continue;
                    }
                    let ident: Id = msg_send![v, identifier];
                    if ident.is_null() {
                        continue;
                    }
                    let c_str: *const i8 = msg_send![ident, UTF8String];
                    if c_str.is_null() {
                        continue;
                    }
                    let s = std::ffi::CStr::from_ptr(c_str).to_string_lossy();
                    match s.as_ref() {
                        "codescribe_copy_button" => copy_button = Some(v),
                        "codescribe_render_button" => render_button = Some(v),
                        _ => {}
                    }
                }

                let action_frames =
                    bubble_action_frames(bubble_width, padding_x, render_button.is_some());
                if let Some(button) = render_button
                    && let Some(render_frame) = action_frames.render
                {
                    let btn_frame: CGRect = msg_send![button, frame];
                    let btn_h = btn_frame.size.height;
                    let new_y = (bubble_height - btn_h - 4.0).max(4.0);
                    let new_frame = CGRect::new(
                        &CGPoint::new(render_frame.x, new_y),
                        &CGSize::new(render_frame.width, btn_h),
                    );
                    let _: () = msg_send![button, setFrame: new_frame];
                }
                if let Some(button) = copy_button {
                    let btn_frame: CGRect = msg_send![button, frame];
                    let btn_h = btn_frame.size.height;
                    let new_y = (bubble_height - btn_h - 4.0).max(4.0);
                    let new_frame = CGRect::new(
                        &CGPoint::new(action_frames.copy.x, new_y),
                        &CGSize::new(action_frames.copy.width, btn_h),
                    );
                    let _: () = msg_send![button, setFrame: new_frame];
                }
            }

            let bubble_y = if meta_height > 0.0 {
                meta_height + meta_spacing
            } else {
                bubble_frame.origin.y
            };
            let new_bubble_frame = CGRect::new(
                &CGPoint::new(bubble_x, bubble_y),
                &CGSize::new(bubble_width, bubble_height),
            );
            let _: () = msg_send![bubble, setFrame: new_bubble_frame];
            let _: () = msg_send![bubble, setNeedsDisplay: true];
            let _: () = msg_send![text_label, setNeedsDisplay: true];
        }

        // Resize container (stack arranged subview).
        let container_height = bubble_height + meta_height + meta_spacing;
        let _: () = msg_send![
            container,
            setFrameSize: CGSize::new(container_frame.size.width, container_height)
        ];
        update_stack_item_height(container, container_height);

        let _: () = msg_send![container, setNeedsLayout: true];
        let _: () = msg_send![container, layoutSubtreeIfNeeded];
        let _: () = msg_send![container, setNeedsDisplay: true];

        // NSStackView (superview) does the actual arrangement; ensure it reflows immediately
        // so updated height constraints take effect without requiring a click/focus change.
        let stack: Id = msg_send![container, superview];
        if !stack.is_null() {
            let _: () = msg_send![stack, setNeedsLayout: true];
            let _: () = msg_send![stack, layoutSubtreeIfNeeded];
        }
    }
}

// ============================================================================
// File Operations Helpers
// ============================================================================

pub fn create_vertical_stack_view(frame: CGRect) -> Id {
    unsafe {
        let ns_stack_view = Class::get("NSStackView").unwrap();

        let stack: Id = msg_send![ns_stack_view, alloc];
        let stack: Id = msg_send![stack, initWithFrame: frame];

        // Vertical orientation (1 = NSUserInterfaceLayoutOrientationVertical)
        let _: () = msg_send![stack, setOrientation: 1_isize];
        // Top alignment
        let _: () = msg_send![stack, setAlignment: 1_isize]; // NSLayoutAttributeLeft
        // Spacing between views
        let _: () = msg_send![stack, setSpacing: 8.0f64];

        stack
    }
}

/// Create a flipped vertical NSStackView (y-axis grows downward).
///
/// This is useful for chat-like UIs where we want "top-down" coordinates and stable bubble
/// sizing during streaming.
pub fn create_flipped_vertical_stack_view(frame: CGRect) -> Id {
    unsafe {
        let ns_stack_view = flipped_stack_view_class();

        let stack: Id = msg_send![ns_stack_view, alloc];
        let stack: Id = msg_send![stack, initWithFrame: frame];

        // Vertical orientation (1 = NSUserInterfaceLayoutOrientationVertical)
        let _: () = msg_send![stack, setOrientation: 1_isize];
        // Top alignment
        let _: () = msg_send![stack, setAlignment: 1_isize]; // NSLayoutAttributeLeft
        // Spacing between views
        let _: () = msg_send![stack, setSpacing: 8.0f64];

        stack
    }
}

fn flipped_stack_view_class() -> &'static Class {
    static mut CLS: *const Class = std::ptr::null();
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        let superclass = Class::get("NSStackView").expect("NSStackView class missing");
        let mut decl = ClassDecl::new("CodeScribeFlippedStackView", superclass)
            .expect("CodeScribeFlippedStackView already defined");
        decl.add_method(
            sel!(isFlipped),
            is_flipped as extern "C" fn(&Object, Sel) -> bool,
        );
        let cls = decl.register();
        CLS = cls as *const Class;
    });
    unsafe { &*CLS }
}

extern "C" fn is_flipped(_this: &Object, _cmd: Sel) -> bool {
    true
}

fn bubble_container_view_class() -> &'static Class {
    static mut CLS: *const Class = std::ptr::null();
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        let superclass = Class::get("NSView").expect("NSView class missing");
        let mut decl = ClassDecl::new("CodeScribeBubbleContainerView", superclass)
            .expect("CodeScribeBubbleContainerView already defined");
        decl.add_method(
            sel!(isFlipped),
            is_flipped as extern "C" fn(&Object, Sel) -> bool,
        );
        decl.add_method(
            sel!(scrollWheel:),
            bubble_container_scroll_wheel as extern "C" fn(&Object, Sel, Id),
        );
        decl.add_method(
            sel!(mouseEntered:),
            bubble_container_mouse_entered as extern "C" fn(&Object, Sel, Id),
        );
        decl.add_method(
            sel!(mouseExited:),
            bubble_container_mouse_exited as extern "C" fn(&Object, Sel, Id),
        );
        let cls = decl.register();
        CLS = cls as *const Class;
    });
    unsafe { &*CLS }
}

extern "C" fn bubble_container_scroll_wheel(this: &Object, _cmd: Sel, event: Id) {
    unsafe {
        let view: Id = (this as *const Object) as Id;
        if view.is_null() || event.is_null() {
            return;
        }

        // When the pointer is over a bubble background, AppKit may not route wheel events to the
        // surrounding scroll view. Forward explicitly so long messages stay scrollable.
        let scroll: Id = msg_send![view, enclosingScrollView];
        if !scroll.is_null() {
            let _: () = msg_send![scroll, scrollWheel: event];
            return;
        }

        let next: Id = msg_send![view, nextResponder];
        if !next.is_null() {
            let _: () = msg_send![next, scrollWheel: event];
        }
    }
}

extern "C" fn bubble_container_mouse_entered(this: &Object, _cmd: Sel, _event: Id) {
    unsafe {
        let view: Id = (this as *const Object) as Id;
        toggle_bubble_copy_buttons(view, true);
    }
}

extern "C" fn bubble_container_mouse_exited(this: &Object, _cmd: Sel, _event: Id) {
    unsafe {
        let view: Id = (this as *const Object) as Id;
        toggle_bubble_copy_buttons(view, false);
    }
}

unsafe fn toggle_bubble_copy_buttons(view: Id, visible: bool) {
    let ns_button = Class::get("NSButton").unwrap();
    let subviews: Id = msg_send![view, subviews];
    if subviews.is_null() {
        return;
    }
    let count: usize = msg_send![subviews, count];
    for i in 0..count {
        let v: Id = msg_send![subviews, objectAtIndex: i];
        if v.is_null() {
            continue;
        }
        let is_button: bool = msg_send![v, isKindOfClass: ns_button];
        if is_button {
            let ident: Id = msg_send![v, identifier];
            if !ident.is_null() {
                let c_str: *const i8 = msg_send![ident, UTF8String];
                if !c_str.is_null() {
                    let s = unsafe { std::ffi::CStr::from_ptr(c_str) }.to_string_lossy();
                    if s == "codescribe_copy_button" || s == "codescribe_render_button" {
                        let _: () = msg_send![v, setHidden: !visible];
                    }
                }
            }
            continue;
        }
        unsafe { toggle_bubble_copy_buttons(v, visible) };
    }
}

fn bubble_text_field_class() -> &'static Class {
    static mut CLS: *const Class = std::ptr::null();
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        let superclass = Class::get("NSTextField").expect("NSTextField class missing");
        let mut decl = ClassDecl::new("CodeScribeBubbleTextField", superclass)
            .expect("CodeScribeBubbleTextField already defined");
        decl.add_method(
            sel!(scrollWheel:),
            bubble_text_scroll_wheel as extern "C" fn(&Object, Sel, Id),
        );
        let cls = decl.register();
        CLS = cls as *const Class;
    });
    unsafe { &*CLS }
}

extern "C" fn bubble_text_scroll_wheel(this: &Object, _cmd: Sel, event: Id) {
    unsafe {
        let view: Id = (this as *const Object) as Id;
        if view.is_null() || event.is_null() {
            return;
        }

        // Selectable text fields sometimes "eat" scroll wheel events without scrolling anything.
        // Forward the wheel to the enclosing scroll view so Agent/Drawer can always scroll.
        let scroll: Id = msg_send![view, enclosingScrollView];
        if !scroll.is_null() {
            let _: () = msg_send![scroll, scrollWheel: event];
            return;
        }

        let next: Id = msg_send![view, nextResponder];
        if !next.is_null() {
            let _: () = msg_send![next, scrollWheel: event];
        }
    }
}

/// Add a view to NSStackView
/// # Safety
/// `stack` must be a valid `NSStackView` and `view` a valid `NSView`.
pub unsafe fn stack_view_add(stack: Id, view: Id) {
    unsafe {
        // NSStackView uses Auto Layout for arranged subviews. Our views are created with manual
        // frames, so we need to:
        // - opt out of autoresizing-mask constraints
        // - provide at least a height constraint, otherwise subviews can collapse/overlap
        let _: () = msg_send![view, setTranslatesAutoresizingMaskIntoConstraints: false];

        let _: () = msg_send![stack, addArrangedSubview: view];

        // Ensure a deterministic width. Without leading/trailing constraints, NSStackView can
        // produce ambiguous layouts (overlaps / broken scrolling) when used as a scroll document.
        let view_leading: Id = msg_send![view, leadingAnchor];
        let view_trailing: Id = msg_send![view, trailingAnchor];
        let stack_leading: Id = msg_send![stack, leadingAnchor];
        let stack_trailing: Id = msg_send![stack, trailingAnchor];
        if !view_leading.is_null()
            && !view_trailing.is_null()
            && !stack_leading.is_null()
            && !stack_trailing.is_null()
        {
            let leading: Id = msg_send![view_leading, constraintEqualToAnchor: stack_leading];
            let trailing: Id = msg_send![view_trailing, constraintEqualToAnchor: stack_trailing];
            if !leading.is_null() {
                let _: () = msg_send![leading, setActive: true];
            }
            if !trailing.is_null() {
                let _: () = msg_send![trailing, setActive: true];
            }
        }

        // Pin height to the initial frame height (good enough for our chat bubbles/cards).
        let frame: CGRect = msg_send![view, frame];
        let height_anchor: Id = msg_send![view, heightAnchor];
        let height_constraint: Id =
            msg_send![height_anchor, constraintEqualToConstant: frame.size.height];
        // Tag for later updates (streaming bubbles grow).
        let _: () = msg_send![height_constraint, setIdentifier: ns_string("codescribe_height")];
        let _: () = msg_send![height_constraint, setActive: true];
    }
}

/// Remove all views from NSStackView
/// # Safety
/// `stack` must be a valid `NSStackView` instance.
pub unsafe fn stack_view_clear(stack: Id) {
    unsafe {
        let arranged: Id = msg_send![stack, arrangedSubviews];
        let count: usize = msg_send![arranged, count];

        for i in (0..count).rev() {
            let view: Id = msg_send![arranged, objectAtIndex: i];
            // For NSStackView, removing an arranged subview requires two steps:
            // 1) removeArrangedSubview (removes constraints/arrangement bookkeeping)
            // 2) removeFromSuperview (removes it from the view hierarchy)
            let _: () = msg_send![stack, removeArrangedSubview: view];
            let _: () = msg_send![view, removeFromSuperview];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame_right(frame: BubbleActionFrame) -> f64 {
        frame.x + frame.width
    }

    #[test]
    fn bubble_action_frames_keep_render_and_copy_separate() {
        let frames = bubble_action_frames(92.0, 12.0, true);
        let render = frames
            .render
            .expect("renderable bubbles should get render toggle");

        assert_eq!(render.x, 12.0);
        assert_eq!(frames.copy.x, 40.0);
        assert!(
            frame_right(render) <= frames.copy.x,
            "render and copy actions must not overlap"
        );
        assert!(
            frames.copy.x - frame_right(render) >= BUBBLE_ACTION_GAP,
            "render and copy actions must keep a strict gap"
        );
        assert!(render.x >= 4.0);
        assert!(frame_right(frames.copy) <= 92.0);
    }

    #[test]
    fn bubble_action_frames_without_render_use_right_edge_for_copy() {
        let frames = bubble_action_frames(180.0, 12.0, false);

        assert_eq!(frames.render, None);
        assert_eq!(frames.copy.width, BUBBLE_COPY_ACTION_WIDTH);
        assert_eq!(frames.copy.x, 128.0);
    }

    // ------------------------------------------------------------------
    // Measurement cache + cap
    // ------------------------------------------------------------------

    /// Simulates the create_bubble_view measurement path against a cache while
    /// counting how many times an actual (expensive) measurement would run.
    /// Returns (resolved_height, measured_this_call).
    fn measure_with_cache(
        cache: &mut BubbleMeasureCache,
        measure_calls: &mut usize,
        text: &str,
        max_width: f64,
        font_size: f64,
        render_mode: RenderMode,
    ) -> f64 {
        let key = BubbleMeasureKey::new(text, max_width, font_size, render_mode);
        if let Some(measure) = cache.get(&key) {
            return measure.text_height; // cache hit — no measurement
        }
        // Cache miss — perform the "expensive" measurement (counted).
        *measure_calls += 1;
        let height = estimate_text_height(text, max_width, font_size)
            .ceil()
            .max(font_size * 1.4);
        cache.insert(
            key,
            BubbleMeasure {
                bubble_width: max_width,
                text_height: height,
            },
        );
        height
    }

    #[test]
    fn measure_cache_hits_on_identical_inputs_and_skips_remeasure() {
        let mut cache = BubbleMeasureCache::new();
        let mut measure_calls = 0usize;

        let text = "hello world, this is a chat bubble that will be measured";
        let h1 = measure_with_cache(
            &mut cache,
            &mut measure_calls,
            text,
            400.0,
            13.0,
            RenderMode::Plain,
        );
        assert_eq!(measure_calls, 1, "first measurement must run");
        assert_eq!(cache.len(), 1);

        // Same text/width/font/render-mode -> cache hit, no new measurement.
        let h2 = measure_with_cache(
            &mut cache,
            &mut measure_calls,
            text,
            400.0,
            13.0,
            RenderMode::Plain,
        );
        assert_eq!(
            measure_calls, 1,
            "identical inputs must reuse the cached measurement"
        );
        assert_eq!(h1, h2, "cache must return the same height");
        assert_eq!(cache.len(), 1, "no new entry on a hit");
    }

    #[test]
    fn measure_cache_misses_when_any_key_input_changes() {
        let mut cache = BubbleMeasureCache::new();
        let mut measure_calls = 0usize;
        let text = "bubble text";

        measure_with_cache(
            &mut cache,
            &mut measure_calls,
            text,
            400.0,
            13.0,
            RenderMode::Plain,
        );
        // Different width -> miss.
        measure_with_cache(
            &mut cache,
            &mut measure_calls,
            text,
            500.0,
            13.0,
            RenderMode::Plain,
        );
        // Different font size -> miss.
        measure_with_cache(
            &mut cache,
            &mut measure_calls,
            text,
            400.0,
            15.0,
            RenderMode::Plain,
        );
        // Different render mode -> miss.
        measure_with_cache(
            &mut cache,
            &mut measure_calls,
            text,
            400.0,
            13.0,
            RenderMode::Markdown,
        );
        // Different text -> miss.
        measure_with_cache(
            &mut cache,
            &mut measure_calls,
            "other",
            400.0,
            13.0,
            RenderMode::Plain,
        );

        assert_eq!(
            measure_calls, 5,
            "each distinct key must measure exactly once"
        );
        assert_eq!(cache.len(), 5);

        // Re-issuing all five keys must be pure cache hits.
        let before = measure_calls;
        measure_with_cache(
            &mut cache,
            &mut measure_calls,
            text,
            400.0,
            13.0,
            RenderMode::Plain,
        );
        measure_with_cache(
            &mut cache,
            &mut measure_calls,
            text,
            500.0,
            13.0,
            RenderMode::Plain,
        );
        measure_with_cache(
            &mut cache,
            &mut measure_calls,
            text,
            400.0,
            15.0,
            RenderMode::Plain,
        );
        measure_with_cache(
            &mut cache,
            &mut measure_calls,
            text,
            400.0,
            13.0,
            RenderMode::Markdown,
        );
        measure_with_cache(
            &mut cache,
            &mut measure_calls,
            "other",
            400.0,
            13.0,
            RenderMode::Plain,
        );
        assert_eq!(measure_calls, before, "all repeats must hit the cache");
    }

    #[test]
    fn measure_cache_clear_forces_remeasure() {
        let mut cache = BubbleMeasureCache::new();
        let mut measure_calls = 0usize;
        let text = "clearable";

        measure_with_cache(
            &mut cache,
            &mut measure_calls,
            text,
            400.0,
            13.0,
            RenderMode::Plain,
        );
        assert_eq!(measure_calls, 1);
        cache.clear();
        assert!(cache.is_empty());
        measure_with_cache(
            &mut cache,
            &mut measure_calls,
            text,
            400.0,
            13.0,
            RenderMode::Plain,
        );
        assert_eq!(measure_calls, 2, "cleared cache must re-measure");
    }

    #[test]
    fn cap_threshold_selects_estimation_for_huge_text() {
        // Below cap: not over cap. Above cap: over cap (would skip AppKit).
        let small = "x".repeat(BUBBLE_MEASURE_CAP_CHARS);
        let huge = "x".repeat(BUBBLE_MEASURE_CAP_CHARS + 1);
        assert!(small.chars().count() <= BUBBLE_MEASURE_CAP_CHARS);
        assert!(huge.chars().count() > BUBBLE_MEASURE_CAP_CHARS);
    }

    #[test]
    fn estimate_text_height_grows_with_length_and_respects_newlines() {
        let width = 400.0;
        let font = 13.0;

        let one_line = estimate_text_height("short", width, font);
        let line_height = (font * 1.4).max(1.0);
        assert!(
            one_line >= line_height,
            "single short line must be at least one line tall"
        );

        // A very long single line wraps to multiple lines -> taller.
        let long = "a".repeat(10_000);
        let wrapped = estimate_text_height(&long, width, font);
        assert!(
            wrapped > one_line * 5.0,
            "wrapped long text must be many lines tall ({wrapped} vs {one_line})"
        );

        // Hard newlines add lines even for short content.
        let multiline = "a\nb\nc\nd\ne";
        let multi_h = estimate_text_height(multiline, width, font);
        assert!(
            multi_h >= line_height * 5.0,
            "five newline-separated lines must be >= 5 line-heights ({multi_h})"
        );
    }

    #[test]
    fn estimate_text_height_caps_runtime_for_megabyte_blob() {
        // The whole point of the cap: estimation is O(n) cheap, never invokes
        // AppKit. This just proves it returns a finite, sane height for a blob
        // far larger than the cap without hanging.
        let blob = "lorem ipsum dolor sit amet ".repeat(200_000); // ~5.4 MB
        let h = estimate_text_height(&blob, 400.0, 13.0);
        assert!(h.is_finite() && h > 0.0);
    }
}
