use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::thread_store::Thread;

const INDEX_FILE_NAME: &str = "index.json";
const INDEX_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadIndexData {
    pub version: u32,
    pub threads: Vec<ThreadSummary>,
}

impl Default for ThreadIndexData {
    fn default() -> Self {
        Self {
            version: INDEX_VERSION,
            threads: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadSummary {
    pub id: String,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub message_count: usize,
    pub mode: String,
    pub tags: Vec<String>,
    pub summary: Option<String>,
    pub has_notes: bool,
    pub is_favorite: bool,
}

impl ThreadSummary {
    fn from_thread(thread: &Thread, is_favorite: bool) -> Self {
        Self {
            id: thread.id.clone(),
            title: thread.title.clone(),
            created_at: thread.created_at,
            updated_at: thread.updated_at,
            message_count: thread.messages.len(),
            mode: thread.mode.clone(),
            tags: thread.tags.clone(),
            summary: thread.summary.clone(),
            has_notes: !thread.notes.is_empty(),
            is_favorite,
        }
    }

    fn searchable_text(&self) -> String {
        let mut out = String::with_capacity(
            self.title.len()
                + self.mode.len()
                + self.tags.iter().map(String::len).sum::<usize>()
                + self.summary.as_ref().map_or(0, String::len)
                + 8,
        );

        out.push_str(&self.title.to_ascii_lowercase());
        out.push(' ');
        out.push_str(&self.mode.to_ascii_lowercase());
        out.push(' ');
        for tag in &self.tags {
            out.push_str(&tag.to_ascii_lowercase());
            out.push(' ');
        }
        if let Some(summary) = &self.summary {
            out.push_str(&summary.to_ascii_lowercase());
        }
        out
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThreadFilter {
    pub mode: Option<String>,
    pub favorites_only: bool,
    pub has_notes: bool,
    pub tag: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ThreadIndex {
    path: PathBuf,
    data: ThreadIndexData,
}

impl ThreadIndex {
    pub fn load_or_create(threads_dir: &Path) -> Result<Self> {
        fs::create_dir_all(threads_dir).with_context(|| {
            format!(
                "Failed to create threads directory: {}",
                threads_dir.display()
            )
        })?;

        let path = threads_dir.join(INDEX_FILE_NAME);
        if path.exists() {
            let raw = fs::read_to_string(&path) // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path
                .with_context(|| format!("Failed to read thread index: {}", path.display()))?;
            let data = serde_json::from_str::<ThreadIndexData>(&raw)
                .with_context(|| format!("Failed to parse thread index: {}", path.display()))?;
            return Ok(Self { path, data });
        }

        let index = Self {
            path,
            data: ThreadIndexData::default(),
        };
        index.save()?;
        Ok(index)
    }

    pub fn add(&mut self, thread: &Thread) -> Result<()> {
        match self
            .data
            .threads
            .iter_mut()
            .find(|summary| summary.id == thread.id)
        {
            Some(existing) => {
                let is_favorite = existing.is_favorite;
                *existing = ThreadSummary::from_thread(thread, is_favorite);
            }
            None => self
                .data
                .threads
                .push(ThreadSummary::from_thread(thread, false)),
        }
        sort_by_updated_desc(&mut self.data.threads);
        self.save()
    }

    pub fn remove(&mut self, id: &str) -> Result<()> {
        self.data.threads.retain(|summary| summary.id != id);
        self.save()
    }

    pub fn list(&self, filter: Option<&ThreadFilter>) -> Vec<&ThreadSummary> {
        let mut entries = self
            .data
            .threads
            .iter()
            .filter(|summary| filter.is_none_or(|f| matches_filter(summary, f)))
            .collect::<Vec<_>>();
        entries.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        entries
    }

    pub fn search(&self, query: &str) -> Vec<&ThreadSummary> {
        let terms = normalize_terms(query);
        if terms.is_empty() {
            return self.list(None);
        }

        let mut entries = self
            .data
            .threads
            .iter()
            .filter(|summary| {
                let haystack = summary.searchable_text();
                terms.iter().all(|term| haystack.contains(term))
            })
            .collect::<Vec<_>>();
        entries.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        entries
    }

    pub fn save(&self) -> Result<()> {
        let data =
            serde_json::to_vec_pretty(&self.data).context("Failed to serialize thread index")?;
        atomic_write(&self.path, &data)
    }

    pub fn data(&self) -> &ThreadIndexData {
        &self.data
    }
}

fn sort_by_updated_desc(entries: &mut [ThreadSummary]) {
    entries.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
}

fn normalize_terms(query: &str) -> Vec<String> {
    query
        .to_ascii_lowercase()
        .split_whitespace()
        .filter(|part| !part.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn matches_filter(summary: &ThreadSummary, filter: &ThreadFilter) -> bool {
    if let Some(mode) = &filter.mode
        && !summary.mode.eq_ignore_ascii_case(mode)
    {
        return false;
    }

    if filter.favorites_only && !summary.is_favorite {
        return false;
    }

    if filter.has_notes && !summary.has_notes {
        return false;
    }

    if let Some(tag) = &filter.tag
        && !summary
            .tags
            .iter()
            .any(|value| value.eq_ignore_ascii_case(tag))
    {
        return false;
    }

    true
}

fn atomic_write(path: &Path, data: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create parent directory for {}", path.display()))?;
    }

    let tmp = path.with_extension("tmp");
    fs::write(&tmp, data)
        .with_context(|| format!("Failed to write temporary file {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| {
        format!(
            "Failed to atomically rename {} -> {}",
            tmp.display(),
            path.display()
        )
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use chrono::Duration;
    use serde_json::json;
    use tempfile::TempDir;

    use super::*;
    use crate::agent::thread_store::{Thread, ThreadMessage, TokenUsage};

    fn sample_thread(
        id: &str,
        title: &str,
        summary: Option<&str>,
        mode: &str,
        minutes_ago: i64,
    ) -> Thread {
        let updated_at = Utc::now() - Duration::minutes(minutes_ago);
        Thread {
            id: id.to_string(),
            created_at: updated_at - Duration::minutes(5),
            updated_at,
            title: title.to_string(),
            mode: mode.to_string(),
            tags: vec!["vet".to_string(), "urgent".to_string()],
            notes: Vec::new(),
            messages: vec![ThreadMessage {
                role: "user".to_string(),
                content: vec![json!({"type":"input_text","text":"hello"})],
                timestamp: updated_at,
                metadata: None,
            }],
            summary: summary.map(ToOwned::to_owned),
            total_tokens: Some(TokenUsage {
                input: 10,
                output: 20,
            }),
            provider: "openai".to_string(),
            model: "gpt-5".to_string(),
        }
    }

    #[test]
    fn search_matches_all_words_and_sorts_by_latest() -> Result<()> {
        let tmp = TempDir::new()?;
        let mut index = ThreadIndex::load_or_create(tmp.path())?;

        let first = sample_thread(
            "t_2026-02-22_aaaaaa",
            "Cat urgent follow-up",
            Some("Prednisone taper plan"),
            "assistive",
            30,
        );
        let second = sample_thread(
            "t_2026-02-23_bbbbbb",
            "Urgent dermatology handoff",
            Some("Cat allergy response"),
            "assistive",
            5,
        );
        let third = sample_thread(
            "t_2026-02-23_cccccc",
            "Billing question",
            Some("No clinical content"),
            "toggle",
            1,
        );

        index.add(&first)?;
        index.add(&second)?;
        index.add(&third)?;

        let results = index.search("cat urgent");
        let ids = results
            .iter()
            .map(|summary| summary.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["t_2026-02-23_bbbbbb", "t_2026-02-22_aaaaaa"]);

        Ok(())
    }

    #[test]
    fn list_applies_filters() -> Result<()> {
        let tmp = TempDir::new()?;
        let mut index = ThreadIndex::load_or_create(tmp.path())?;

        let mut a = sample_thread(
            "t_2026-02-23_filter1",
            "Case A",
            Some("alpha"),
            "assistive",
            10,
        );
        a.notes.push(crate::agent::thread_store::ThreadNote {
            id: "n_1".to_string(),
            created_at: Utc::now(),
            text: "Pinned".to_string(),
            anchored_to_message: Some(0),
        });

        let b = sample_thread("t_2026-02-23_filter2", "Case B", Some("beta"), "toggle", 2);

        index.add(&a)?;
        index.add(&b)?;

        if let Some(entry) = index
            .data
            .threads
            .iter_mut()
            .find(|summary| summary.id == "t_2026-02-23_filter1")
        {
            entry.is_favorite = true;
        }
        index.save()?;

        let filter = ThreadFilter {
            mode: Some("assistive".to_string()),
            favorites_only: true,
            has_notes: true,
            tag: Some("urgent".to_string()),
        };
        let results = index.list(Some(&filter));
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "t_2026-02-23_filter1");

        Ok(())
    }
}
