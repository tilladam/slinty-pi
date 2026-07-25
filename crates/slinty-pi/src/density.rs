//! Persists the transcript density mode (Verbose/Normal/Summary) across runs.

use std::path::PathBuf;

const NORMAL: i32 = 1;

fn state_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("dev", "slinty-pi", "slinty-pi")
        .map(|dirs| dirs.data_dir().join("state.json"))
}

pub fn load() -> i32 {
    let Some(path) = state_path() else {
        return NORMAL;
    };
    let Ok(contents) = std::fs::read_to_string(path) else {
        return NORMAL;
    };
    parse(&contents)
}

pub fn save(density: i32) {
    let Some(path) = state_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    let _ = std::fs::write(path, serialize(density));
}

fn parse(contents: &str) -> i32 {
    serde_json::from_str::<serde_json::Value>(contents)
        .ok()
        .and_then(|v| v.get("density").and_then(|d| d.as_i64()))
        .map(|d| d.clamp(0, 2) as i32)
        .unwrap_or(NORMAL)
}

fn serialize(density: i32) -> String {
    serde_json::json!({ "density": density }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_serialize_and_parse() {
        for d in 0..=2 {
            assert_eq!(parse(&serialize(d)), d);
        }
    }

    #[test]
    fn falls_back_to_normal_on_missing_or_malformed_input() {
        assert_eq!(parse(""), NORMAL);
        assert_eq!(parse("not json"), NORMAL);
        assert_eq!(parse("{}"), NORMAL);
        assert_eq!(parse(r#"{"density":"nope"}"#), NORMAL);
    }

    #[test]
    fn clamps_out_of_range_values() {
        assert_eq!(parse(r#"{"density":99}"#), 2);
        assert_eq!(parse(r#"{"density":-5}"#), 0);
    }
}
