//! Input normalization and tokenization for the intent pipeline.
//!
//! Deliberately small and deterministic: lowercase, trim, collapse
//! whitespace, strip ASCII punctuation. Both the KV cache key and the
//! inference prompt consume this, so the same user input always produces
//! the same normalized form.

/// Normalize raw user input.
pub fn normalize(input: &str) -> String {
    let lowered = input.trim().to_lowercase();
    let stripped: String = lowered
        .chars()
        .map(|c| if c.is_ascii_punctuation() { ' ' } else { c })
        .collect();
    stripped.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Tokenize normalized input into words.
pub fn tokenize(input: &str) -> Vec<String> {
    normalize(input)
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_case_punctuation_whitespace() {
        assert_eq!(
            normalize("  Open   my Robotics-Work!  "),
            "open my robotics work"
        );
    }

    #[test]
    fn tokenizes_words() {
        assert_eq!(tokenize("Open VSCode now"), vec!["open", "vscode", "now"]);
    }

    #[test]
    fn empty_input_normalizes_to_empty() {
        assert_eq!(normalize("   ...   "), "");
    }

    #[test]
    fn idempotent() {
        let once = normalize("Open: my, robotics work?");
        assert_eq!(normalize(&once), once);
    }
}
