pub fn chunk_text(text: &str) -> Vec<String> {
    let text = text.trim();
    if text.is_empty() {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut start = 0;
    let bytes = text.as_bytes();

    for i in 0..bytes.len() {
        if matches!(bytes[i], b'.' | b'!' | b'?') {
            let end = i + 1;
            let at_end = end >= bytes.len();
            let followed_by_space = !at_end && bytes[end].is_ascii_whitespace();
            if at_end || followed_by_space {
                let chunk = text[start..end].trim();
                if !chunk.is_empty() {
                    chunks.push(chunk.to_string());
                }
                start = end;
            }
        }
    }

    let remaining = text[start..].trim();
    if !remaining.is_empty() {
        chunks.push(remaining.to_string());
    }

    if chunks.is_empty() {
        chunks.push(text.to_string());
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_sentence() {
        let chunks = chunk_text("Hello world.");
        assert_eq!(chunks, vec!["Hello world."]);
    }

    #[test]
    fn multiple_sentences() {
        let chunks = chunk_text("First sentence. Second sentence! Third?");
        assert_eq!(
            chunks,
            vec!["First sentence.", "Second sentence!", "Third?"]
        );
    }

    #[test]
    fn no_punctuation() {
        let chunks = chunk_text("Hello world");
        assert_eq!(chunks, vec!["Hello world"]);
    }

    #[test]
    fn abbreviations_no_space() {
        let chunks = chunk_text("Dr.Smith went home.");
        assert_eq!(chunks, vec!["Dr.Smith went home."]);
    }

    #[test]
    fn splits_after_punctuation_space() {
        let chunks = chunk_text("First part. Second part.");
        assert_eq!(chunks, vec!["First part.", "Second part."]);
    }

    #[test]
    fn empty_input() {
        let chunks = chunk_text("");
        assert_eq!(chunks, vec![""]);
    }

    #[test]
    fn whitespace_only() {
        let chunks = chunk_text("   ");
        assert_eq!(chunks, vec![""]);
    }
}
