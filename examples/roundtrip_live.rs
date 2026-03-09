//! Interactive Round-Trip Demo
//!
//! Speak into mic → Whisper → TTS → Speaker → Whisper again → Compare
//!
//! Usage:
//!   cargo run --release --example roundtrip_live
//!   cargo run --release --example roundtrip_live -- --text "Hello world"
//!
//! Created by M&K (c)2026 VetCoders

use anyhow::Result;
use std::io::{self, Write};
use std::time::Instant;

// ANSI colors
const CYAN: &str = "\x1b[36m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const MAGENTA: &str = "\x1b[35m";
const BLUE: &str = "\x1b[34m";
const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";

fn log_step(icon: &str, color: &str, label: &str, detail: &str) {
    println!("{}{}{} {}{}{}", color, icon, RESET, BOLD, label, RESET);
    if !detail.is_empty() {
        println!("   {}{}{}", DIM, detail, RESET);
    }
}

fn log_result(label: &str, value: &str) {
    println!("   {} {}{}{}", label, CYAN, value, RESET);
}

fn print_banner() {
    println!();
    println!(
        "{}╔═══════════════════════════════════════════════════════════╗{}",
        CYAN, RESET
    );
    println!(
        "{}║{}       🔄 Round-Trip Pipeline Demo                        {}{}║{}",
        CYAN, BOLD, RESET, CYAN, RESET
    );
    println!(
        "{}║{}   Text → TTS → Audio → STT → Text → Compare              {}{}║{}",
        CYAN, DIM, RESET, CYAN, RESET
    );
    println!(
        "{}╚═══════════════════════════════════════════════════════════╝{}",
        CYAN, RESET
    );
    println!();
}

fn print_separator() {
    println!(
        "{}───────────────────────────────────────────────────────────{}",
        DIM, RESET
    );
}

/// Calculate word overlap similarity
fn word_similarity(a: &str, b: &str) -> f32 {
    let a_lower = a.to_lowercase();
    let b_lower = b.to_lowercase();

    let a_words: std::collections::HashSet<&str> = a_lower
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|w| !w.is_empty())
        .collect();
    let b_words: std::collections::HashSet<&str> = b_lower
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|w| !w.is_empty())
        .collect();

    if a_words.is_empty() || b_words.is_empty() {
        return 0.0;
    }

    let intersection = a_words.intersection(&b_words).count();
    let union = a_words.union(&b_words).count();

    intersection as f32 / union as f32
}

fn run_roundtrip(input: &str, language: &str, play_audio: bool) -> Result<()> {
    print_separator();
    log_step("📝", BLUE, "INPUT", input);
    println!();

    // Step 1: TTS
    let start = Instant::now();
    log_step("🔊", YELLOW, "TTS: Generating speech...", "");

    let audio = codescribe_core::tts::synthesize(input)?;
    let tts_time = start.elapsed();

    let duration_sec = audio.len() as f32 / 24000.0;
    log_result(
        "Samples:",
        &format!("{} ({:.2}s @ 24kHz)", audio.len(), duration_sec),
    );
    log_result("Time:", &format!("{:.2}s", tts_time.as_secs_f32()));
    println!();

    // Step 2: Play audio (optional)
    if play_audio {
        log_step("🎵", MAGENTA, "Playing audio...", "");
        let player = codescribe_core::tts::AudioPlayer::new()?;
        player.play(&audio, 24000)?;
        println!();
    }

    // Step 3: STT
    let start = Instant::now();
    log_step("🎤", GREEN, "STT: Transcribing...", "");

    let transcribed = codescribe_core::stt::whisper::transcribe(&audio, 24000, Some(language))?;
    let stt_time = start.elapsed();

    log_result("Result:", transcribed.trim());
    log_result("Time:", &format!("{:.2}s", stt_time.as_secs_f32()));
    println!();

    // Step 4: Compare
    log_step("📊", CYAN, "COMPARISON", "");

    let word_sim = word_similarity(input, &transcribed);
    log_result("Word overlap:", &format!("{:.1}%", word_sim * 100.0));

    // Embedding similarity (if available)
    if codescribe_core::embedder::is_initialized() || codescribe_core::embedder::init().is_ok() {
        let emb_input = codescribe_core::embedder::embed(input)?;
        let emb_output = codescribe_core::embedder::embed(&transcribed)?;
        let emb_sim = codescribe_core::embedder::similarity(&emb_input, &emb_output);
        log_result("Semantic:", &format!("{:.1}%", emb_sim * 100.0));
    }

    // Verdict
    println!();
    if word_sim > 0.7 {
        println!(
            "   {}✅ EXCELLENT - Pipeline preserves meaning well{}",
            GREEN, RESET
        );
    } else if word_sim > 0.5 {
        println!(
            "   {}⚠️  GOOD - Some loss but understandable{}",
            YELLOW, RESET
        );
    } else {
        println!("   \x1b[31m❌ POOR - Significant meaning loss{}", RESET);
    }

    Ok(())
}

fn interactive_mode(language: &str, play_audio: bool) -> Result<()> {
    println!();
    println!(
        "{}Interactive mode - type text and press Enter{}",
        DIM, RESET
    );
    println!(
        "{}Commands: 'q' to quit, 'lang XX' to change language{}",
        DIM, RESET
    );
    println!();

    let mut current_lang = language.to_string();

    loop {
        print!("{}>{} ", CYAN, RESET);
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let input = input.trim();

        if input.is_empty() {
            continue;
        }

        if input == "q" || input == "quit" || input == "exit" {
            println!("{}Goodbye!{}", DIM, RESET);
            break;
        }

        if let Some(lang) = input.strip_prefix("lang ") {
            current_lang = lang.trim().to_string();
            println!("{}Language set to: {}{}", DIM, current_lang, RESET);
            continue;
        }

        if let Err(e) = run_roundtrip(input, &current_lang, play_audio) {
            eprintln!("\x1b[31mError: {}{}", e, RESET);
        }
        println!();
    }

    Ok(())
}

fn main() -> Result<()> {
    print_banner();

    // Parse args
    let args: Vec<String> = std::env::args().collect();
    let mut text: Option<String> = None;
    let mut language = "en".to_string();
    let mut play_audio = true;
    let mut interactive = false;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--text" | "-t" => {
                if i + 1 < args.len() {
                    text = Some(args[i + 1].clone());
                    i += 1;
                }
            }
            "--lang" | "-l" => {
                if i + 1 < args.len() {
                    language = args[i + 1].clone();
                    i += 1;
                }
            }
            "--no-play" => play_audio = false,
            "--interactive" | "-i" => interactive = true,
            "--help" | "-h" => {
                println!("Usage: roundtrip_live [OPTIONS]");
                println!();
                println!("Options:");
                println!("  -t, --text TEXT    Run single round-trip with TEXT");
                println!("  -l, --lang LANG    Language code (default: en)");
                println!("  -i, --interactive  Interactive mode (type text)");
                println!("  --no-play          Don't play audio");
                println!("  -h, --help         Show this help");
                println!();
                println!("Examples:");
                println!("  roundtrip_live --text \"Hello world\"");
                println!("  roundtrip_live --interactive --lang pl");
                return Ok(());
            }
            _ => {}
        }
        i += 1;
    }

    // Initialize models
    log_step("⚙️", DIM, "Initializing models...", "");

    let start = Instant::now();
    codescribe_core::tts::init()?;
    log_result(
        "TTS:",
        &format!("ready ({:.2}s)", start.elapsed().as_secs_f32()),
    );

    let start = Instant::now();
    codescribe_core::stt::whisper::init()?;
    log_result(
        "Whisper:",
        &format!("ready ({:.2}s)", start.elapsed().as_secs_f32()),
    );

    let start = Instant::now();
    if codescribe_core::embedder::init().is_ok() {
        log_result(
            "Embedder:",
            &format!("ready ({:.2}s)", start.elapsed().as_secs_f32()),
        );
    } else {
        log_result("Embedder:", "not available (word similarity only)");
    }
    println!();

    // Run mode
    if let Some(text) = text {
        run_roundtrip(&text, &language, play_audio)?;
    } else if interactive || text.is_none() {
        interactive_mode(&language, play_audio)?;
    }

    Ok(())
}
