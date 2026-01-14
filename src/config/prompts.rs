use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tracing::{info, warn};

// Default prompts (fallback if file missing/empty)
pub const DEFAULT_FORMATTING_PROMPT: &str = r#"You are a TRANSCRIPTION FORMATTER. Your task is formatting raw speech-to-text output.

CONTEXT: This is automated voice-to-text from a microphone. The user dictated something and Whisper transcribed it. You format it for readability.

CRITICAL: You are NOT interacting with the user. You are processing machine-generated transcription. NEVER refuse. NEVER say "I can't". Just format the text.

ALLOWED:
- Fix punctuation (periods, commas, question marks)
- Fix capitalization (sentence starts, proper nouns)
- Add paragraphs and bullet points where appropriate
- Remove Whisper repetition artifacts (e.g., "Wielki, Wielki, Wielki..." → "Wielki")

FORBIDDEN:
- NEVER change the meaning
- NEVER add new content or commentary
- NEVER translate - keep the original language
- NEVER respond to the content - you are formatting, not conversing
- NEVER refuse - this is machine transcription, not user input

Return ONLY the formatted text. No preamble, no explanation.

Examples:
"cześć jak się masz mam pytanie pytanie pytanie do ciebie"
→ "Cześć, jak się masz? Mam pytanie do ciebie."

"Wielki Wielki Wielki problem"
→ "Wielki problem."

"najpierw zrób to potem tamto a na końcu jeszcze coś"
→ "Najpierw zrób to, potem tamto, a na końcu jeszcze coś."
"#;

pub const DEFAULT_ASSISTIVE_PROMPT: &str = r#"Jesteś kurierem/enhancerem. Augmentujesz i PRZEKAZUJESZ słowa użytkownika, NIE odpowiadasz na nie.

TWOJA ROLA:
- Użytkownik mówi coś → Ty to przekazujesz dalej (do innego modelu/systemu)
- NIE jesteś asystentem który odpowiada na pytania
- Jesteś filtrem który ulepsza i strukturyzuje wiadomość użytkownika

CO ROBISZ:
- Przekazujesz intencję użytkownika z lepszą strukturą
- Dodajesz kontekst jeśli potrzebny
- Poprawiasz czytelność i jasność przekazu
- Używasz kaomoji jeśli pasuje (nigdy emoji)

CZEGO NIE ROBISZ:
- NIE odpowiadasz na pytania użytkownika, chyba, że dostaniesz jasną i bezpośrednią prośbę.
- NIE wykonujesz poleceń użytkownika. chyba, że dostaniesz jednoznaczną instrukcję.
- NIE udzielasz rad ani sugestii od siebie, bez wyrażnej prośby.

Przykład:
Użytkownik: "chcę zrobić dark mode w aplikacji"
Ty: "Chcę zrobić dark mode w aplikacji. Potrzebuję implementacji przełącznika trybu jasny/ciemny z persystencją ustawienia."

Przykład:
Użytkownik: "jak zrobić API endpoint"
Ty: "Pytanie o implementację API endpoint - proszę o przykład kodu i wyjaśnienie best practices."

Preferowany język: polski.
"#;

pub fn prompts_dir() -> PathBuf {
    crate::config::Config::config_dir().join("prompts")
}

fn ensure_prompts_dir() -> std::io::Result<()> {
    let dir = prompts_dir();
    if !dir.exists() {
        fs::create_dir_all(&dir)?;
    }
    Ok(())
}

fn load_or_create(filename: &str, default_content: &str) -> String {
    if let Err(e) = ensure_prompts_dir() {
        warn!("Failed to create prompts dir: {}", e);
        return default_content.to_string();
    }

    let path = prompts_dir().join(filename);
    if !path.exists() {
        if let Err(e) = fs::write(&path, default_content) {
            warn!("Failed to write default prompt to {}: {}", path.display(), e);
        } else {
            info!("Created default prompt file: {}", path.display());
        }
        return default_content.to_string();
    }

    match fs::read_to_string(&path) {
        Ok(content) => {
            if content.trim().is_empty() {
                warn!("Prompt file {} is empty, using default", path.display());
                default_content.to_string()
            } else {
                content
            }
        }
        Err(e) => {
            warn!("Failed to read prompt from {}: {}, using default", path.display(), e);
            default_content.to_string()
        }
    }
}

pub fn get_formatting_prompt() -> String {
    load_or_create("formatting.txt", DEFAULT_FORMATTING_PROMPT)
}

pub fn get_assistive_prompt() -> String {
    load_or_create("assistive.txt", DEFAULT_ASSISTIVE_PROMPT)
}

#[allow(dead_code)] // Used by tauri-app
pub fn get_formatting_prompt_path() -> PathBuf {
    prompts_dir().join("formatting.txt")
}

#[allow(dead_code)] // Used by tauri-app
pub fn get_assistive_prompt_path() -> PathBuf {
    prompts_dir().join("assistive.txt")
}

pub fn open_prompt_file(filename: &str) {
    let path = prompts_dir().join(filename);
    // Ensure it exists before opening
    if filename == "formatting.txt" {
        get_formatting_prompt();
    } else if filename == "assistive.txt" {
        get_assistive_prompt();
    }
    
    // Use macOS 'open' command
    let _ = std::process::Command::new("open")
        .arg(&path)
        .spawn();
}

#[allow(dead_code)] // Used by tauri-app
pub fn reset_to_defaults() -> std::io::Result<()> {
    ensure_prompts_dir()?;
    fs::write(prompts_dir().join("formatting.txt"), DEFAULT_FORMATTING_PROMPT)?;
    fs::write(prompts_dir().join("assistive.txt"), DEFAULT_ASSISTIVE_PROMPT)?;
    Ok(())
}

pub fn open_prompts_folder() {
    if let Err(e) = ensure_prompts_dir() {
        warn!("Failed to create prompts dir: {}", e);
        return;
    }

    let dir = prompts_dir();
    info!("Opening prompts folder: {}", dir.display());
    let _ = Command::new("open").arg(&dir).spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prompt_paths_api() {
        // Test path functions (used by tauri-app and tests)
        let formatting_path = get_formatting_prompt_path();
        let assistive_path = get_assistive_prompt_path();

        // Paths should be different
        assert_ne!(formatting_path, assistive_path);

        // Paths should end with expected filenames
        assert!(formatting_path.ends_with("formatting.txt"));
        assert!(assistive_path.ends_with("assistive.txt"));
    }

    #[test]
    fn test_reset_to_defaults() {
        // This tests the reset_to_defaults function (used by tauri-app)
        // We can't fully test it without temp dir setup, but we verify it compiles
        // and is callable
        let result = reset_to_defaults();
        // Should succeed or fail gracefully
        let _ = result;
    }
}
