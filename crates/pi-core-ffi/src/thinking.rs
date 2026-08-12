//! Composer thinking-level picker + session stats indicator (SW8).
//! Ports (not shares — this crate doesn't depend on `pi-core`, matching its
//! established posture) `pi_core::backend`'s `refresh_thinking`/
//! `update_stats`.

use pi_rpc::{Command, PiClient, ThinkingLevel};

/// Mirrors `pi_rpc::ThinkingLevel` 1:1 — UniFFI needs its own exported type,
/// can't export a `pi-rpc` type directly.
#[derive(Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ThinkingLevelKind {
    Off,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl From<ThinkingLevel> for ThinkingLevelKind {
    fn from(level: ThinkingLevel) -> Self {
        match level {
            ThinkingLevel::Off => Self::Off,
            ThinkingLevel::Minimal => Self::Minimal,
            ThinkingLevel::Low => Self::Low,
            ThinkingLevel::Medium => Self::Medium,
            ThinkingLevel::High => Self::High,
            ThinkingLevel::Xhigh => Self::Xhigh,
            ThinkingLevel::Max => Self::Max,
        }
    }
}

impl From<ThinkingLevelKind> for ThinkingLevel {
    fn from(kind: ThinkingLevelKind) -> Self {
        match kind {
            ThinkingLevelKind::Off => Self::Off,
            ThinkingLevelKind::Minimal => Self::Minimal,
            ThinkingLevelKind::Low => Self::Low,
            ThinkingLevelKind::Medium => Self::Medium,
            ThinkingLevelKind::High => Self::High,
            ThinkingLevelKind::Xhigh => Self::Xhigh,
            ThinkingLevelKind::Max => Self::Max,
        }
    }
}

/// Swift-facing picker entry — mirrors `ModelRecord`'s shape.
#[derive(Clone, uniffi::Record)]
pub struct ThinkingLevelRecord {
    pub level: ThinkingLevelKind,
    pub label: String,
    pub is_current: bool,
}

/// `{s}` -> `ThinkingLevel`, reusing the type's own `#[serde(rename_all =
/// "lowercase")]` round-trip rather than a hand-rolled duplicate match.
fn parse_thinking_level(s: &str) -> Option<ThinkingLevel> {
    serde_json::from_value(serde_json::Value::String(s.to_string())).ok()
}

/// `ThinkingLevel` -> its lowercase wire string, same round-trip in reverse.
fn thinking_level_str(level: ThinkingLevel) -> String {
    serde_json::to_value(level)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// `GetAvailableThinkingLevels` + `GetState` -> `Vec<ThinkingLevelRecord>` —
/// ports `pi_core::backend::refresh_thinking`. Fetched fresh every call (not
/// cached), since availability differs per model. Empty when the model
/// offers no levels or the request fails — the Swift-side picker is hidden
/// entirely below 2 entries anyway (matches `app.slint`'s
/// `thinking-list.length > 1` gate).
pub async fn refresh_thinking_levels(client: &PiClient) -> Vec<ThinkingLevelRecord> {
    let levels: Vec<ThinkingLevel> = match client.request(Command::GetAvailableThinkingLevels).await
    {
        Ok(Some(data)) => data
            .get("levels")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .filter_map(parse_thinking_level)
                    .collect()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    if levels.is_empty() {
        return Vec::new();
    }
    let current = match client.get_state().await {
        Ok(state) => state
            .get("thinkingLevel")
            .and_then(|v| v.as_str())
            .and_then(parse_thinking_level),
        Err(_) => None,
    };
    levels
        .into_iter()
        .map(|level| ThinkingLevelRecord {
            level: level.into(),
            label: capitalize(&thinking_level_str(level)),
            is_current: Some(level) == current,
        })
        .collect()
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Pre-formatted session-size/cost snapshot for the status bar. `tokens_
/// label` is exactly `pi_render::format_tokens`'s output (e.g. `"1.2k"`) —
/// Swift composes the rest of the caption around it, keeping the token-count
/// threshold logic centralized here rather than duplicated client-side.
#[derive(Clone, uniffi::Record)]
pub struct SessionStatsRecord {
    pub tokens_label: String,
    pub cost: f64,
    pub context_percent: f32,
}

/// `GetSessionStats` -> `SessionStatsRecord` — ports `pi_core::backend::
/// update_stats`'s response parsing. `None` on a request failure or an
/// unexpected response shape (tolerant, degrades gracefully — matches this
/// crate's posture throughout).
pub async fn fetch_session_stats(client: &PiClient) -> Option<SessionStatsRecord> {
    let data = client.get_session_stats().await.ok()?;
    let cost = data.get("cost").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let tokens = data
        .pointer("/tokens/total")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let context_percent = data
        .pointer("/contextUsage/percent")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0) as f32;
    Some(SessionStatsRecord {
        tokens_label: pi_render::format_tokens(tokens),
        cost,
        context_percent,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_wire_level() {
        assert_eq!(parse_thinking_level("off"), Some(ThinkingLevel::Off));
        assert_eq!(
            parse_thinking_level("minimal"),
            Some(ThinkingLevel::Minimal)
        );
        assert_eq!(parse_thinking_level("low"), Some(ThinkingLevel::Low));
        assert_eq!(parse_thinking_level("medium"), Some(ThinkingLevel::Medium));
        assert_eq!(parse_thinking_level("high"), Some(ThinkingLevel::High));
        assert_eq!(parse_thinking_level("xhigh"), Some(ThinkingLevel::Xhigh));
        assert_eq!(parse_thinking_level("max"), Some(ThinkingLevel::Max));
        assert_eq!(parse_thinking_level("bogus"), None);
    }

    #[test]
    fn round_trips_every_level_to_its_wire_string() {
        for (level, s) in [
            (ThinkingLevel::Off, "off"),
            (ThinkingLevel::Minimal, "minimal"),
            (ThinkingLevel::Low, "low"),
            (ThinkingLevel::Medium, "medium"),
            (ThinkingLevel::High, "high"),
            (ThinkingLevel::Xhigh, "xhigh"),
            (ThinkingLevel::Max, "max"),
        ] {
            assert_eq!(thinking_level_str(level), s);
        }
    }

    #[test]
    fn stats_record_reads_nested_fields_and_formats_tokens() {
        let data = serde_json::json!({
            "cost": 0.0234,
            "tokens": {"total": 15_000},
            "contextUsage": {"percent": 42.5},
        });
        let stats = SessionStatsRecord {
            tokens_label: pi_render::format_tokens(
                data.pointer("/tokens/total").unwrap().as_u64().unwrap(),
            ),
            cost: data.get("cost").unwrap().as_f64().unwrap(),
            context_percent: data
                .pointer("/contextUsage/percent")
                .unwrap()
                .as_f64()
                .unwrap() as f32,
        };
        assert_eq!(stats.tokens_label, "15.0k");
        assert_eq!(stats.cost, 0.0234);
        assert_eq!(stats.context_percent, 42.5);
    }

    #[test]
    fn stats_record_defaults_missing_fields_to_zero() {
        let data = serde_json::json!({});
        let cost = data.get("cost").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let tokens = data
            .pointer("/tokens/total")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let context_percent = data
            .pointer("/contextUsage/percent")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0) as f32;
        assert_eq!(cost, 0.0);
        assert_eq!(tokens, 0);
        assert_eq!(context_percent, 0.0);
    }
}
