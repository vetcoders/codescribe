use anyhow::{Context, Result, bail};
use arboard::Clipboard;
use codescribe_core::agent::{ToolDefinition, ToolRegistry, ToolResultContent};
use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder, RgbaImage};
use serde_json::{Value, json};

const OK_TEXT: &str = "ok";

pub fn register(registry: &mut ToolRegistry) {
    registry
        .register(
            read_clipboard_definition(),
            Box::new(|input| Box::pin(handle_read(input))),
        )
        .expect("register read_clipboard tool");
    registry
        .register(
            write_clipboard_definition(),
            Box::new(|input| Box::pin(handle_write(input))),
        )
        .expect("register write_clipboard tool");
}

fn read_clipboard_definition() -> ToolDefinition {
    ToolDefinition {
        name: "read_clipboard".to_string(),
        description: "Read the current clipboard content. Returns text content, or image data if clipboard contains an image.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {}
        }),
    }
}

fn write_clipboard_definition() -> ToolDefinition {
    ToolDefinition {
        name: "write_clipboard".to_string(),
        description: "Write text to the system clipboard.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "text": {
                    "type": "string",
                    "description": "Text to write to clipboard"
                }
            },
            "required": ["text"]
        }),
    }
}

async fn handle_read(_input: Value) -> Vec<ToolResultContent> {
    match read_clipboard_with(
        crate::os::clipboard::get_clipboard,
        read_clipboard_image_png,
    ) {
        Ok(content) => vec![content],
        Err(error) => vec![ToolResultContent::Error(error.to_string())],
    }
}

async fn handle_write(input: Value) -> Vec<ToolResultContent> {
    match write_clipboard_with(&input, crate::os::clipboard::set_clipboard) {
        Ok(content) => vec![content],
        Err(error) => vec![ToolResultContent::Error(error.to_string())],
    }
}

fn read_clipboard_with<GetText, GetImage>(
    get_text: GetText,
    get_image: GetImage,
) -> Result<ToolResultContent>
where
    GetText: Fn() -> Result<String>,
    GetImage: Fn() -> Result<Option<Vec<u8>>>,
{
    match get_text() {
        Ok(text) => return Ok(ToolResultContent::Text(text)),
        Err(error) => {
            tracing::debug!("text clipboard read failed, trying image fallback: {error}");
        }
    }

    if let Some(image_bytes) = get_image()? {
        return Ok(ToolResultContent::Image {
            data: image_bytes,
            media_type: "image/png".to_string(),
        });
    }

    bail!("Clipboard is empty or contains unsupported data")
}

fn write_clipboard_with<SetClipboard>(
    input: &Value,
    setter: SetClipboard,
) -> Result<ToolResultContent>
where
    SetClipboard: Fn(&str) -> Result<()>,
{
    let text = input
        .get("text")
        .and_then(Value::as_str)
        .context("Missing required string field 'text'")?;

    setter(text).context("Failed to write clipboard text")?;
    Ok(ToolResultContent::Text(OK_TEXT.to_string()))
}

fn read_clipboard_image_png() -> Result<Option<Vec<u8>>> {
    let mut clipboard = Clipboard::new().context("Failed to initialize clipboard")?;
    let image = match clipboard.get_image() {
        Ok(image) => image,
        Err(_) => return Ok(None),
    };

    let width = u32::try_from(image.width).context("Clipboard image width exceeds u32")?;
    let height = u32::try_from(image.height).context("Clipboard image height exceeds u32")?;

    let rgba = RgbaImage::from_raw(width, height, image.bytes.into_owned())
        .context("Invalid clipboard image buffer")?;

    let mut png_data = Vec::new();
    let encoder = PngEncoder::new(&mut png_data);
    encoder
        .write_image(rgba.as_raw(), width, height, ExtendedColorType::Rgba8)
        .context("Failed to encode clipboard image as PNG")?;

    Ok(Some(png_data))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_clipboard_returns_text_first() {
        let result = read_clipboard_with(
            || Ok("Hello clipboard".to_string()),
            || Ok(Some(vec![1_u8, 2_u8, 3_u8])),
        )
        .expect("read clipboard should succeed");

        assert_eq!(
            result,
            ToolResultContent::Text("Hello clipboard".to_string())
        );
    }

    #[test]
    fn read_clipboard_falls_back_to_image() {
        let result = read_clipboard_with(
            || bail!("text unavailable"),
            || Ok(Some(vec![9_u8, 8_u8, 7_u8])),
        )
        .expect("image fallback should succeed");

        assert_eq!(
            result,
            ToolResultContent::Image {
                data: vec![9_u8, 8_u8, 7_u8],
                media_type: "image/png".to_string(),
            }
        );
    }

    #[test]
    fn write_clipboard_uses_setter() {
        let input = json!({ "text": "Paste me" });
        let mut written = String::new();

        let result = write_clipboard_with(&input, |text| {
            written = text.to_string();
            Ok(())
        })
        .expect("write clipboard should succeed");

        assert_eq!(written, "Paste me");
        assert_eq!(result, ToolResultContent::Text("ok".to_string()));
    }
}
