//! Transcript provider selection and lightweight format detection.
//!
//! Provider selection is deliberately independent from the native tailer so the
//! browser/replay entry points can share the same `auto` behaviour. `Auto` only
//! inspects transcript envelopes and never reads credentials.

use std::fmt;
use std::str::FromStr;

/// Transcript provider used by the native watcher and portable replay parser.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProviderKind {
    /// Detect an explicit file from its JSONL envelope. Directory discovery
    /// keeps Claude's historical precedence when both stores are present.
    #[default]
    Auto,
    /// Claude Code JSONL transcript and `~/.claude/projects` discovery.
    Claude,
    /// Codex rollout JSONL transcript and `$CODEX_HOME/sessions` discovery.
    Codex,
}

impl ProviderKind {
    /// Whether this provider is the Codex rollout format.
    pub const fn is_codex(self) -> bool {
        matches!(self, Self::Codex)
    }

    /// Resolve `auto` from a single transcript line.
    pub fn detect_line(line: &str) -> Option<Self> {
        let value = serde_json::from_str::<serde_json::Value>(line).ok()?;
        let kind = value.get("type")?.as_str()?;
        if crate::transcript::is_codex_record_type(kind) {
            Some(Self::Codex)
        } else {
            Some(Self::Claude)
        }
    }
}

impl fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Auto => "auto",
            Self::Claude => "claude",
            Self::Codex => "codex",
        })
    }
}

impl FromStr for ProviderKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "claude" => Ok(Self::Claude),
            "codex" => Ok(Self::Codex),
            other => Err(format!(
                "unknown provider {other:?}; expected auto, claude, or codex"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_values_round_trip() {
        for value in ["auto", "claude", "codex"] {
            let parsed: ProviderKind = value.parse().unwrap();
            assert_eq!(parsed.to_string(), value);
        }
    }

    #[test]
    fn auto_detects_envelope_without_reading_auth() {
        assert_eq!(
            ProviderKind::detect_line(r#"{"type":"response_item","payload":{}}"#),
            Some(ProviderKind::Codex)
        );
        assert_eq!(
            ProviderKind::detect_line(r#"{"type":"assistant","message":{}}"#),
            Some(ProviderKind::Claude)
        );
    }
}
