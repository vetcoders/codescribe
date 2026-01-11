use leptos::prelude::*;

#[derive(Clone)]
struct LexiconEntry {
    term: String,
    mispronunciations: Vec<String>,
}

#[derive(Clone)]
#[allow(dead_code)] // Fields will be used when backend integration is complete
struct CalibrationRun {
    sentence: String,
    transcript: String,
    wer: f32,
}

#[component]
pub fn TeacherView() -> impl IntoView {
    let (topic, set_topic) = signal("general".to_string());
    let (reference, set_reference) = signal(String::new());
    let (transcript, set_transcript) = signal(String::new());
    let (is_recording, set_is_recording) = signal(false);
    let (is_learning, set_is_learning) = signal(false);
    let (status, set_status) = signal(String::new());
    let (status_log, set_status_log) = signal(Vec::<String>::new());
    let (lexicon_preview, set_lexicon_preview) = signal(Vec::<LexiconEntry>::new());
    let (lexicon_count, set_lexicon_count) = signal(0usize);
    let (sentences, set_sentences) = signal(Vec::<String>::new());
    let (wizard_index, set_wizard_index) = signal(0usize);
    let (calibration_runs, set_calibration_runs) = signal(Vec::<CalibrationRun>::new());

    let push_status = move |msg: &str, kind: &str| {
        let line = format!("[{}] {}", kind, msg);
        set_status.set(line.clone());
        let mut log = status_log.get();
        log.insert(0, line);
        log.truncate(12);
        set_status_log.set(log);
    };

    let avg_wer = move || {
        let runs = calibration_runs.get();
        if runs.is_empty() {
            return None;
        }
        let sum: f32 = runs.iter().map(|r| r.wer).sum();
        Some((sum / runs.len() as f32) * 100.0)
    };

    let copy_to_ref = move |text: String| {
        set_reference.set(text);
        set_transcript.set(String::new());
        push_status("Reference set. Click Record and read it aloud.", "info");
    };

    let next_wizard = move |_: web_sys::MouseEvent| {
        let sents = sentences.get();
        if sents.is_empty() {
            return;
        }
        let current = wizard_index.get();
        let next = (current + 1).min(sents.len() - 1);
        set_wizard_index.set(next);
        if let Some(s) = sents.get(next) {
            copy_to_ref(s.clone());
        }
        push_status(&format!("Sentence {}/{}", next + 1, sents.len()), "info");
    };

    let toggle_recording = move |_: web_sys::MouseEvent| {
        if is_recording.get() {
            set_is_recording.set(false);
            push_status("Recording stopped. Review transcript and click Learn.", "info");
            // In real implementation, would get transcript from audio stream
            set_transcript.set("(Transcript would appear here after recording)".to_string());
        } else {
            set_transcript.set(String::new());
            set_is_recording.set(true);
            push_status("Recording... Read the reference text aloud.", "info");
        }
    };

    let handle_learn = move |_: web_sys::MouseEvent| {
        let ref_text = reference.get();
        if ref_text.is_empty() {
            push_status("Please enter reference text first.", "err");
            return;
        }

        set_is_learning.set(true);
        push_status("Analyzing errors...", "info");

        // Simulate learning (placeholder)
        leptos::task::spawn_local(async move {
            gloo_timers::future::TimeoutFuture::new(500).await;

            // Add a mock calibration run
            let mut runs = calibration_runs.get();
            runs.insert(0, CalibrationRun {
                sentence: reference.get(),
                transcript: transcript.get(),
                wer: 0.15, // Mock 15% WER
            });
            runs.truncate(50);
            set_calibration_runs.set(runs);

            // Add mock lexicon entry
            let mut lex = lexicon_preview.get();
            lex.push(LexiconEntry {
                term: "(example term)".to_string(),
                mispronunciations: vec!["(example)".to_string()],
            });
            set_lexicon_preview.set(lex);
            set_lexicon_count.update(|c| *c += 1);

            set_is_learning.set(false);
            push_status("Learned new terms. (Backend integration coming soon)", "info");
        });
    };

    let generate_sentences = move |_: web_sys::MouseEvent| {
        push_status("Generating sentences...", "info");
        // Mock sentences for now
        let mock_sentences = vec![
            format!("This is a sample sentence about {}.", topic.get()),
            format!("Another example related to {} topic.", topic.get()),
            format!("Practice reading this {} text clearly.", topic.get()),
        ];
        set_sentences.set(mock_sentences);
        set_wizard_index.set(0);
        push_status("Generated 3 sample sentences. (Backend integration coming soon)", "info");
    };

    let clear_lexicon = move |_: web_sys::MouseEvent| {
        set_lexicon_preview.set(Vec::new());
        set_lexicon_count.set(0);
        push_status("Lexicon cleared.", "info");
    };

    view! {
        <div class="teacher-view">
            <h2>"🎓 The Teacher (Active Learning)"</h2>

            <section class="vista-panel">
                <h3>"Topic & Controls"</h3>

                <div class="form-row">
                    <label>"Topic:"</label>
                    <input
                        class="input"
                        prop:value=move || topic.get()
                        on:input=move |ev| set_topic.set(event_target_value(&ev))
                        placeholder="e.g. veterinary, programming, cooking"
                    />
                </div>

                <div class="controls row" style="margin-top: 12px;">
                    <button class="secondary" on:click=generate_sentences>
                        "Generate Set"
                    </button>
                    <button class="secondary" on:click=clear_lexicon>
                        "Clear Lexicon"
                    </button>
                </div>
            </section>

            <Show when=move || !sentences.get().is_empty()>
                <section class="vista-panel calibration-set">
                    <div class="flex-between">
                        <h3>{move || format!("Calibration Sentences ({})", sentences.get().len())}</h3>
                        <div class="wizard-meta">
                            <span>{move || format!("Step {} / {}", wizard_index.get() + 1, sentences.get().len())}</span>
                            <button class="secondary sm-btn" on:click=next_wizard>"Next ▶"</button>
                        </div>
                    </div>
                    <ul class="sentence-list">
                        <For
                            each=move || sentences.get()
                            key=|s| s.clone()
                            children=move |sentence| {
                                let s = sentence.clone();
                                let s2 = sentence.clone();
                                view! {
                                    <li>
                                        <button
                                            class="icon-btn"
                                            on:click=move |_: web_sys::MouseEvent| copy_to_ref(s.clone())
                                        >
                                            "📋"
                                        </button>
                                        {s2}
                                    </li>
                                }
                            }
                        />
                    </ul>
                </section>
            </Show>

            <Show when=move || !calibration_runs.get().is_empty()>
                <section class="vista-panel metrics-card">
                    <div class="flex-between">
                        <strong>"Calibration Metrics"</strong>
                    </div>
                    <div class="metric-line">
                        {move || {
                            let wer = avg_wer();
                            let runs = calibration_runs.get().len();
                            format!("Avg WER: {:.1}% (runs: {})", wer.unwrap_or(0.0), runs)
                        }}
                    </div>
                </section>
            </Show>

            <section class="vista-panel">
                <div class="record-section">
                    <button
                        class=move || if is_recording.get() { "record-btn recording" } else { "record-btn" }
                        disabled=move || is_learning.get()
                        on:click=toggle_recording
                    >
                        {move || if is_recording.get() { "⏹️ Stop Recording" } else { "🎙️ Record" }}
                    </button>
                    <Show when=move || is_recording.get()>
                        <span class="recording-indicator">"● REC"</span>
                    </Show>
                </div>

                <div class="split-view">
                    <div class="half">
                        <label>"Reference Text (What you said):"</label>
                        <textarea
                            class="input"
                            rows="5"
                            placeholder="Paste correct text here..."
                            prop:value=move || reference.get()
                            on:input=move |ev| set_reference.set(event_target_value(&ev))
                        ></textarea>
                    </div>
                    <div class="half">
                        <label>"Transcript (What Whisper heard):"</label>
                        <textarea
                            class="input"
                            rows="5"
                            placeholder="Waiting for transcript..."
                            prop:value=move || transcript.get()
                            on:input=move |ev| set_transcript.set(event_target_value(&ev))
                        ></textarea>
                    </div>
                </div>

                <div class="actions row" style="margin-top: 12px;">
                    <button
                        class="primary"
                        disabled=move || is_learning.get() || reference.get().is_empty()
                        on:click=handle_learn
                    >
                        {move || if is_learning.get() { "Learning..." } else { "🧠 Fix & Learn" }}
                    </button>
                    <span class="status muted">{move || status.get()}</span>
                </div>
            </section>

            <section class="vista-panel lexicon-preview">
                <div class="flex-between">
                    <strong>{move || format!("Lexicon Preview ({} entries)", lexicon_count.get())}</strong>
                </div>
                <Show when=move || lexicon_preview.get().is_empty()>
                    <p class="muted">"No lexicon entries yet. Use Fix & Learn to add terms."</p>
                </Show>
                <ul>
                    <For
                        each=move || lexicon_preview.get()
                        key=|e| e.term.clone()
                        children=move |entry| view! {
                            <li>
                                <code>{entry.term.clone()}</code>
                                " ← "
                                {entry.mispronunciations.join(", ")}
                            </li>
                        }
                    />
                </ul>
            </section>

            <section class="vista-panel status-log">
                <strong>"Log"</strong>
                <ul>
                    <For
                        each=move || status_log.get()
                        key=|s| s.clone()
                        children=move |line| view! { <li>{line}</li> }
                    />
                </ul>
                <Show when=move || status_log.get().is_empty()>
                    <p class="muted">"No log entries yet."</p>
                </Show>
            </section>
        </div>
    }
}
