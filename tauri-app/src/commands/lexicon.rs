//! Lexicon JSONL read/write commands

use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

/// A single lexicon entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LexiconEntry {
    pub term: String,
    pub category: String,
    pub phonetic: Option<String>,
    pub examples: Vec<String>,
}

/// Get lexicon directory path
fn lexicon_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".CodeScribe").join("lexicon")
}

/// Get entries from a lexicon topic file
#[tauri::command]
pub fn get_lexicon_entries(topic: String) -> Result<Vec<LexiconEntry>, String> {
    let path = lexicon_dir().join(format!("{}.jsonl", topic));

    if !path.exists() {
        return Ok(vec![]);
    }

    let file = fs::File::open(&path).map_err(|e| e.to_string())?;
    let reader = BufReader::new(file);

    let mut entries = Vec::new();
    for line in reader.lines() {
        let line = line.map_err(|e| e.to_string())?;
        if line.trim().is_empty() {
            continue;
        }
        let entry: LexiconEntry = serde_json::from_str(&line).map_err(|e| e.to_string())?;
        entries.push(entry);
    }

    Ok(entries)
}

/// List available lexicon topics
#[tauri::command]
pub fn list_lexicon_topics() -> Result<Vec<String>, String> {
    let dir = lexicon_dir();

    if !dir.exists() {
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        return Ok(vec![]);
    }

    let mut topics = Vec::new();
    for entry in fs::read_dir(&dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.extension().map(|e| e == "jsonl").unwrap_or(false)
            && let Some(stem) = path.file_stem()
        {
            topics.push(stem.to_string_lossy().to_string());
        }
    }

    Ok(topics)
}

/// Save a lexicon entry (append to JSONL file)
#[tauri::command]
pub fn save_lexicon_entry(topic: String, entry: LexiconEntry) -> Result<(), String> {
    let dir = lexicon_dir();
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let path = dir.join(format!("{}.jsonl", topic));

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| e.to_string())?;

    let json = serde_json::to_string(&entry).map_err(|e| e.to_string())?;
    writeln!(file, "{}", json).map_err(|e| e.to_string())?;

    Ok(())
}
