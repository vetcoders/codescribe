//! Icon management and status glyph rendering for tray icon
//!
//! Handles loading the CodeScribe logo and drawing status indicators.

use anyhow::Result;
use image::{GenericImageView, imageops::FilterType};
use std::sync::atomic::{AtomicBool, Ordering};
use tray_icon::Icon;

use crate::tray::types::TrayStatus;

/// Parameters for drawing a glyph on the icon
struct GlyphParams {
    center_x: i32,
    center_y: i32,
    radius: i32,
    color: (u8, u8, u8),
}

/// Embedded CodeScribe logo icon (resized for menu bar)
/// Place icon.png in codescribe-rs/assets/ directory
const ICON_BYTES: &[u8] = include_bytes!("../../assets/icon.png");

/// Menu bar icon size (44x44 for Retina, 22x22 logical)
const ICON_SIZE: u32 = 44;

/// Global flag for status glyph visibility
static SHOW_STATUS_GLYPH: AtomicBool = AtomicBool::new(true);

/// Get whether the status glyph is currently enabled
pub fn is_status_glyph_enabled() -> bool {
    SHOW_STATUS_GLYPH.load(Ordering::SeqCst)
}

/// Load the custom CodeScribe icon, optionally tinted by status
pub fn load_custom_icon(status: TrayStatus) -> Result<Icon> {
    let img = image::load_from_memory(ICON_BYTES)
        .map_err(|e| anyhow::anyhow!("Failed to load icon: {}", e))?;

    // Resize to menu bar size (44x44 for Retina)
    let resized = img.resize_exact(ICON_SIZE, ICON_SIZE, FilterType::Lanczos3);
    let (width, height) = resized.dimensions();
    let mut rgba = resized.to_rgba8().into_raw();

    // Icon stays white/neutral - no tinting
    // Status is shown only via the glyph color

    // Draw status glyph if enabled (larger colored dot in bottom-right corner)
    if is_status_glyph_enabled() {
        draw_status_glyph(&mut rgba, width, height, status);
    }

    Icon::from_rgba(rgba, width, height)
        .map_err(|e| anyhow::anyhow!("Failed to create icon: {}", e))
}

/// Draw the status glyph on the icon
fn draw_status_glyph(rgba: &mut [u8], width: u32, height: u32, status: TrayStatus) {
    // Glyph parameters - larger circle (12x12) for better visibility
    const GLYPH_RADIUS: i32 = 6;
    let glyph_center_x = (width as i32) - GLYPH_RADIUS - 2; // 2px padding from edge
    let glyph_center_y = (height as i32) - GLYPH_RADIUS - 2;

    // Status-based glyph colors:
    // - Green: Idle/Ready, Success
    // - Red: Recording/Listening, Error (X shape)
    // - Orange: Processing/Thinking
    let color = match status {
        TrayStatus::Idle => (80u8, 200, 100),   // Green - ready
        TrayStatus::Listening => (255, 70, 70), // Red - recording
        TrayStatus::Thinking => (255, 165, 0),  // Orange - processing
        TrayStatus::Success => (80, 220, 100),  // Bright green - done
        TrayStatus::Error => (255, 50, 50),     // Bright red - error
    };

    let params = GlyphParams {
        center_x: glyph_center_x,
        center_y: glyph_center_y,
        radius: GLYPH_RADIUS,
        color,
    };

    // For Error status, draw an "X" instead of a circle
    if status == TrayStatus::Error {
        draw_x_glyph(rgba, width, height, &params);
    } else {
        draw_circle_glyph(rgba, width, height, &params);
    }
}

/// Draw an X-shaped glyph for error status
fn draw_x_glyph(rgba: &mut [u8], width: u32, height: u32, params: &GlyphParams) {
    const LINE_WIDTH: i32 = 2;
    let (r, g, b) = params.color;

    for y in (params.center_y - params.radius).max(0)
        ..(params.center_y + params.radius).min(height as i32)
    {
        for x in (params.center_x - params.radius).max(0)
            ..(params.center_x + params.radius).min(width as i32)
        {
            let dx = x - params.center_x;
            let dy = y - params.center_y;

            // Check if point is on diagonal lines (forming X)
            let on_diag1 = (dx - dy).abs() <= LINE_WIDTH;
            let on_diag2 = (dx + dy).abs() <= LINE_WIDTH;

            // Only draw within the circle bounds
            let in_bounds = dx * dx + dy * dy <= params.radius * params.radius;

            if in_bounds && (on_diag1 || on_diag2) {
                let idx = ((y as u32 * width + x as u32) * 4) as usize;
                rgba[idx] = r;
                rgba[idx + 1] = g;
                rgba[idx + 2] = b;
                rgba[idx + 3] = 255;
            }
        }
    }
}

/// Draw a circular glyph for normal status
fn draw_circle_glyph(rgba: &mut [u8], width: u32, height: u32, params: &GlyphParams) {
    let (r, g, b) = params.color;

    for y in (params.center_y - params.radius).max(0)
        ..(params.center_y + params.radius).min(height as i32)
    {
        for x in (params.center_x - params.radius).max(0)
            ..(params.center_x + params.radius).min(width as i32)
        {
            let dx = x - params.center_x;
            let dy = y - params.center_y;
            let distance_squared = dx * dx + dy * dy;

            if distance_squared <= params.radius * params.radius {
                let idx = ((y as u32 * width + x as u32) * 4) as usize;
                rgba[idx] = r;
                rgba[idx + 1] = g;
                rgba[idx + 2] = b;
                rgba[idx + 3] = 255; // Fully opaque
            }
        }
    }
}

/// Create a simple colored circle icon as fallback
pub fn create_fallback_icon(status: TrayStatus) -> Result<Icon> {
    const SIZE: u32 = 22;
    const RADIUS: i32 = 10;
    const CENTER: i32 = 11;

    let (r, g, b) = match status {
        TrayStatus::Idle => (100u8, 100, 100),  // Gray
        TrayStatus::Listening => (220, 60, 60), // Red
        TrayStatus::Thinking => (60, 130, 220), // Blue
        TrayStatus::Success => (60, 200, 100),  // Green
        TrayStatus::Error => (255, 50, 50),     // Bright red
    };

    let mut rgba = vec![0u8; (SIZE * SIZE * 4) as usize];

    for y in 0..SIZE as i32 {
        for x in 0..SIZE as i32 {
            let dx = x - CENTER;
            let dy = y - CENTER;
            if dx * dx + dy * dy <= RADIUS * RADIUS {
                let idx = ((y as u32 * SIZE + x as u32) * 4) as usize;
                rgba[idx] = r;
                rgba[idx + 1] = g;
                rgba[idx + 2] = b;
                rgba[idx + 3] = 255;
            }
        }
    }

    Icon::from_rgba(rgba, SIZE, SIZE)
        .map_err(|e| anyhow::anyhow!("Failed to create fallback icon: {}", e))
}
