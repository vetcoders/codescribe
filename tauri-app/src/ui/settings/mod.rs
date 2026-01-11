use leptos::prelude::*;
use serde_json::Value;

use crate::ui::tauri;

#[derive(serde::Serialize)]
struct NoArgs {}

#[derive(serde::Serialize)]
struct SaveConfigArgs {
    config: Value,
}

#[derive(serde::Serialize)]
struct AudioNoArgs {}

#[component]
pub fn SettingsView() -> impl IntoView {
    let (loaded, set_loaded) = signal(false);
    let (error, set_error) = signal(None::<String>);

    let (use_local_stt, set_use_local_stt) = signal(false);
    let (local_model, set_local_model) = signal(String::new());
    let (stt_endpoint, set_stt_endpoint) = signal(String::new());
    let (llm_host, set_llm_host) = signal(String::new());

    let (hold_mods, set_hold_mods) = signal(String::from("ctrl_alt"));
    let (hold_exclusive, set_hold_exclusive) = signal(false);
    let (toggle_trigger, set_toggle_trigger) = signal(String::from("double_option"));
    let (hold_start_delay_ms, set_hold_start_delay_ms) = signal(200u64);
    let (whisper_language, set_whisper_language) = signal(String::from("auto"));

    let (audio_devices, set_audio_devices) = signal(Vec::<String>::new());
    let (current_audio_device, set_current_audio_device) = signal(None::<String>);
    // Empty means "use system default"
    let (audio_input_device, set_audio_input_device) = signal(String::new());

    let (models, set_models) = signal(Vec::<String>::new());

    Effect::new(move |_| {
        if loaded.get() {
            return;
        }
        set_loaded.set(true);

        leptos::task::spawn_local(async move {
            // Load config
            let cfg: Result<Value, String> = tauri::invoke("get_config", NoArgs {}).await;
            match cfg {
                Ok(v) => {
                    set_use_local_stt.set(v.get("use_local_stt").and_then(|x| x.as_bool()).unwrap_or(false));
                    set_local_model.set(
                        v.get("local_model")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_string(),
                    );
                    set_stt_endpoint.set(
                        v.get("stt_endpoint")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_string(),
                    );
                    set_llm_host.set(
                        v.get("ollama_host")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_string(),
                    );

                    set_hold_mods.set(
                        v.get("hold_mods")
                            .and_then(|x| x.as_str())
                            .unwrap_or("ctrl_alt")
                            .to_string(),
                    );
                    set_hold_exclusive.set(
                        v.get("hold_exclusive")
                            .and_then(|x| x.as_bool())
                            .unwrap_or(false),
                    );
                    set_toggle_trigger.set(
                        v.get("toggle_trigger")
                            .and_then(|x| x.as_str())
                            .unwrap_or("double_option")
                            .to_string(),
                    );
                    set_hold_start_delay_ms.set(
                        v.get("hold_start_delay_ms")
                            .and_then(|x| x.as_u64())
                            .unwrap_or(200),
                    );
                    set_whisper_language.set(
                        v.get("whisper_language")
                            .and_then(|x| x.as_str())
                            .unwrap_or("auto")
                            .to_string(),
                    );
                    set_audio_input_device.set(
                        v.get("audio_input_device")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_string(),
                    );
                }
                Err(e) => set_error.set(Some(e)),
            }

            // Load models
            let res: Result<Vec<String>, String> = tauri::invoke("get_available_models", NoArgs {}).await;
            if let Ok(v) = res {
                set_models.set(v);
            }

            // Load audio devices
            let devs: Result<Vec<String>, String> = tauri::invoke("list_audio_devices", AudioNoArgs {}).await;
            if let Ok(v) = devs {
                set_audio_devices.set(v);
            }
            let current: Result<Option<String>, String> = tauri::invoke("get_current_audio_device", AudioNoArgs {}).await;
            if let Ok(v) = current {
                set_current_audio_device.set(v);
            }
        });
    });

    view! {
        <div class="settings-view">
            <h2>"Settings"</h2>

            <Show when=move || error.get().is_some()>
                <pre class="error">{move || error.get().unwrap_or_default()}</pre>
            </Show>

            <div class="panel">
                <h3>"STT"</h3>

                <label class="row">
                    <input
                        type="checkbox"
                        prop:checked=move || use_local_stt.get()
                        on:change=move |ev| {
                            let checked = event_target_checked(&ev);
                            set_use_local_stt.set(checked);
                        }
                    />
                    <span>"Use local STT (Whisper)"</span>
                </label>

                <div class="row">
                    <label class="label">"Local model"</label>
                    <select
                        class="input"
                        on:change=move |ev| set_local_model.set(event_target_value(&ev))
                        prop:value=move || local_model.get()
                    >
                        <For
                            each=move || models.get()
                            key=|m| m.clone()
                            children=move |m| view! { <option value={m.clone()}>{m.clone()}</option> }
                        />
                    </select>
                </div>

                <div class="row">
                    <label class="label">"STT endpoint (optional)"</label>
                    <input
                        class="input"
                        placeholder="https://..."
                        prop:value=move || stt_endpoint.get()
                        on:input=move |ev| set_stt_endpoint.set(event_target_value(&ev))
                    />
                </div>
            </div>

            <div class="panel">
                <h3>"LLM"</h3>
                <div class="row">
                    <label class="label">"LLM host"</label>
                    <input
                        class="input"
                        placeholder="http://localhost:11434"
                        prop:value=move || llm_host.get()
                        on:input=move |ev| set_llm_host.set(event_target_value(&ev))
                    />
                </div>
            </div>

            <div class="panel">
                <h3>"Language"</h3>
                <div class="row">
                    <label class="label">"Whisper language"</label>
                    <select
                        class="input"
                        prop:value=move || whisper_language.get()
                        on:change=move |ev| set_whisper_language.set(event_target_value(&ev))
                    >
                        <option value="auto">"auto"</option>
                        <option value="pl">"pl"</option>
                        <option value="en">"en"</option>
                    </select>
                </div>
            </div>

            <div class="panel">
                <h3>"Hotkeys"</h3>

                <div class="row">
                    <label class="label">"Hold mods"</label>
                    <select
                        class="input"
                        prop:value=move || hold_mods.get()
                        on:change=move |ev| set_hold_mods.set(event_target_value(&ev))
                    >
                        <option value="ctrl">"ctrl"</option>
                        <option value="ctrl_alt">"ctrl_alt"</option>
                        <option value="ctrl_shift">"ctrl_shift"</option>
                        <option value="ctrl_cmd">"ctrl_cmd"</option>
                    </select>
                </div>

                <label class="row">
                    <input
                        type="checkbox"
                        prop:checked=move || hold_exclusive.get()
                        on:change=move |ev| set_hold_exclusive.set(event_target_checked(&ev))
                    />
                    <span>"Hold exclusive"</span>
                </label>

                <div class="row">
                    <label class="label">"Toggle trigger"</label>
                    <select
                        class="input"
                        prop:value=move || toggle_trigger.get()
                        on:change=move |ev| set_toggle_trigger.set(event_target_value(&ev))
                    >
                        <option value="double_option">"double_option"</option>
                        <option value="double_ralt">"double_ralt"</option>
                        <option value="none">"none"</option>
                    </select>
                </div>

                <div class="row">
                    <label class="label">"Hold start delay (ms)"</label>
                    <input
                        class="input"
                        type="number"
                        min="0"
                        prop:value=move || hold_start_delay_ms.get().to_string()
                        on:input=move |ev| {
                            let v = event_target_value(&ev);
                            if let Ok(n) = v.parse::<u64>() {
                                set_hold_start_delay_ms.set(n);
                            }
                        }
                    />
                </div>
            </div>

            <div class="panel">
                <h3>"Audio"</h3>
                <div class="row">
                    <label class="label">"Input device"</label>
                    <select
                        class="input"
                        prop:value=move || audio_input_device.get()
                        on:change=move |ev| set_audio_input_device.set(event_target_value(&ev))
                    >
                        <option value="">
                            {move || {
                                let current = current_audio_device.get().unwrap_or_else(|| "(unknown)".to_string());
                                format!("System default (current: {})", current)
                            }}
                        </option>
                        <For
                            each=move || audio_devices.get()
                            key=|d| d.clone()
                            children=move |d| view! { <option value={d.clone()}>{d.clone()}</option> }
                        />
                    </select>
                </div>
            </div>

            <div class="row">
                <button on:click=move |_| {
                    set_error.set(None);
                    let payload = serde_json::json!({
                        "use_local_stt": use_local_stt.get(),
                        "local_model": local_model.get(),
                        "stt_endpoint": stt_endpoint.get(),
                        "llm_host": llm_host.get(),
                        "whisper_language": whisper_language.get(),
                        "hold_mods": hold_mods.get(),
                        "hold_exclusive": hold_exclusive.get(),
                        "toggle_trigger": toggle_trigger.get(),
                        "hold_start_delay_ms": hold_start_delay_ms.get(),
                        "audio_input_device": audio_input_device.get(),
                    });
                    leptos::task::spawn_local(async move {
                        let res: Result<(), String> = tauri::invoke(
                            "save_config",
                            SaveConfigArgs { config: payload },
                        )
                        .await;
                        if let Err(e) = res {
                            set_error.set(Some(e));
                        }
                    });
                }>
                    "Save"
                </button>
            </div>
        </div>
    }
}
