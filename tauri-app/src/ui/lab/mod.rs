use leptos::prelude::*;
use serde_json::Value;

use crate::ui::tauri;

#[derive(serde::Serialize)]
struct NoArgs {}

#[derive(serde::Serialize)]
struct TranscribeArgs {
    audio_path: String,
}

#[derive(Clone, Copy, PartialEq)]
enum LabSection {
    Lab,
    Chat,
}

#[derive(Clone)]
struct TranscriptEntry {
    ts: String,
    text: String,
}

#[derive(Clone)]
struct ChatMessage {
    role: String,
    content: String,
}

#[component]
pub fn LabView() -> impl IntoView {
    let (section, set_section) = signal(LabSection::Lab);

    view! {
        <div class="lab-view">
            <div class="flex-between">
                <h2>"CodeScribe Voice & Chat Lab"</h2>
            </div>

            <div class="tab-strip sub-tabs">
                <button
                    class=move || if section.get() == LabSection::Lab { "tab-pill active" } else { "tab-pill" }
                    on:click=move |_| set_section.set(LabSection::Lab)
                >
                    "Voice Lab"
                </button>
                <button
                    class=move || if section.get() == LabSection::Chat { "tab-pill active" } else { "tab-pill" }
                    on:click=move |_| set_section.set(LabSection::Chat)
                >
                    "Chat"
                </button>
            </div>

            <Show when=move || section.get() == LabSection::Lab>
                <LabSurface />
            </Show>
            <Show when=move || section.get() == LabSection::Chat>
                <ChatPanel />
            </Show>
        </div>
    }
}

#[component]
fn LabSurface() -> impl IntoView {
    view! {
        <div class="lab-layout">
            <div class="vista-grid-top">
                <SpectrogramPanel />
                <TranscriptPanel />
            </div>
            <div class="vista-grid-bottom">
                <EndpointPanel />
                <DiagnosticsPanel />
            </div>
        </div>
    }
}

#[component]
fn SpectrogramPanel() -> impl IntoView {
    let (is_streaming, set_is_streaming) = signal(false);
    let (status_text, set_status_text) = signal("Ready".to_string());
    let (buffer_kb, set_buffer_kb) = signal(0.0f32);

    view! {
        <section class="vista-panel">
            <div class="flex-between">
                <h3>"Streaming Spectrogram"</h3>
                <span class="status-pill">{move || status_text.get()}</span>
            </div>

            <div class="spectrogram-placeholder">
                <p class="muted">"(Spectrogram visualization - coming soon)"</p>
                <p class="muted">"Canvas-based audio visualization requires WebGL integration"</p>
            </div>

            <div class="controls row" style="margin-top: 14px;">
                <button
                    disabled=move || is_streaming.get()
                    on:click=move |_| {
                        set_is_streaming.set(true);
                        set_status_text.set("Streaming...".to_string());
                        set_buffer_kb.set(0.0);
                    }
                >
                    "Start streaming"
                </button>
                <button
                    class="secondary"
                    disabled=move || !is_streaming.get()
                    on:click=move |_| {
                        set_is_streaming.set(false);
                        set_status_text.set("Stopped".to_string());
                    }
                >
                    "Stop"
                </button>
            </div>

            <div class="progress-wrap">
                <progress max="100" value=move || (buffer_kb.get() / 100.0 * 100.0).min(100.0) as i32></progress>
                <span>{move || format!("{:.1} KB buffered", buffer_kb.get())}</span>
            </div>
        </section>
    }
}

#[component]
fn TranscriptPanel() -> impl IntoView {
    let (transcript, _set_transcript) = signal(String::new());
    let (history, _set_history) = signal(Vec::<TranscriptEntry>::new());

    let copy_transcript = move |_| {
        let text = transcript.get();
        if !text.is_empty() {
            // In WASM, we'd use clipboard API
            // For now, just log
            log::info!("Copy transcript: {}", text);
        }
    };

    view! {
        <section class="vista-panel">
            <div class="flex-between">
                <h3>"Live Transcript"</h3>
                <button class="secondary" on:click=copy_transcript>
                    "Copy transcript"
                </button>
            </div>

            <div class="transcript-box">
                {move || {
                    let t = transcript.get();
                    if t.is_empty() {
                        "(no transcript yet)".to_string()
                    } else {
                        t
                    }
                }}
            </div>

            <div class="transcript-history">
                <For
                    each=move || history.get()
                    key=|e| format!("{}-{}", e.ts, e.text)
                    children=move |entry| view! {
                        <div class="history-chip">
                            "[" {entry.ts.clone()} "] " {entry.text.clone()}
                        </div>
                    }
                />
            </div>
        </section>
    }
}

#[component]
fn EndpointPanel() -> impl IntoView {
    let (audio_path, set_audio_path) = signal(String::new());
    let (output, set_output) = signal(String::new());
    let (error, set_error) = signal(None::<String>);
    let (is_busy, set_is_busy) = signal(false);

    let transcribe = move |_| {
        let path = audio_path.get();
        if path.is_empty() {
            set_error.set(Some("Please enter an audio file path".to_string()));
            return;
        }
        set_error.set(None);
        set_output.set(String::new());
        set_is_busy.set(true);

        leptos::task::spawn_local(async move {
            let res: Result<String, String> = tauri::invoke(
                "transcribe_audio",
                TranscribeArgs { audio_path: path },
            ).await;

            set_is_busy.set(false);
            match res {
                Ok(t) => set_output.set(t),
                Err(e) => set_error.set(Some(e)),
            }
        });
    };

    view! {
        <section class="vista-panel">
            <h3>"Endpoint & Capture Controls"</h3>

            <div class="input-stack">
                <label>"Audio file path"</label>
                <input
                    class="input"
                    type="text"
                    placeholder="/path/to/audio.wav or .mp3"
                    prop:value=move || audio_path.get()
                    on:input=move |ev| set_audio_path.set(event_target_value(&ev))
                />
                <small class="label-muted">
                    "Enter path to an audio file to transcribe"
                </small>
            </div>

            <div class="controls row" style="margin-top: 14px;">
                <button
                    class="secondary"
                    disabled=move || is_busy.get()
                    on:click=transcribe
                >
                    {move || if is_busy.get() { "Transcribing..." } else { "Upload → STT" }}
                </button>
            </div>

            <Show when=move || error.get().is_some()>
                <pre class="error">{move || error.get().unwrap_or_default()}</pre>
            </Show>

            <Show when=move || !output.get().is_empty()>
                <pre class="endpoint-output">{move || output.get()}</pre>
            </Show>
        </section>
    }
}

#[component]
fn DiagnosticsPanel() -> impl IntoView {
    let (config, set_config) = signal(None::<String>);
    let (models, set_models) = signal(Vec::<String>::new());
    let (devices, set_devices) = signal(Vec::<String>::new());
    let (error, set_error) = signal(None::<String>);

    view! {
        <section class="vista-panel">
            <h3>"IPC Diagnostics"</h3>

            <div class="controls row">
                <button class="secondary" on:click=move |_| {
                    set_error.set(None);
                    leptos::task::spawn_local(async move {
                        let res: Result<Value, String> = tauri::invoke("get_config", NoArgs {}).await;
                        match res {
                            Ok(v) => set_config.set(serde_json::to_string_pretty(&v).ok()),
                            Err(e) => set_error.set(Some(e)),
                        }
                    });
                }>
                    "Load config"
                </button>

                <button class="secondary" on:click=move |_| {
                    set_error.set(None);
                    leptos::task::spawn_local(async move {
                        let res: Result<Vec<String>, String> = tauri::invoke("get_available_models", NoArgs {}).await;
                        match res {
                            Ok(v) => set_models.set(v),
                            Err(e) => set_error.set(Some(e)),
                        }
                    });
                }>
                    "List models"
                </button>

                <button class="secondary" on:click=move |_| {
                    set_error.set(None);
                    leptos::task::spawn_local(async move {
                        let res: Result<Vec<String>, String> = tauri::invoke("list_audio_devices", NoArgs {}).await;
                        match res {
                            Ok(v) => set_devices.set(v),
                            Err(e) => set_error.set(Some(e)),
                        }
                    });
                }>
                    "List devices"
                </button>
            </div>

            <Show when=move || error.get().is_some()>
                <pre class="error">{move || error.get().unwrap_or_default()}</pre>
            </Show>

            <Show when=move || config.get().is_some()>
                <details>
                    <summary>"Config (click to expand)"</summary>
                    <pre class="code">{move || config.get().unwrap_or_default()}</pre>
                </details>
            </Show>

            <Show when=move || !models.get().is_empty()>
                <div class="list">
                    <strong>"Models: "</strong>
                    {move || models.get().join(", ")}
                </div>
            </Show>

            <Show when=move || !devices.get().is_empty()>
                <div class="list">
                    <strong>"Devices: "</strong>
                    {move || devices.get().join(", ")}
                </div>
            </Show>
        </section>
    }
}

#[component]
fn ChatPanel() -> impl IntoView {
    let (draft, set_draft) = signal(String::new());
    let (messages, set_messages) = signal(Vec::<ChatMessage>::new());
    let (is_busy, set_is_busy) = signal(false);
    let (status, set_status) = signal(String::new());

    let do_send = move || {
        let text = draft.get().trim().to_string();
        if text.is_empty() || is_busy.get() {
            return;
        }

        // Add user message
        let mut msgs = messages.get();
        msgs.push(ChatMessage {
            role: "user".to_string(),
            content: text.clone(),
        });
        set_messages.set(msgs);
        set_draft.set(String::new());
        set_is_busy.set(true);
        set_status.set("Sending...".to_string());

        // Simulate assistant response (placeholder)
        leptos::task::spawn_local(async move {
            // TODO: Implement actual chat via Tauri command
            // For now, just add a placeholder response
            gloo_timers::future::TimeoutFuture::new(500).await;

            let mut msgs = messages.get();
            msgs.push(ChatMessage {
                role: "assistant".to_string(),
                content: "(Chat integration coming soon - connect to LLM endpoint in Settings)".to_string(),
            });
            set_messages.set(msgs);
            set_is_busy.set(false);
            set_status.set(String::new());
        });
    };

    let reset_chat = move |_: web_sys::MouseEvent| {
        set_messages.set(Vec::new());
        set_draft.set(String::new());
        set_status.set(String::new());
    };

    view! {
        <section class="vista-panel chat-layout">
            <div class="chat-main">
                <h3>"Assistant Conversation"</h3>

                <div class="chat-messages">
                    <Show when=move || messages.get().is_empty()>
                        <p class="muted">"Start a conversation by typing a message below."</p>
                    </Show>
                    <For
                        each=move || messages.get()
                        key=|m| format!("{}-{}", m.role, m.content.len())
                        children=move |msg| {
                            let class = format!("chat-bubble {}", msg.role);
                            view! {
                                <div class=class>
                                    {msg.content.clone()}
                                </div>
                            }
                        }
                    />
                </div>

                <div class="chat-composer">
                    <textarea
                        placeholder="Ask something... (Shift+Enter for newline)"
                        prop:value=move || draft.get()
                        on:input=move |ev| set_draft.set(event_target_value(&ev))
                        on:keydown=move |ev: web_sys::KeyboardEvent| {
                            if ev.key() == "Enter" && !ev.shift_key() {
                                ev.prevent_default();
                                do_send();
                            }
                        }
                    ></textarea>

                    <div class="chat-actions">
                        <span class="muted">{move || status.get()}</span>
                        <button class="secondary" on:click=reset_chat>
                            "Reset chat"
                        </button>
                        <button
                            disabled=move || is_busy.get()
                            on:click=move |_: web_sys::MouseEvent| do_send()
                        >
                            "Send"
                        </button>
                    </div>
                </div>
            </div>

            <div class="chat-side-card">
                <strong>"How it works"</strong>
                <p>
                    "Messages are sent through the configured LLM endpoint (Settings → LLM host). "
                    "Responses live in this session only – reset chat to start fresh."
                </p>
                <p class="muted">
                    "Tip: Configure your LLM endpoint in Settings before using chat."
                </p>
            </div>
        </section>
    }
}
