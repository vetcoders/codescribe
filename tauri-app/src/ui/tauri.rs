use serde::Serialize;
use serde::de::DeserializeOwned;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "core"], js_name = invoke, catch)]
    async fn invoke_js(cmd: &str, args: JsValue) -> Result<JsValue, JsValue>;
}

/// Check if Tauri IPC is available
pub fn is_tauri_available() -> bool {
    let window = web_sys::window().expect("no window");
    js_sys::Reflect::has(&window, &"__TAURI__".into()).unwrap_or(false)
}

pub async fn invoke<T: DeserializeOwned>(cmd: &str, args: impl Serialize) -> Result<T, String> {
    // Wait for Tauri to be available (with timeout)
    for _ in 0..50 {
        if is_tauri_available() {
            break;
        }
        gloo_timers::future::TimeoutFuture::new(10).await;
    }

    if !is_tauri_available() {
        return Err("Tauri IPC not available".to_string());
    }

    let args = serde_wasm_bindgen::to_value(&args).map_err(|e| e.to_string())?;

    match invoke_js(cmd, args).await {
        Ok(v) => serde_wasm_bindgen::from_value(v).map_err(|e| e.to_string()),
        Err(e) => {
            // Tauri error - extract message from JsValue
            let msg = js_sys::Reflect::get(&e, &"message".into())
                .ok()
                .and_then(|v| v.as_string())
                .unwrap_or_else(|| format!("{:?}", e));
            Err(msg)
        }
    }
}
