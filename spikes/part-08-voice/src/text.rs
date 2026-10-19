//! Text helpers: transcript normalisation, word error rate and log-line number extraction.

/// Lowercased words with punctuation removed. Apostrophes stay inside words.
pub fn words(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !(c.is_alphanumeric() || c == '\''))
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

/// Removes `[...]` and `(...)` spans, where whisper puts non-speech markers such as `[BLANK_AUDIO]`.
pub fn strip_non_speech_tags(s: &str) -> String {
    let mut out = String::new();
    let mut depth = 0usize;
    for c in s.chars() {
        match c {
            '[' | '(' => depth += 1,
            ']' | ')' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// Word-level Levenshtein distance over the reference length. Returns None for an empty reference.
pub fn word_error_rate(reference: &str, hypothesis: &str) -> Option<f64> {
    let r = words(reference);
    let h = words(hypothesis);
    if r.is_empty() {
        return None;
    }
    let mut prev: Vec<usize> = (0..=h.len()).collect();
    for (i, rw) in r.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, hw) in h.iter().enumerate() {
            let substitution = prev[j] + usize::from(rw != hw);
            cur.push(substitution.min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    Some(prev[h.len()] as f64 / r.len() as f64)
}

/// The first number following `marker`, skipping whitespace: `number_after("load time =  1.5 ms", "=")`.
pub fn number_after(s: &str, marker: &str) -> Option<f64> {
    let rest = s[s.find(marker)? + marker.len()..].trim_start();
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | '-' | 'e' | '+')))
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_drop_punctuation_and_case() {
        assert_eq!(
            words("  Take a Screenshot, now!\r\n"),
            ["take", "a", "screenshot", "now"]
        );
        assert_eq!(words("it's"), ["it's"]);
    }

    #[test]
    fn wer_counts_substitutions_insertions_deletions() {
        assert_eq!(word_error_rate("a b c d", "a b c d"), Some(0.0));
        assert_eq!(word_error_rate("a b c d", "a x c d"), Some(0.25));
        assert_eq!(word_error_rate("a b c d", "a b c"), Some(0.25));
        assert_eq!(word_error_rate("a b", "a b c d"), Some(1.0));
        assert_eq!(word_error_rate("a b", ""), Some(1.0));
        assert_eq!(word_error_rate("", "anything"), None);
    }

    #[test]
    fn strips_whisper_markers() {
        assert!(words(&strip_non_speech_tags(" [BLANK_AUDIO] ")).is_empty());
        assert!(words(&strip_non_speech_tags("(silence) [MUSIC]")).is_empty());
        assert_eq!(
            words(&strip_non_speech_tags("hello [noise] world")),
            ["hello", "world"]
        );
    }

    #[test]
    fn extracts_numbers_from_log_lines() {
        let line = "whisper_print_timings:     load time =   115.85 ms";
        assert_eq!(number_after(line, "load time ="), Some(115.85));
        assert_eq!(
            number_after("x infer=0.0727401 sec, audio=2.6 sec", "infer="),
            Some(0.0727401)
        );
        assert_eq!(number_after("no numbers here", "here"), None);
        assert_eq!(number_after("absent", "marker"), None);
    }
}
