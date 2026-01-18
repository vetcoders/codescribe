//! Assistant Tab - Chat with AI via IPC
//!
//! Provides conversational interface to the AI assistant.
//! Messages are sent to CLI via IPC for processing.
//!
//! Created by M&K (c)2026 VetCoders

use leptos::prelude::*;

#[cfg(target_arch = "wasm32")]
use crate::ui::tauri::invoke;

/// Chat message
#[derive(Clone, Debug)]
pub struct ChatMessage {
    pub role: MessageRole,
    pub content: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MessageRole {
    User,
    Assistant,
}

/// Assistant View - Chat interface
#[component]
pub fn AssistantView() -> impl IntoView {
    let (messages, set_messages) = signal(Vec::<ChatMessage>::new());
    let (input, set_input) = signal(String::new());
    let (is_loading, set_loading) = signal(false);
    let (error, set_error) = signal(Option::<String>::None);

    // Send message handler (generic over event type)
    let do_send = move || {
        let msg = input.get();
        if msg.trim().is_empty() || is_loading.get() {
            return;
        }

        // Add user message
        set_messages.update(|m| {
            m.push(ChatMessage {
                role: MessageRole::User,
                content: msg.clone(),
            });
        });
        set_input.set(String::new());
        set_loading.set(true);
        set_error.set(None);

        // Send to CLI via IPC
        #[cfg(target_arch = "wasm32")]
        {
            use wasm_bindgen_futures::spawn_local;

            let msg_clone = msg.clone();
            spawn_local(async move {
                #[derive(serde::Serialize)]
                struct SendMessageArgs {
                    message: String,
                }

                match invoke::<MessageResponse>(
                    "send_message",
                    SendMessageArgs { message: msg_clone },
                )
                .await
                {
                    Ok(response) => {
                        set_messages.update(|m| {
                            m.push(ChatMessage {
                                role: MessageRole::Assistant,
                                content: response.content,
                            });
                        });
                        set_loading.set(false);
                    }
                    Err(e) => {
                        set_error.set(Some(format!("Error: {}", e)));
                        set_loading.set(false);
                    }
                }
            });
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            // Desktop fallback - won't reach here in Tauri
            set_loading.set(false);
        }
    };

    // Separate handlers for click and keydown
    let send_on_click = move |_: web_sys::MouseEvent| do_send();
    let send_on_enter = move |ev: web_sys::KeyboardEvent| {
        if ev.key() == "Enter" && !ev.shift_key() {
            ev.prevent_default();
            do_send();
        }
    };

    // Reset context handler
    let reset_context = move |_| {
        #[cfg(target_arch = "wasm32")]
        {
            use wasm_bindgen_futures::spawn_local;

            spawn_local(async move {
                match invoke::<()>("reset_ai_context", ()).await {
                    Ok(()) => {
                        set_messages.set(Vec::new());
                        set_error.set(None);
                    }
                    Err(e) => {
                        set_error.set(Some(format!("Error resetting context: {}", e)));
                    }
                }
            });
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            set_messages.set(Vec::new());
        }
    };

    view! {
        <div class="assistant-container">
            <div class="assistant-header">
                <h2>"AI Assistant"</h2>
                <button class="reset-btn" on:click=reset_context>
                    "Reset Context"
                </button>
            </div>

            <div class="messages-container">
                {move || {
                    let msgs = messages.get();
                    if msgs.is_empty() {
                        view! {
                            <div class="empty-state">
                                <p>"Start a conversation with the AI assistant."</p>
                                <p class="hint">"Messages are processed by the CLI's embedded model."</p>
                            </div>
                        }.into_any()
                    } else {
                        view! {
                            <div class="messages">
                                {msgs.into_iter().map(|msg| {
                                    let class = match msg.role {
                                        MessageRole::User => "message user",
                                        MessageRole::Assistant => "message assistant",
                                    };
                                    view! {
                                        <div class=class>
                                            <div class="message-content">{msg.content}</div>
                                        </div>
                                    }
                                }).collect::<Vec<_>>()}
                            </div>
                        }.into_any()
                    }
                }}

                {move || is_loading.get().then(|| view! {
                    <div class="message assistant loading">
                        <div class="typing-indicator">
                            <span></span><span></span><span></span>
                        </div>
                    </div>
                })}

                {move || error.get().map(|e| view! {
                    <div class="error-message">{e}</div>
                })}
            </div>

            <div class="input-container">
                <textarea
                    placeholder="Type your message..."
                    prop:value=move || input.get()
                    on:input=move |ev| {
                        set_input.set(event_target_value(&ev));
                    }
                    on:keydown=send_on_enter
                    disabled=move || is_loading.get()
                />
                <button
                    class="send-btn"
                    on:click=send_on_click
                    disabled=move || is_loading.get() || input.get().trim().is_empty()
                >
                    {move || if is_loading.get() { "..." } else { "Send" }}
                </button>
            </div>
        </div>
    }
}

/// Response from IPC (matching CLI's MessageResponse)
#[derive(serde::Deserialize)]
struct MessageResponse {
    content: String,
}
