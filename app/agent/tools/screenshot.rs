use std::ffi::c_void;
use std::ptr;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use codescribe_core::agent::{AgentAssetStore, ToolDefinition, ToolRegistry, ToolResultContent};

use crate::os::permissions::{self, PermissionStatus};
use core_foundation::base::{CFRelease, CFType, TCFType, kCFAllocatorDefault};
use core_foundation::data::{CFData, CFDataCreateMutable, CFDataRef, CFMutableDataRef};
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::number::CFNumber;
use core_foundation::string::{CFString, CFStringRef};
use core_graphics::base::{kCGBitmapByteOrder32Big, kCGImageAlphaLast, kCGRenderingIntentDefault};
use core_graphics::color_space::CGColorSpace;
use core_graphics::data_provider::CGDataProvider;
use core_graphics::display::CGDisplay;
use core_graphics::geometry::CG_ZERO_RECT;
use core_graphics::image::CGImage;
use core_graphics::window::{
    CGWindowID, kCGNullWindowID, kCGWindowImageBoundsIgnoreFraming, kCGWindowImageDefault,
    kCGWindowLayer, kCGWindowListExcludeDesktopElements, kCGWindowListOptionIncludingWindow,
    kCGWindowListOptionOnScreenOnly, kCGWindowNumber,
};
use image::RgbaImage;
use image::imageops::FilterType;
use serde_json::{Value, json};

const MAX_SCREENSHOT_EDGE: u32 = 1568;
const MAX_SCREENSHOT_BYTES: usize = 5 * 1024 * 1024;

#[derive(Clone, Copy)]
enum CaptureRegion {
    Full,
    Frontmost,
}

pub fn register(registry: &mut ToolRegistry) {
    registry
        .register(
            screenshot_definition(),
            Box::new(|input| Box::pin(handle_take_screenshot(input))),
        )
        .expect("register take_screenshot tool");
}

fn screenshot_definition() -> ToolDefinition {
    ToolDefinition {
        name: "take_screenshot".to_string(),
        description:
        "Capture a screenshot of the screen or a specific region. Returns a saved image asset reference."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "region": {
                    "type": "string",
                    "enum": ["full", "frontmost"],
                    "default": "full",
                    "description": "What to capture: 'full' for entire screen, 'frontmost' for the frontmost window"
                }
            }
        }),
    }
}

async fn handle_take_screenshot(input: Value) -> Vec<ToolResultContent> {
    match capture_and_encode(input) {
        Ok(png_bytes) => match AgentAssetStore::save_image(&png_bytes, "image/png") {
            Ok(asset) => vec![
                ToolResultContent::Text(format!(
                    "Screenshot captured as asset {} (image/png, {} bytes)",
                    asset.asset_id, asset.size_bytes
                )),
                ToolResultContent::ImageAsset(asset),
            ],
            Err(error) => vec![ToolResultContent::Error(error.to_string())],
        },
        Err(error) => vec![ToolResultContent::Error(error.to_string())],
    }
}

const SCREEN_RECORDING_DENIED_MESSAGE: &str = "Screen Recording permission is required to capture screenshots. \
Grant it in System Settings > Privacy & Security > Screen Recording, \
then restart the app and try again.";

fn capture_and_encode(input: Value) -> Result<Vec<u8>> {
    let granted = permissions::check_screen_recording() == PermissionStatus::Granted;
    capture_and_encode_with_permission(granted, input)
}

/// Capture and encode a screenshot, gated on Screen Recording permission.
///
/// When `granted` is false we refuse to call into `CGDisplay::screenshot` at all:
/// a blind capture without permission returns an empty/desktop-only image that
/// would silently leak a useless frame to the LLM. Instead we surface a clear,
/// actionable error. The `granted` parameter is split out so the denied branch
/// is unit-testable without touching real TCC state.
fn capture_and_encode_with_permission(granted: bool, input: Value) -> Result<Vec<u8>> {
    if !granted {
        // Trigger a one-shot system prompt so the user can grant access for the
        // next attempt; this is idempotent and never loops per-capture.
        permissions::request_screen_recording();
        bail!(SCREEN_RECORDING_DENIED_MESSAGE);
    }

    let region = parse_region(&input)?;
    let image = capture_image(region)?;

    let encoded = encode_png(&image)?;
    if longest_edge(&image)? <= MAX_SCREENSHOT_EDGE && encoded.len() <= MAX_SCREENSHOT_BYTES {
        return Ok(encoded);
    }

    let rgba = cgimage_to_rgba(&image)?;
    let (mut target_width, mut target_height) = scaled_dimensions(rgba.width(), rgba.height());
    let mut encoded = encode_resized_png(&rgba, target_width, target_height)?;

    while encoded.len() > MAX_SCREENSHOT_BYTES && target_width > 1 && target_height > 1 {
        (target_width, target_height) = shrink_dimensions_for_byte_cap(
            target_width,
            target_height,
            encoded.len(),
            MAX_SCREENSHOT_BYTES,
        );
        encoded = encode_resized_png(&rgba, target_width, target_height)?;
    }

    Ok(encoded)
}

fn parse_region(input: &Value) -> Result<CaptureRegion> {
    let region = input
        .get("region")
        .and_then(Value::as_str)
        .unwrap_or("full");

    match region {
        "full" => Ok(CaptureRegion::Full),
        "frontmost" => Ok(CaptureRegion::Frontmost),
        other => bail!("Invalid region '{other}'. Expected 'full' or 'frontmost'"),
    }
}

fn capture_image(region: CaptureRegion) -> Result<CGImage> {
    match region {
        CaptureRegion::Full => capture_full_screen(),
        CaptureRegion::Frontmost => capture_frontmost_window(),
    }
}

fn capture_full_screen() -> Result<CGImage> {
    CGDisplay::screenshot(
        CG_ZERO_RECT,
        kCGWindowListOptionOnScreenOnly,
        kCGNullWindowID,
        kCGWindowImageDefault,
    )
    .context(
        "Failed to capture full-screen screenshot. Screen Recording permission may be required",
    )
}

fn capture_frontmost_window() -> Result<CGImage> {
    let window_id = frontmost_window_id().context("No frontmost window available for capture")?;

    CGDisplay::screenshot(
        CG_ZERO_RECT,
        kCGWindowListOptionIncludingWindow,
        window_id,
        kCGWindowImageBoundsIgnoreFraming,
    )
    .context("Failed to capture frontmost window. Screen Recording permission may be required")
}

fn frontmost_window_id() -> Option<CGWindowID> {
    // Required by spec: discover visible windows using OnScreenOnly, then capture frontmost one.
    let list = CGDisplay::window_list_info(
        kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
        None,
    )?;

    let window_number_key = unsafe { CFString::wrap_under_get_rule(kCGWindowNumber) };
    let window_layer_key = unsafe { CFString::wrap_under_get_rule(kCGWindowLayer) };

    for raw_entry in list.get_all_values() {
        if raw_entry.is_null() {
            continue;
        }

        let dictionary: CFDictionary<CFString, CFType> =
            unsafe { TCFType::wrap_under_get_rule(raw_entry as CFDictionaryRef) };

        let layer = cf_i64(&dictionary, &window_layer_key).unwrap_or_default();
        if layer != 0 {
            continue;
        }

        let window_id =
            cf_i64(&dictionary, &window_number_key).and_then(|value| u32::try_from(value).ok());
        if window_id.is_some() {
            return window_id;
        }
    }

    None
}

fn cf_i64(dictionary: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<i64> {
    dictionary
        .find(key)
        .and_then(|value| value.downcast::<CFNumber>())
        .and_then(|number| number.to_i64())
}

fn longest_edge(image: &CGImage) -> Result<u32> {
    let width = u32::try_from(image.width()).context("Screenshot width exceeds u32")?;
    let height = u32::try_from(image.height()).context("Screenshot height exceeds u32")?;
    Ok(width.max(height))
}

fn scaled_dimensions(width: u32, height: u32) -> (u32, u32) {
    let longest = width.max(height);
    if longest <= MAX_SCREENSHOT_EDGE {
        return (width, height);
    }

    let scale = f64::from(MAX_SCREENSHOT_EDGE) / f64::from(longest);
    let scaled_width = (f64::from(width) * scale).round().max(1.0) as u32;
    let scaled_height = (f64::from(height) * scale).round().max(1.0) as u32;
    (scaled_width, scaled_height)
}

fn shrink_dimensions_for_byte_cap(
    width: u32,
    height: u32,
    current_bytes: usize,
    max_bytes: usize,
) -> (u32, u32) {
    if current_bytes <= max_bytes || width <= 1 || height <= 1 {
        return (width, height);
    }

    let byte_ratio = max_bytes.max(1) as f64 / current_bytes.max(1) as f64;
    let scale = byte_ratio.sqrt().clamp(0.5, 0.9);
    let scaled_width = (f64::from(width) * scale).floor().max(1.0) as u32;
    let scaled_height = (f64::from(height) * scale).floor().max(1.0) as u32;

    if scaled_width == width && scaled_height == height {
        (
            width.saturating_sub(1).max(1),
            height.saturating_sub(1).max(1),
        )
    } else {
        (scaled_width, scaled_height)
    }
}

fn encode_resized_png(image: &RgbaImage, width: u32, height: u32) -> Result<Vec<u8>> {
    let resized = image::imageops::resize(image, width, height, FilterType::Lanczos3);
    let cg_image = rgba_to_cgimage(&resized)?;
    encode_png(&cg_image)
}

fn cgimage_to_rgba(image: &CGImage) -> Result<RgbaImage> {
    if image.bits_per_component() != 8 || image.bits_per_pixel() != 32 {
        bail!(
            "Unsupported screenshot format (bits_per_component={}, bits_per_pixel={})",
            image.bits_per_component(),
            image.bits_per_pixel()
        );
    }

    let width = image.width();
    let height = image.height();
    let bytes_per_row = image.bytes_per_row();

    let min_row_bytes = width
        .checked_mul(4)
        .context("Screenshot row width overflow")?;
    if bytes_per_row < min_row_bytes {
        bail!("Invalid screenshot row stride")
    }

    let source_data = image.data();
    let source = source_data.bytes();
    let expected_len = bytes_per_row
        .checked_mul(height)
        .context("Screenshot byte size overflow")?;
    if source.len() < expected_len {
        bail!("Screenshot image buffer is truncated")
    }

    let pixel_count = width
        .checked_mul(height)
        .context("Screenshot pixel count overflow")?;
    let out_len = pixel_count
        .checked_mul(4)
        .context("Screenshot output buffer overflow")?;

    let mut output = vec![0_u8; out_len];
    for row in 0..height {
        let row_offset = row
            .checked_mul(bytes_per_row)
            .context("Screenshot row offset overflow")?;
        for col in 0..width {
            let src = row_offset
                .checked_add(
                    col.checked_mul(4)
                        .context("Screenshot source offset overflow")?,
                )
                .context("Screenshot source offset overflow")?;
            let dst = (row * width + col)
                .checked_mul(4)
                .context("Screenshot destination offset overflow")?;

            // CGWindowListCreateImage commonly provides BGRA; convert to RGBA.
            output[dst] = source[src + 2];
            output[dst + 1] = source[src + 1];
            output[dst + 2] = source[src];
            output[dst + 3] = source[src + 3];
        }
    }

    let width_u32 = u32::try_from(width).context("Screenshot width exceeds u32")?;
    let height_u32 = u32::try_from(height).context("Screenshot height exceeds u32")?;
    RgbaImage::from_raw(width_u32, height_u32, output).context("Failed to build RGBA screenshot")
}

fn rgba_to_cgimage(image: &RgbaImage) -> Result<CGImage> {
    let width = usize::try_from(image.width()).context("Resized width exceeds usize")?;
    let height = usize::try_from(image.height()).context("Resized height exceeds usize")?;
    let bytes_per_row = width
        .checked_mul(4)
        .context("Resized image row stride overflow")?;

    let color_space = CGColorSpace::create_device_rgb();
    let data = Arc::new(image.clone().into_raw());
    let provider = CGDataProvider::from_buffer(data);

    Ok(CGImage::new(
        width,
        height,
        8,
        32,
        bytes_per_row,
        &color_space,
        kCGBitmapByteOrder32Big | kCGImageAlphaLast,
        &provider,
        true,
        kCGRenderingIntentDefault,
    ))
}

fn encode_png(image: &CGImage) -> Result<Vec<u8>> {
    let mutable_data = unsafe { CFDataCreateMutable(kCFAllocatorDefault, 0) };
    if mutable_data.is_null() {
        bail!("Failed to allocate mutable CFData for PNG encoding")
    }

    let png_ut_type = CFString::new("public.png");
    let destination = unsafe {
        CGImageDestinationCreateWithData(
            mutable_data,
            png_ut_type.as_concrete_TypeRef(),
            1,
            ptr::null(),
        )
    };

    if destination.is_null() {
        unsafe { CFRelease(mutable_data as _) };
        bail!("Failed to create ImageIO destination for PNG encoding")
    }

    let finalized = unsafe {
        let image_ref = image.as_ref() as *const _ as core_graphics::sys::CGImageRef;
        CGImageDestinationAddImage(destination, image_ref, ptr::null());
        let finalized = CGImageDestinationFinalize(destination);
        CFRelease(destination as _);
        finalized
    };

    if !finalized {
        unsafe { CFRelease(mutable_data as _) };
        bail!("Failed to finalize PNG encoding")
    }

    let immutable_data = unsafe { CFData::wrap_under_create_rule(mutable_data as CFDataRef) };
    Ok(immutable_data.bytes().to_vec())
}

#[link(name = "ImageIO", kind = "framework")]
unsafe extern "C" {
    fn CGImageDestinationCreateWithData(
        data: CFMutableDataRef,
        r#type: CFStringRef,
        count: usize,
        options: CFDictionaryRef,
    ) -> *mut c_void;

    fn CGImageDestinationAddImage(
        destination: *mut c_void,
        image: core_graphics::sys::CGImageRef,
        properties: CFDictionaryRef,
    );

    fn CGImageDestinationFinalize(destination: *mut c_void) -> bool;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_region_defaults_to_full() {
        let region = parse_region(&json!({})).expect("parse region should succeed");
        assert!(matches!(region, CaptureRegion::Full));
    }

    #[test]
    fn scaled_dimensions_respect_max_edge() {
        let (w, h) = scaled_dimensions(4000, 2000);
        assert_eq!(w, 1568);
        assert_eq!(h, 784);
    }

    #[test]
    fn shrink_dimensions_for_byte_cap_reduces_large_pngs_progressively() {
        let (w, h) = shrink_dimensions_for_byte_cap(1568, 784, 10 * 1024 * 1024, 5 * 1024 * 1024);

        assert!(w < 1568);
        assert!(h < 784);
        assert!(w >= 1);
        assert!(h >= 1);
    }

    #[test]
    fn capture_refuses_when_screen_recording_denied() {
        // granted=false must short-circuit BEFORE any capture and return an
        // actionable permission error instead of a blind/empty frame.
        let error = capture_and_encode_with_permission(false, json!({"region": "full"}))
            .expect_err("denied permission must yield an error");
        assert!(
            error.to_string().contains("Screen Recording permission"),
            "error should mention Screen Recording permission, got: {error}"
        );
    }
}
