use std::sync::Arc;

#[tokio::test]
async fn test_format_text_with_status_signature() {
    let callback = Arc::new(|delta: &str| {
        println!("Received delta: {}", delta);
    });

    // We don't await/unwrap because we don't want to make real API calls or need config.
    // Just checking if the function call compiles with the new signature.
    let _future = codescribe::ai_formatting::format_text_with_status(
        "test input",
        Some("en"),
        true, // assistive
        Some(callback),
    );

    // Check call without callback
    let _future2 =
        codescribe::ai_formatting::format_text_with_status("test input", None, false, None);
}
