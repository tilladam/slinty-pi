//! Reads and writes pi's `~/.pi/agent/models.json` (provider/model config;
//! pi hot-reloads it when its model picker opens). The write path preserves
//! everything this app doesn't touch — unknown fields, key order, and the
//! file's trailing newline — verified below against a real hand-written
//! file, not assumed.

use std::path::PathBuf;

use serde_json::Value;

pub const RELATIVE_PATH: &str = ".pi/agent/models.json";

#[derive(Debug, thiserror::Error)]
pub enum ModelsJsonError {
    #[error("could not read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not parse {path} as JSON: {source}")]
    Parse {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("{path} is not a JSON object at the top level")]
    NotAnObject { path: PathBuf },
    #[error("could not write {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
}

pub fn default_path() -> Option<PathBuf> {
    std::env::home_dir().map(|h| h.join(RELATIVE_PATH))
}

/// A parsed `models.json`. `trailing_newline` is tracked separately from
/// `doc` (a `serde_json::Value`, not a typed struct — see the module doc
/// comment) so [`Self::to_bytes`] can reproduce it; `serde_json`'s
/// pretty-printer never emits one.
#[derive(Debug)]
pub struct ModelsJson {
    doc: Value,
    trailing_newline: bool,
}

impl ModelsJson {
    /// A fresh `{}` document, for when `models.json` doesn't exist yet.
    pub fn empty() -> Self {
        Self {
            doc: Value::Object(Default::default()),
            trailing_newline: true,
        }
    }

    pub fn parse(bytes: &[u8], path: &std::path::Path) -> Result<Self, ModelsJsonError> {
        let text = String::from_utf8_lossy(bytes);
        let doc: Value = serde_json::from_str(&text).map_err(|source| ModelsJsonError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
        if !doc.is_object() {
            return Err(ModelsJsonError::NotAnObject {
                path: path.to_path_buf(),
            });
        }
        Ok(Self {
            doc,
            trailing_newline: text.ends_with('\n'),
        })
    }

    pub fn load(path: &std::path::Path) -> Result<Self, ModelsJsonError> {
        let bytes = std::fs::read(path).map_err(|source| ModelsJsonError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        Self::parse(&bytes, path)
    }

    /// `serde_json`'s 2-space pretty-printer, `preserve_order`'d (see
    /// `Cargo.toml`) so untouched providers keep their original key order,
    /// plus the original trailing newline if the source had one.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = serde_json::to_vec_pretty(&self.doc).expect("Value always serializes");
        if self.trailing_newline {
            out.push(b'\n');
        }
        out
    }

    /// Adds or replaces `doc.providers.<key>`, creating `providers` as an
    /// object if the document doesn't have one yet.
    pub fn set_provider(&mut self, key: &str, provider: Value) {
        let providers = self
            .doc
            .as_object_mut()
            .expect("parse() rejects non-object top level")
            .entry("providers")
            .or_insert_with(|| Value::Object(Default::default()));
        if !providers.is_object() {
            *providers = Value::Object(Default::default());
        }
        providers
            .as_object_mut()
            .expect("just ensured this is an object")
            .insert(key.to_string(), provider);
    }

    /// Atomic write (tmp file + rename, same directory so the rename is on
    /// one filesystem) plus a `.bak` of whatever was there before — the M3
    /// plan's stated mitigation for `models.json` corruption risk.
    pub fn write(&self, path: &std::path::Path) -> Result<(), ModelsJsonError> {
        let err = |source| ModelsJsonError::Write {
            path: path.to_path_buf(),
            source,
        };
        if path.exists() {
            std::fs::copy(path, path.with_extension("json.bak")).map_err(err)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, self.to_bytes()).map_err(err)?;
        std::fs::rename(&tmp, path).map_err(err)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real hand-written `~/.pi/agent/models.json` from this machine,
    /// captured 2026-07-28 — the fixture the round-trip guarantee is tested
    /// against, not a synthetic example.
    const REAL_FIXTURE: &str = "{\n  \"providers\": {\n    \"rapid-mlx\": {\n      \"name\": \"Rapid-MLX Local\",\n      \"baseUrl\": \"http://localhost:8000/v1\",\n      \"api\": \"openai-completions\",\n      \"apiKey\": \"rapid-mlx\",\n      \"compat\": {\n        \"supportsDeveloperRole\": false,\n        \"supportsReasoningEffort\": false\n      },\n      \"models\": [\n        {\n          \"id\": \"gpt-oss-120b-mxfp4-q8\",\n          \"name\": \"GPT-OSS 120B (Rapid-MLX)\",\n          \"contextWindow\": 128000,\n          \"maxTokens\": 32000\n        },\n        {\n          \"id\": \"mlx-community/Qwen3.6-35B-A3B-8bit\",\n          \"name\": \"Qwen 3.6 35B 8-bit (Rapid-MLX)\",\n          \"contextWindow\": 128000,\n          \"maxTokens\": 32000\n        },\n        {\n          \"id\": \"mlx-community/Qwen3.5-4B-MLX-4bit\",\n          \"name\": \"Qwen 3.5 4B 4-bit (Rapid-MLX)\",\n          \"contextWindow\": 128000,\n          \"maxTokens\": 32000\n        },\n        {\n          \"id\": \"mlx-community/Qwen3-4B-Instruct-2507-4bit\",\n          \"name\": \"Qwen3 4B Instruct 4-bit (Rapid-MLX)\",\n          \"contextWindow\": 128000,\n          \"maxTokens\": 32000\n        }\n      ]\n    }\n  }\n}\n";

    #[test]
    fn parses_then_reserializes_a_real_file_byte_for_byte_unedited() {
        let parsed =
            ModelsJson::parse(REAL_FIXTURE.as_bytes(), std::path::Path::new("models.json"))
                .expect("parses");
        let out = parsed.to_bytes();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            REAL_FIXTURE,
            "reserializing an untouched real file must reproduce it byte-for-byte \
             (key order, 2-space indent, and trailing newline all matter here)"
        );
    }

    #[test]
    fn adding_a_provider_leaves_the_existing_one_byte_identical() {
        let mut parsed =
            ModelsJson::parse(REAL_FIXTURE.as_bytes(), std::path::Path::new("models.json"))
                .expect("parses");
        parsed.set_provider(
            "ollama",
            serde_json::json!({
                "name": "Ollama",
                "baseUrl": "http://localhost:11434/v1",
                "api": "openai-completions",
                "apiKey": "ollama",
            }),
        );
        let out = String::from_utf8(parsed.to_bytes()).unwrap();
        // The untouched rapid-mlx block, verbatim including its indentation,
        // must still appear unchanged inside the new document.
        let rapid_mlx_block_start = REAL_FIXTURE.find("\"rapid-mlx\": {").unwrap();
        let rapid_mlx_block_end =
            REAL_FIXTURE.rfind("      ]\n    }\n").unwrap() + "      ]\n    }".len();
        let untouched = &REAL_FIXTURE[rapid_mlx_block_start..rapid_mlx_block_end];
        assert!(
            out.contains(untouched),
            "existing rapid-mlx entry should be byte-identical after adding a sibling provider"
        );
        assert!(out.contains("\"ollama\""));
    }

    #[test]
    fn rejects_a_non_object_top_level() {
        let err = ModelsJson::parse(b"[1, 2, 3]", std::path::Path::new("models.json"))
            .expect_err("array top level should be rejected, not silently accepted");
        assert!(matches!(err, ModelsJsonError::NotAnObject { .. }));
    }

    #[test]
    fn rejects_malformed_json_rather_than_guessing() {
        let err = ModelsJson::parse(b"{ not json", std::path::Path::new("models.json"))
            .expect_err("malformed JSON should be a hard error, never silently rewritten");
        assert!(matches!(err, ModelsJsonError::Parse { .. }));
    }

    #[test]
    fn write_is_atomic_and_keeps_a_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("models.json");
        std::fs::write(&path, REAL_FIXTURE).unwrap();

        let mut doc = ModelsJson::load(&path).unwrap();
        doc.set_provider("ollama", serde_json::json!({"name": "Ollama"}));
        doc.write(&path).unwrap();

        assert!(!dir.path().join("models.json.tmp").exists());
        let backup = std::fs::read_to_string(dir.path().join("models.json.bak")).unwrap();
        assert_eq!(
            backup, REAL_FIXTURE,
            "backup should hold the pre-edit content"
        );

        let reloaded = ModelsJson::load(&path).unwrap();
        assert!(reloaded.doc["providers"]["ollama"].is_object());
        assert!(reloaded.doc["providers"]["rapid-mlx"].is_object());
    }
}
