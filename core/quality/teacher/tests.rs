//! Proof fixtures: e2e_overlay_delivery_parity 01_no-to-dobra (candle run).

use super::*;

/// Live assembly from real e2e run (01_no-to-dobra.wav, candle live path).
const LIVE: &str = "Teraz po parę słów, korzystając z surowej transkrypcji przez Codescribe mamy już pierwsze słowo do analizy wobec czego chcę, żebyś za chwilę, żebyś wziął ten plik WAV i puść i przychodził go na nasz endpoint. bo muszę mieć pewność czy leksykon działa Więc po prostu po to, aby Duże bazy kodowe przestały być tajemnicą Czarną, dziurą, na agentu veiłej. korzysta z rusta w wersji Toolchain 2024";

/// Whisper final-pass from the same run.
const WHISPER: &str = "No to dobra, teraz generalnie powiem parę słów korzystając z surowej transkrypcji przez Codescribe. Mamy już pierwsze słowo do analizy, wobec czego chcę, żebyś wziął ten blik Wave i puścił go na... nasz endpoint, bo muszę mieć pewność czy leksykon działa, więc po prostu na temu biuz dupy. LOCK3 to aplikacja stworzona po to aby duże bazy kodowe przestały być tajemnicą i czarną dziurą dla agentów AI. Korzystam z Rust w wersji Tooltrain 2024. Dziękuję.";

/// Human reference for 01_no-to-dobra.
const HUMAN: &str = "No to dobra. Teraz generalnie powiem parę słów korzystając z surowej transkrypcji przez Codescribe (mamy już pierwsze słowo do analizy), do czego chcę, żebyś, nie wiem, żebyś wziął ten plik WAV i puścił go na nasz endpoint, bo muszę mieć pewność, hmmm, czy leksykon działa więc po [(niewyraźnie)]. Loctree to aplikacja stworzona po to, aby duże bazy kodowe przestały być tajemnicą i czarną dziurą dla agentów AI. Korzysta z Rusta, w wersji Toolchain 2024.";

#[test]
fn proof_e2e_no_to_dobra_emits_needs_attention_and_lexicon_hints() {
    let report = teach(TeacherInput {
        live: LIVE.into(),
        whisper: WHISPER.into(),
        human: Some(HUMAN.into()),
        label: Some("01_no-to-dobra e2e candle".into()),
    });

    assert!(
        !report.attention.is_empty(),
        "must surface Needs attention spans"
    );
    assert!(
        report.whisper_errors_vs_human > 0,
        "Whisper must disagree with human somewhere"
    );

    // Classic jargon hallucinations / mishears.
    let hints_joined: String = report
        .lexicon_hints
        .iter()
        .map(|h| {
            format!(
                "{}->{}",
                h.from_whisper.to_lowercase(),
                h.to_human.to_lowercase()
            )
        })
        .collect::<Vec<_>>()
        .join(" | ");
    eprintln!("lexicon hints: {hints_joined}");
    eprintln!("thesis: {}", report.thesis_summary);
    eprintln!(
        "hit-rate: {:?} ({}/{})",
        report.gap_hallucination_hit_rate,
        report.whisper_errors_at_live_weak,
        report.whisper_errors_vs_human
    );

    // At least one of the known petarda pairs should appear.
    let has_jargon_hint = report.lexicon_hints.iter().any(|h| {
        let f = h.from_whisper.to_lowercase();
        let t = h.to_human.to_lowercase();
        (f.contains("lock") && t.contains("loctree"))
            || (f.contains("blik") && t.contains("wav"))
            || (f.contains("wave") && t.contains("wav"))
            || (f.contains("tooltrain") && t.contains("toolchain"))
            || (t.contains("loctree") || t.contains("toolchain") || t.contains("wav"))
    });
    assert!(
        has_jargon_hint
            || report.attention.iter().any(|a| {
                matches!(
                    a.kind,
                    AttentionKind::WhisperErrorAtLiveWeakness | AttentionKind::WhisperExcess
                ) && (a.whisper_tokens.iter().any(|w| {
                    let l = w.to_lowercase();
                    l.contains("lock")
                        || l.contains("blik")
                        || l.contains("tooltrain")
                        || l.contains("wave")
                }) || a.note.to_lowercase().contains("lock")
                    || a.note.to_lowercase().contains("toolchain"))
            }),
        "expected LOCK3/blik/Tooltrain-class attention; hints={hints_joined}"
    );

    // Thesis score: we expect a non-trivial co-location rate on this fixture.
    if let Some(rate) = report.gap_hallucination_hit_rate {
        assert!(
            rate >= 0.3,
            "expected gap≡halu hit-rate ≥ 30% on this fixture, got {rate:.2}"
        );
    }
}

#[test]
fn synthetic_apple_gap_whisper_fill_is_flagged() {
    // Apple omits jargon; Whisper invents a lookalike in the hole.
    let live = "podajemy lek w dawce pieciu miligramow";
    let whisper = "podajemy amoksycyline LOCK3 w dawce pieciu miligramow";
    let human = "podajemy amoksycyline Loctree w dawce pieciu miligramow";

    let report = teach(TeacherInput {
        live: live.into(),
        whisper: whisper.into(),
        human: Some(human.into()),
        label: Some("synthetic gap".into()),
    });

    assert!(
        report.attention.iter().any(|a| matches!(
            a.kind,
            AttentionKind::WhisperExcess | AttentionKind::WhisperErrorAtLiveWeakness
        ) || a
            .whisper_tokens
            .iter()
            .any(|t| t.to_lowercase().contains("lock"))),
        "Whisper fill into live gap must be Needs attention"
    );
    assert!(
        report
            .lexicon_hints
            .iter()
            .any(|h| h.from_whisper.to_lowercase().contains("lock")
                && h.to_human.to_lowercase().contains("loctree"))
            || report
                .attention
                .iter()
                .any(|a| a.note.to_lowercase().contains("lock")),
        "lexicon should want LOCK3 → Loctree"
    );
}

#[test]
fn html_report_is_self_contained() {
    let report = teach(TeacherInput {
        live: LIVE.into(),
        whisper: WHISPER.into(),
        human: Some(HUMAN.into()),
        label: Some("html".into()),
    });
    let html = report::report_to_html(&report);
    assert!(html.contains("<!DOCTYPE html>"));
    assert!(html.contains("Needs attention"));
    assert!(html.contains("Thesis"));
}
