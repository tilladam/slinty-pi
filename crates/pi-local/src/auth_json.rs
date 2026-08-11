//! Reads and writes pi's `~/.pi/agent/auth.json` (provider credentials;
//! see pi's docs/providers.md "Auth File"). Same write-fidelity contract as
//! [`super::models_json`] — unknown fields, key order, and the trailing
//! newline survive a round-trip — plus two rules of its own:
//!
//! - Entries this app didn't write stay untouched, and entries whose `key`
//!   uses pi's interpolation forms (`$ENV`, `${ENV}`, `!command`) or whose
//!   `type` isn't `api_key` (OAuth tokens) are **refused** as edit targets
//!   rather than silently overwritten.
//! - The file carries secrets: it is written 0600 (created 0600 when new,
//!   an existing file's stricter-or-equal mode is preserved), and nothing
//!   in this module ever puts key material into an error, label, or log.

use std::path::PathBuf;

use serde_json::Value;

pub const RELATIVE_PATH: &str = ".pi/agent/auth.json";

#[derive(Debug, thiserror::Error)]
pub enum AuthJsonError {
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
    #[error("provider id must not be empty")]
    EmptyProvider,
    #[error("refusing to overwrite `{provider}`: {reason}")]
    Protected {
        provider: String,
        reason: &'static str,
    },
}

pub fn default_path() -> Option<PathBuf> {
    std::env::home_dir().map(|h| h.join(RELATIVE_PATH))
}

/// How an existing entry stores its credential — shown in the UI so
/// interpolation/OAuth entries are visibly read-only, without ever
/// exposing the credential itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyForm {
    /// A literal `api_key` value; this app may replace it.
    Literal,
    /// `$ENV` / `${ENV}` interpolation (read-only here).
    Env,
    /// `!command` execution (read-only here).
    Command,
    /// `type` isn't `api_key` — OAuth tokens etc. (managed by `pi /login`).
    Managed,
}

impl KeyForm {
    pub fn label(self) -> &'static str {
        match self {
            KeyForm::Literal => "api key",
            KeyForm::Env => "$ENV — read-only",
            KeyForm::Command => "!command — read-only",
            KeyForm::Managed => "managed by pi /login",
        }
    }
}

#[derive(Debug)]
pub struct AuthJson {
    doc: Value,
    trailing_newline: bool,
}

impl AuthJson {
    /// A fresh `{}` document, for when `auth.json` doesn't exist yet.
    pub fn empty() -> Self {
        Self {
            doc: Value::Object(Default::default()),
            trailing_newline: true,
        }
    }

    pub fn parse(bytes: &[u8], path: &std::path::Path) -> Result<Self, AuthJsonError> {
        let text = String::from_utf8_lossy(bytes);
        let doc: Value = serde_json::from_str(&text).map_err(|source| AuthJsonError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
        if !doc.is_object() {
            return Err(AuthJsonError::NotAnObject {
                path: path.to_path_buf(),
            });
        }
        Ok(Self {
            doc,
            trailing_newline: text.ends_with('\n'),
        })
    }

    /// Load the file, or an empty document if it doesn't exist yet. Any
    /// other failure (unreadable, malformed) is still an error — never
    /// treat a file we couldn't parse as empty, we'd clobber it on write.
    pub fn load_or_empty(path: &std::path::Path) -> Result<Self, AuthJsonError> {
        match std::fs::read(path) {
            Ok(bytes) => Self::parse(&bytes, path),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::empty()),
            Err(source) => Err(AuthJsonError::Read {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    /// `(provider, form)` per entry, in document order. No key material.
    pub fn entries(&self) -> Vec<(String, KeyForm)> {
        self.doc
            .as_object()
            .expect("parse() rejects non-object top level")
            .iter()
            .map(|(provider, entry)| (provider.clone(), entry_form(entry)))
            .collect()
    }

    /// Insert or replace `provider` with `{"type": "api_key", "key": key}`.
    /// Refuses to touch protected entries (interpolation forms, OAuth, or
    /// anything else this app can't faithfully re-create).
    pub fn set_api_key(&mut self, provider: &str, key: &str) -> Result<(), AuthJsonError> {
        let provider = provider.trim();
        if provider.is_empty() {
            return Err(AuthJsonError::EmptyProvider);
        }
        let obj = self
            .doc
            .as_object_mut()
            .expect("parse() rejects non-object top level");
        if let Some(existing) = obj.get(provider) {
            match entry_form(existing) {
                KeyForm::Literal => {}
                KeyForm::Env | KeyForm::Command => {
                    return Err(AuthJsonError::Protected {
                        provider: provider.to_string(),
                        reason: "its key uses $ENV/!command interpolation; edit the file directly",
                    });
                }
                KeyForm::Managed => {
                    return Err(AuthJsonError::Protected {
                        provider: provider.to_string(),
                        reason: "it is not an api_key entry (use `pi /login`)",
                    });
                }
            }
            // A literal entry may carry extra fields (e.g. a provider-scoped
            // `env` map) that a plain replacement would drop — update the
            // key in place instead.
            let entry = obj
                .get_mut(provider)
                .and_then(|e| e.as_object_mut())
                .expect("entry_form classified this as a literal api_key object");
            entry.insert("key".into(), Value::String(key.to_string()));
        } else {
            obj.insert(
                provider.to_string(),
                serde_json::json!({"type": "api_key", "key": key}),
            );
        }
        Ok(())
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = serde_json::to_vec_pretty(&self.doc).expect("Value always serializes");
        if self.trailing_newline {
            out.push(b'\n');
        }
        out
    }

    /// Atomic write (tmp + same-directory rename) with a `.bak` of the
    /// previous content. Every file involved — tmp, backup, final — is 0600
    /// before any secret lands in it (`fs::copy` preserves the original's
    /// mode for the backup).
    pub fn write(&self, path: &std::path::Path) -> Result<(), AuthJsonError> {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;

        let err = |source| AuthJsonError::Write {
            path: path.to_path_buf(),
            source,
        };
        if path.exists() {
            std::fs::copy(path, path.with_extension("json.bak")).map_err(err)?;
        }
        let tmp = path.with_extension("json.tmp");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(err)?;
        file.write_all(&self.to_bytes()).map_err(err)?;
        file.sync_all().map_err(err)?;
        drop(file);
        std::fs::rename(&tmp, path).map_err(err)?;
        Ok(())
    }
}

/// Classify an entry without touching its credential. Any unescaped `$`
/// counts as interpolation (pi interpolates inside larger literals too);
/// `$$`/`$!` escape prefixes make the rest literal enough to replace.
fn entry_form(entry: &Value) -> KeyForm {
    if entry.get("type").and_then(|t| t.as_str()) != Some("api_key") {
        return KeyForm::Managed;
    }
    let Some(key) = entry.get("key").and_then(|k| k.as_str()) else {
        return KeyForm::Managed;
    };
    if key.starts_with('!') {
        return KeyForm::Command;
    }
    let without_escapes = key.replace("$$", "").replace("$!", "");
    if without_escapes.contains('$') {
        return KeyForm::Env;
    }
    KeyForm::Literal
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;

    /// Shaped after pi's own docs/providers.md "Auth File" examples,
    /// including an interpolation entry with a provider-scoped `env` map
    /// and an OAuth entry with fields we must never re-create by hand.
    const FIXTURE: &str = "{\n  \"anthropic\": {\n    \"type\": \"api_key\",\n    \"key\": \"sk-ant-existing\"\n  },\n  \"cloudflare-ai-gateway\": {\n    \"type\": \"api_key\",\n    \"key\": \"$CLOUDFLARE_API_KEY\",\n    \"env\": {\n      \"CLOUDFLARE_ACCOUNT_ID\": \"account-id\"\n    }\n  },\n  \"openai\": {\n    \"type\": \"api_key\",\n    \"key\": \"!op read 'op://vault/item/credential'\"\n  },\n  \"github-copilot\": {\n    \"type\": \"oauth\",\n    \"access\": \"gho_token\",\n    \"expires\": 1733234567890\n  }\n}\n";

    fn parsed() -> AuthJson {
        AuthJson::parse(FIXTURE.as_bytes(), std::path::Path::new("auth.json")).expect("parses")
    }

    #[test]
    fn untouched_round_trip_is_byte_identical() {
        assert_eq!(String::from_utf8(parsed().to_bytes()).unwrap(), FIXTURE);
    }

    #[test]
    fn entries_classify_every_form_without_exposing_keys() {
        let entries = parsed().entries();
        assert_eq!(
            entries,
            vec![
                ("anthropic".into(), KeyForm::Literal),
                ("cloudflare-ai-gateway".into(), KeyForm::Env),
                ("openai".into(), KeyForm::Command),
                ("github-copilot".into(), KeyForm::Managed),
            ]
        );
    }

    #[test]
    fn replacing_a_literal_key_keeps_every_other_entry_byte_identical() {
        let mut doc = parsed();
        doc.set_api_key("anthropic", "sk-ant-new").unwrap();
        let out = String::from_utf8(doc.to_bytes()).unwrap();
        assert!(out.contains("\"key\": \"sk-ant-new\""));
        assert!(!out.contains("sk-ant-existing"));
        // Everything after the anthropic entry must be untouched verbatim.
        let tail_start = FIXTURE.find("\"cloudflare-ai-gateway\"").unwrap();
        assert!(out.contains(&FIXTURE[tail_start..FIXTURE.len() - 2]));
    }

    #[test]
    fn adding_a_new_provider_keeps_existing_entries() {
        let mut doc = parsed();
        doc.set_api_key("mistral", "mk-123").unwrap();
        let out = String::from_utf8(doc.to_bytes()).unwrap();
        assert!(out.contains("\"mistral\""));
        assert!(out.contains("sk-ant-existing"));
    }

    #[test]
    fn interpolation_and_oauth_entries_are_refused() {
        let mut doc = parsed();
        for provider in ["cloudflare-ai-gateway", "openai", "github-copilot"] {
            let err = doc.set_api_key(provider, "sk-clobber").unwrap_err();
            assert!(
                matches!(err, AuthJsonError::Protected { .. }),
                "{provider} must be refused"
            );
        }
        // And the refusals must not have modified anything.
        assert_eq!(String::from_utf8(doc.to_bytes()).unwrap(), FIXTURE);
    }

    #[test]
    fn updating_a_literal_entry_preserves_its_sibling_fields() {
        let fixture = "{\n  \"custom\": {\n    \"type\": \"api_key\",\n    \"key\": \"old\",\n    \"env\": {\n      \"EXTRA\": \"kept\"\n    }\n  }\n}\n";
        let mut doc =
            AuthJson::parse(fixture.as_bytes(), std::path::Path::new("auth.json")).expect("parses");
        doc.set_api_key("custom", "new").unwrap();
        let out = String::from_utf8(doc.to_bytes()).unwrap();
        assert!(out.contains("\"EXTRA\": \"kept\""));
        assert!(out.contains("\"key\": \"new\""));
    }

    #[test]
    fn escaped_dollar_prefixes_count_as_literals() {
        let fixture =
            "{\n  \"a\": {\n    \"type\": \"api_key\",\n    \"key\": \"$$literal\"\n  }\n}\n";
        let doc = AuthJson::parse(fixture.as_bytes(), std::path::Path::new("auth.json")).unwrap();
        assert_eq!(doc.entries()[0].1, KeyForm::Literal);
    }

    #[test]
    fn empty_provider_id_is_rejected() {
        let mut doc = AuthJson::empty();
        assert!(matches!(
            doc.set_api_key("  ", "k"),
            Err(AuthJsonError::EmptyProvider)
        ));
    }

    #[test]
    fn missing_file_loads_as_empty_but_malformed_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("auth.json");
        assert!(AuthJson::load_or_empty(&missing)
            .unwrap()
            .entries()
            .is_empty());

        std::fs::write(&missing, "{ not json").unwrap();
        assert!(matches!(
            AuthJson::load_or_empty(&missing),
            Err(AuthJsonError::Parse { .. })
        ));
    }

    #[test]
    fn write_creates_0600_atomically_with_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");

        // Fresh file: created 0600.
        let mut doc = AuthJson::empty();
        doc.set_api_key("anthropic", "sk-1").unwrap();
        doc.write(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "fresh auth.json must be 0600");
        assert!(!dir.path().join("auth.json.tmp").exists());

        // Rewrite: backup exists, is also 0600, and holds the old content.
        let before = std::fs::read_to_string(&path).unwrap();
        let mut doc = AuthJson::load_or_empty(&path).unwrap();
        doc.set_api_key("anthropic", "sk-2").unwrap();
        doc.write(&path).unwrap();
        let bak = dir.path().join("auth.json.bak");
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), before);
        let bak_mode = std::fs::metadata(&bak).unwrap().permissions().mode() & 0o777;
        assert_eq!(bak_mode, 0o600, "backup carries secrets too");
        assert!(std::fs::read_to_string(&path).unwrap().contains("sk-2"));
        assert!(!std::fs::read_to_string(&path).unwrap().contains("sk-1"));
    }
}
