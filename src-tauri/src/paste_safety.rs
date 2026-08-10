//! Make raw model output safe to paste into the user's document.
//!
//! Shared by "Generate with Flow" and Transforms: both paste a model's reply
//! directly into whatever application the user is working in, so leaked
//! reasoning blocks or a wrapping Markdown fence would land as visible garbage.

/// Make a raw model response paste-safe, or reject it.
///
/// Strips leaked reasoning (`<think>…</think>`) and unwraps a lone Markdown
/// code fence. Returns `None` only when nothing remains — symbol-only output
/// can be intentional.
pub(crate) fn sanitize_model_output(raw: &str) -> Option<String> {
    let text = strip_reasoning_blocks(raw.trim());
    let text = unwrap_full_code_fence(text.trim());
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    Some(text.to_string())
}

/// Remove leaked reasoning blocks (`<think>`, `<thinking>`, `<reasoning>`,
/// case-insensitive). A block that never closes swallows the rest of the text
/// — an unfinished thought is reasoning, not content.
fn strip_reasoning_blocks(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    // ASCII lowercase preserves byte offsets exactly (the tags are ASCII), so
    // indices found in `lower` are valid for `text`.
    let lower = text.to_ascii_lowercase();
    let mut pos = 0usize;
    while pos < text.len() {
        // Find the next reasoning-open tag at or after `pos`.
        let next = ["<think>", "<thinking>", "<reasoning>"]
            .iter()
            .filter_map(|tag| lower[pos..].find(*tag).map(|i| (pos + i, *tag)))
            .min_by_key(|(i, _)| *i);
        let Some((start, tag)) = next else {
            out.push_str(&text[pos..]);
            break;
        };
        out.push_str(&text[pos..start]);
        // `tag` is like "<think>", so this yields the full "</think>".
        let close = format!("</{}", &tag[1..]);
        match lower[start..].find(&close) {
            Some(rel) => pos = start + rel + close.len(),
            None => break, // unclosed: drop the rest
        }
    }
    out
}

/// If the ENTIRE output is a single Markdown code fence, unwrap it — the
/// prompt asks for raw content, so a lone wrapper fence is framing, not
/// formatting. Output with interior fences (real mixed Markdown) is left
/// untouched.
fn unwrap_full_code_fence(text: &str) -> &str {
    let t = text.trim();
    if !t.starts_with("```") {
        return t;
    }
    let Some(first_newline) = t.find('\n') else {
        return t;
    };
    let tail = t[first_newline + 1..].trim_end();
    let Some(body) = tail.strip_suffix("```") else {
        return t;
    };
    // The closing fence must sit on its own line.
    if !(body.is_empty() || body.ends_with('\n')) {
        return t;
    }
    // An interior fence line means this is real Markdown, not one wrapper.
    if body
        .lines()
        .any(|line| line.trim_start().starts_with("```"))
    {
        return t;
    }
    body.trim_end_matches('\n')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_passes_normal_content_through() {
        assert_eq!(
            sanitize_model_output("Dear Sam,\n\nSee you Monday.\nBest,\nAlex"),
            Some("Dear Sam,\n\nSee you Monday.\nBest,\nAlex".to_string())
        );
        // Interior formatting (including a real fenced block inside larger
        // Markdown) is preserved untouched.
        let mixed = "Intro paragraph.\n\n```py\nprint(1)\n```\n\nOutro.";
        assert_eq!(sanitize_model_output(mixed), Some(mixed.to_string()));
    }

    #[test]
    fn sanitize_rejects_empty_or_empty_wrapper_output() {
        assert_eq!(sanitize_model_output(""), None);
        assert_eq!(sanitize_model_output("   \n\n\n \t \n"), None);
        assert_eq!(sanitize_model_output("```\n\n```"), None);
    }

    #[test]
    fn sanitize_preserves_symbol_only_content() {
        assert_eq!(
            sanitize_model_output("...\n---\n\"\"\n"),
            Some("...\n---\n\"\"".to_string())
        );
        assert_eq!(sanitize_model_output("✅"), Some("✅".to_string()));
        assert_eq!(sanitize_model_output("{}"), Some("{}".to_string()));
    }

    #[test]
    fn sanitize_strips_leaked_reasoning() {
        assert_eq!(
            sanitize_model_output(
                "<think>The user wants a poem. I should rhyme.</think>\nWaves rise and fall."
            ),
            Some("Waves rise and fall.".to_string())
        );
        // Unclosed reasoning swallows the rest (it is thought, not content).
        assert_eq!(
            sanitize_model_output("<thinking>hmm this is hard and I never stop"),
            None
        );
        // Reasoning-only output is a failure, not a paste.
        assert_eq!(sanitize_model_output("<think>only thoughts</think>"), None);
    }

    #[test]
    fn sanitize_unwraps_a_lone_wrapper_fence() {
        assert_eq!(
            sanitize_model_output("```\nfn main() {}\n```"),
            Some("fn main() {}".to_string())
        );
        assert_eq!(
            sanitize_model_output("```python\nprint(\"hi\")\n```"),
            Some("print(\"hi\")".to_string())
        );
        // A fence that doesn't wrap the whole output stays as-is.
        let partial = "Some text\n```\ncode\n```";
        assert_eq!(sanitize_model_output(partial), Some(partial.to_string()));
    }
}
