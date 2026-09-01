use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryEntry {
    pub start: String,
    pub end: String,
    pub model: String,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct HistoryDraft {
    pub start_time: DateTime<Local>,
    pub last_end_time: DateTime<Local>,
    pub model: String,
    pub text: String,
}

pub struct HistoryManager {
    pub enabled: bool,
    pub history_dir: PathBuf,
    current_draft: Option<HistoryDraft>,
}

impl HistoryManager {
    pub fn new(enabled: bool, history_dir: PathBuf) -> Self {
        if enabled {
            let _ = fs::create_dir_all(&history_dir);
        }
        Self {
            enabled,
            history_dir,
            current_draft: None,
        }
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        if self.enabled && !enabled {
            self.flush();
        }
        if enabled && !self.enabled {
            let _ = fs::create_dir_all(&self.history_dir);
        }
        self.enabled = enabled;
    }

    pub fn on_utterance(
        &mut self,
        start: DateTime<Local>,
        end: DateTime<Local>,
        text: &str,
        model_id: &str,
    ) {
        if !self.enabled {
            return;
        }
        let text = text.trim();
        if text.is_empty() {
            return;
        }

        if let Some(ref mut draft) = self.current_draft {
            let gap = (start - draft.last_end_time).num_seconds();
            if gap < 60 && draft.model == model_id {
                // Same history entry: append text and update last_end_time
                append_readable_text(&mut draft.text, text);
                draft.last_end_time = end;
                return;
            }
        }

        // Gap >= 60s, model changed, or no current draft: flush previous and start new entry
        self.flush();

        self.current_draft = Some(HistoryDraft {
            start_time: start,
            last_end_time: end,
            model: model_id.to_string(),
            text: text.to_string(),
        });
    }

    pub fn flush(&mut self) -> Option<HistoryEntry> {
        let draft = self.current_draft.take()?;
        let trimmed_text = draft.text.trim().to_string();
        if trimmed_text.is_empty() {
            return None;
        }

        let entry = HistoryEntry {
            start: draft.start_time.to_rfc3339(),
            end: draft.last_end_time.to_rfc3339(),
            model: draft.model,
            text: trimmed_text,
        };

        if let Err(err) = self.write_entry(&entry, draft.start_time) {
            eprintln!("[History] Warning: Failed to write history entry: {}", err);
        }

        Some(entry)
    }

    fn write_entry(&self, entry: &HistoryEntry, date: DateTime<Local>) -> Result<(), String> {
        let _ = fs::create_dir_all(&self.history_dir);
        let filename = format!("{}.jsonl", date.format("%Y-%m-%d"));
        let file_path = self.history_dir.join(filename);

        let json_line = serde_json::to_string(entry)
            .map_err(|e| format!("Serialization error: {}", e))?;

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file_path)
            .map_err(|e| format!("Failed to open {:?}: {}", file_path, e))?;

        writeln!(file, "{}", json_line)
            .map_err(|e| format!("Failed to write to {:?}: {}", file_path, e))?;

        println!("[History] Entry written to {:?}", file_path);
        Ok(())
    }

    pub fn current_draft(&self) -> Option<&HistoryDraft> {
        self.current_draft.as_ref()
    }
}

pub fn append_readable_text(buffer: &mut String, new_text: &str) {
    if buffer.is_empty() {
        buffer.push_str(new_text);
        return;
    }

    let last_char = buffer.chars().last().unwrap();
    let first_char = new_text.chars().next().unwrap();

    let is_cjk = |c: char| -> bool {
        ('\u{4E00}'..='\u{9FFF}').contains(&c)
            || ('\u{3400}'..='\u{4DBF}').contains(&c)
            || ('\u{3000}'..='\u{303F}').contains(&c)
            || ('\u{FF00}'..='\u{FFEF}').contains(&c)
    };

    if last_char.is_ascii_alphanumeric() && first_char.is_ascii_alphanumeric() {
        buffer.push(' ');
    } else if !is_cjk(last_char)
        && !is_cjk(first_char)
        && !last_char.is_whitespace()
        && !new_text.starts_with(|c: char| c.is_ascii_punctuation())
    {
        buffer.push(' ');
    }

    buffer.push_str(new_text);
}
