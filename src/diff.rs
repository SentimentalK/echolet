#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffAction {
    pub backspaces: usize,
    pub new_suffix: String,
}

#[derive(Default)]
pub struct PartialSession {
    last_partial: Vec<char>,
}

impl PartialSession {
    pub fn new() -> Self {
        Self {
            last_partial: Vec::new(),
        }
    }

    /// Compute the diff between last typed partial and new partial text for the current utterance.
    pub fn update(&mut self, current_text: &str) -> Option<DiffAction> {
        let curr_chars: Vec<char> = current_text.chars().collect();

        // If the text hasn't changed at all, no action needed
        if curr_chars == self.last_partial {
            return None;
        }

        // Find longest common prefix
        let mut common_prefix_len = 0;
        while common_prefix_len < self.last_partial.len()
            && common_prefix_len < curr_chars.len()
            && self.last_partial[common_prefix_len] == curr_chars[common_prefix_len]
        {
            common_prefix_len += 1;
        }

        let backspaces = self.last_partial.len() - common_prefix_len;
        let new_suffix: String = curr_chars[common_prefix_len..].iter().collect();

        self.last_partial = curr_chars;

        Some(DiffAction {
            backspaces,
            new_suffix,
        })
    }

    /// Finalize current utterance (e.g. on endpoint or utterance stop).
    /// This commits the current text so future diffs never delete it.
    pub fn finalize(&mut self) {
        self.last_partial.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_diff_append() {
        let mut session = PartialSession::new();

        let a1 = session.update("昨天").unwrap();
        assert_eq!(a1, DiffAction { backspaces: 0, new_suffix: "昨天".to_string() });

        let a2 = session.update("昨天是").unwrap();
        assert_eq!(a2, DiffAction { backspaces: 0, new_suffix: "是".to_string() });

        let a3 = session.update("昨天是 Monday").unwrap();
        assert_eq!(a3, DiffAction { backspaces: 0, new_suffix: " Monday".to_string() });
    }

    #[test]
    fn test_diff_tail_correction() {
        let mut session = PartialSession::new();

        session.update("我觉得这个 link");
        let a2 = session.update("我觉得这个 Linux").unwrap();
        assert_eq!(a2, DiffAction {
            backspaces: 4, // deletes 'l', 'i', 'n', 'k'
            new_suffix: "Linux".to_string()
        });
    }

    #[test]
    fn test_finalize_prevents_backspacing_previous_utterance() {
        let mut session = PartialSession::new();

        session.update("第一句话。");
        session.finalize(); // Committed!

        // Second utterance
        let a2 = session.update("第二句话").unwrap();
        assert_eq!(a2, DiffAction {
            backspaces: 0, // Must NOT delete "第一句话。"
            new_suffix: "第二句话".to_string()
        });
    }
}
