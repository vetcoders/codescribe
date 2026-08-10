//! Calibrate the formatting semantic-guard floor on the real embedder.
//!
//! The lexicon-gate calibration (2026-08-09) proved thresholds must come from
//! measurement, not intuition — the intuitive 0.70 there would have killed
//! 1279 of 2311 good rules. Same discipline here before wiring
//! `semantic_divergence` into the quality gate: measure what MiniLM says about
//! meaning-preserving formatting versus meaning-changing rewrites, then pick a
//! floor with clear daylight between the populations.
//!
//! Run: `cargo run -p codescribe-core --example semantic_guard_calibration`
//! (needs the MiniLM snapshot in the HF cache — the same one the build embeds).

fn main() -> anyhow::Result<()> {
    // (label, raw, formatted, meaning_kept)
    let pairs: &[(&str, &str, &str, bool)] = &[
        (
            "punctuation+caps only",
            "dobra zróbmy tak najpierw sprawdź ten plik potem zobacz testy i nie rozwal scope",
            "Dobra, zróbmy tak: najpierw sprawdź ten plik, potem zobacz testy — i nie rozwal scope.",
            true,
        ),
        (
            "paragraphs + fillers removed",
            "no więc eee chciałbym żebyś yyy najpierw zrobił backup a potem dopiero migrację bo jak coś pójdzie nie tak to musimy mieć powrót",
            "Chciałbym, żebyś najpierw zrobił backup, a potem dopiero migrację. Jeśli coś pójdzie nie tak, musimy mieć powrót.",
            true,
        ),
        (
            "medical dictation cleanup",
            "pacjent waży cztery i pół kilograma podałem meloksykam zero przecinek dwa mililitra podskórnie kontrola za tydzień",
            "Pacjent waży 4,5 kg. Podałem meloksykam 0,2 ml podskórnie. Kontrola za tydzień.",
            true,
        ),
        (
            "english formatting only",
            "ok so the deploy failed twice check the logs first then restart the service",
            "OK, so the deploy failed twice. Check the logs first, then restart the service.",
            true,
        ),
        (
            "negation dropped",
            "nie wysyłaj tego maila do klienta dopóki nie potwierdzę treści",
            "Wyślij tego maila do klienta. Potwierdzę treść.",
            false,
        ),
        (
            "instruction inverted",
            "usuń stary backup dopiero po zweryfikowaniu nowego",
            "Usuń nowy backup po zweryfikowaniu starego.",
            false,
        ),
        (
            "hallucinated content",
            "spotkanie przenosimy na czwartek",
            "Spotkanie przenosimy na czwartek o 15:00 w biurze przy Głównej. Proszę potwierdzić obecność i zabrać laptopy.",
            false,
        ),
        (
            "topic replaced",
            "zamów proszę odczynniki do laboratorium na przyszły tydzień",
            "Faktura za odczynniki została opłacona w zeszłym miesiącu.",
            false,
        ),
    ];

    let texts: Vec<&str> = pairs
        .iter()
        .flat_map(|(_, raw, formatted, _)| [*raw, *formatted])
        .collect();
    let embeddings = codescribe_core::embedder::embed_batch(&texts)?;

    let mut kept_min = f32::MAX;
    let mut changed_max = f32::MIN;
    println!("{:<28} {:>8}  meaning_kept", "pair", "cosine");
    for (i, (label, _, _, kept)) in pairs.iter().enumerate() {
        let cosine =
            codescribe_core::embedder::similarity(&embeddings[i * 2], &embeddings[i * 2 + 1]);
        println!("{label:<28} {cosine:>8.3}  {kept}");
        if *kept {
            kept_min = kept_min.min(cosine);
        } else {
            changed_max = changed_max.max(cosine);
        }
    }
    println!();
    println!("meaning-kept minimum:    {kept_min:.3}");
    println!("meaning-changed maximum: {changed_max:.3}");
    println!("daylight:                {:.3}", kept_min - changed_max);
    Ok(())
}
