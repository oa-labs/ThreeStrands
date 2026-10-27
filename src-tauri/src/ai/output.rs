//! Decodes model output shared by every structured feature.

pub(super) const MAX_ACTION_OUTPUT_CHARS: usize = 32_000;

/// Strips a single leading/trailing markdown code fence (```` ``` ```` or
/// ```` ```json ````), which some models emit despite being told not to.
pub(super) fn strip_markdown_fences(content: &str) -> &str {
    let trimmed = content.trim();
    let Some(rest) = trimmed.strip_prefix("```") else {
        return trimmed;
    };
    let rest = rest.strip_prefix("json").unwrap_or(rest);
    let rest = rest.strip_prefix("JSON").unwrap_or(rest);
    let rest = rest.trim_start_matches(['\r', '\n']);
    rest.strip_suffix("```").map_or(trimmed, str::trim_end)
}

/// Why model output could not be decoded as JSON at all.
pub(super) enum OutputDecodeError {
    Oversized,
    Empty,
    Malformed,
}

pub(super) fn decode_json_output(content: &str, target: &str) -> Result<serde_json::Value, OutputDecodeError> {
    if content.chars().count() > MAX_ACTION_OUTPUT_CHARS {
        log::error!(target: "ai_analyze_thread", "oversized {target} output ({} chars)", content.chars().count());
        return Err(OutputDecodeError::Oversized);
    }
    let trimmed = strip_markdown_fences(content);
    if trimmed.is_empty() {
        log::error!(target: "ai_analyze_thread", "empty {target} output (raw content: {content:?})");
        return Err(OutputDecodeError::Empty);
    }
    serde_json::from_str(trimmed).map_err(|error| {
        log::error!(target: "ai_analyze_thread", "malformed {target} JSON: {error} (raw content: {content:?})");
        OutputDecodeError::Malformed
    })
}

pub(super) fn contains_quoted_history(value: &str) -> bool {
    value.lines().any(|line| {
        let trimmed = line.trim();
        trimmed.starts_with('>')
            || trimmed.eq_ignore_ascii_case("-----Original Message-----")
            || trimmed.eq_ignore_ascii_case("---------- Forwarded message ----------")
            || (trimmed.starts_with("On ") && trimmed.ends_with(" wrote:"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_markdown_json_fence_around_action_proposals() {
        assert_eq!(
            strip_markdown_fences("```json\n[{\"a\":1}]\n```"),
            "[{\"a\":1}]"
        );
        assert_eq!(
            strip_markdown_fences("```\n[{\"a\":1}]\n```"),
            "[{\"a\":1}]"
        );
        assert_eq!(strip_markdown_fences("  [{\"a\":1}]  "), "[{\"a\":1}]");
        assert_eq!(
            strip_markdown_fences("not fenced at all"),
            "not fenced at all"
        );
    }
}
