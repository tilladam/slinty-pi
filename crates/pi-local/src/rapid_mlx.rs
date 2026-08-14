//! Integration with [rapid-mlx](https://rapidmlx.com) (Apache-2.0, Apple
//! Silicon M1+/macOS 14+): detection, CLI catalog parsing, and a managed
//! `serve` child process lifecycle.
//!
//! Management is CLI-first, not HTTP: there is no documented `--json` flag
//! (`rapid-mlx models --help` / `info --help` confirm no such option as of
//! 0.11.3), so `models`, `models --cached`, `ps`, and `info <alias>` are
//! parsed from rich-formatted human text. To stay robust against the
//! renderer's column padding (verified live: a long alias can push a row's
//! later columns past their header-aligned start, e.g.
//! `qwen3-4b-instruct-2507-4bit` overflowing the `Alias` column in
//! `models --cached`), every parser here counts whitespace-separated tokens
//! rather than slicing by character column — every field in these tables is
//! either a single token or, for `Size`/`Modified`, a token whose *count* is
//! fixed (`"4.2 GiB"`, `"2d ago"`) even though its *position* isn't. Rows
//! that don't fit the expected token count are skipped rather than
//! misparsed; that's the tradeoff the M3 plan calls out for CLI-scraping.

use std::collections::BTreeMap;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader, Lines};
use tokio::process::{Child, ChildStdout, Command};

pub const DEFAULT_BINARY: &str = "rapid-mlx";

#[derive(Debug, thiserror::Error)]
pub enum RapidMlxError {
    #[error("failed to run rapid-mlx: {0}")]
    Io(#[from] std::io::Error),
    #[error("rapid-mlx command failed: {0}")]
    Command(String),
    #[error("rapid-mlx exited before its server became ready")]
    ExitedBeforeReady,
    #[error("timed out waiting for rapid-mlx to become ready")]
    ReadyTimeout,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningServer {
    pub pid: u32,
    pub port: u16,
    pub model: String,
    pub uptime: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEntry {
    pub alias: String,
    /// Approximate download footprint. `None` on catalogs older than 0.11.3
    /// (no `Size` column yet) or when rapid-mlx prints `—` (unknown).
    pub size_bytes: Option<u64>,
    pub tool_format: Option<String>,
    pub reasoning_parser: Option<String>,
    pub spec_decode: bool,
    pub hybrid: bool,
    pub suffix_tier: Option<String>,
    pub dflash: Option<String>,
    pub ddtree: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedModel {
    pub alias: String,
    pub hf_repo: String,
    pub size_bytes: u64,
    pub modified: String,
}

/// Per-alias profile from `rapid-mlx info <alias>` — only the first
/// (top-level) box, keyed by its raw field labels ("Tool format",
/// "Reasoning parser", "Spec decode", ...). The DFlash/DDTree eligibility
/// boxes that follow share several of the same field labels, so parsing
/// stops at the first box's closing rule to avoid overwriting them.
///
/// Not wired into the models panel yet (the current cached-models list only
/// needs alias/size/fit) — feeds a future per-alias detail view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AliasProfile {
    pub model_path: Option<String>,
    pub fields: BTreeMap<String, String>,
}

#[allow(dead_code)] // see AliasProfile's doc comment
impl AliasProfile {
    pub fn field(&self, key: &str) -> Option<&str> {
        self.fields.get(key).map(String::as_str)
    }

    pub fn tool_format(&self) -> Option<&str> {
        self.field("Tool format")
    }

    pub fn reasoning_parser(&self) -> Option<&str> {
        self.field("Reasoning parser")
    }

    /// Fields here lead with a `✓`/`✗` glyph (e.g. `"✗ disabled (no
    /// MTP/drafter trained)"`); `true` iff the glyph is `✓`.
    pub fn spec_decode_enabled(&self) -> bool {
        self.field("Spec decode")
            .map(|v| v.trim_start().starts_with('✓'))
            .unwrap_or(false)
    }
}

/// `GET /health` on a rapid-mlx server (verified live against 0.11.3):
/// `{"status": "healthy", "ready": true, "model_loaded": true,
/// "model_name": "mlx-community/…", …}`. The served model matters because
/// rapid-mlx answers `/v1/chat/completions` with **404 "model does not
/// exist"** for any other model id — a reachable server is not enough for a
/// request to succeed.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct ServerHealth {
    #[serde(default)]
    pub ready: bool,
    #[serde(default)]
    pub model_loaded: bool,
    #[serde(default)]
    pub model_name: Option<String>,
}

/// Probe a rapid-mlx server's `/health`, 1s timeout. `base_url` may carry
/// the `/v1` suffix pi's models.json uses; health lives at the origin.
/// `None` means unreachable or not a rapid-mlx-shaped health response.
pub async fn server_health(base_url: &str) -> Option<ServerHealth> {
    let origin = base_url.trim_end_matches('/');
    let origin = origin.strip_suffix("/v1").unwrap_or(origin);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(1))
        .build()
        .ok()?;
    client
        .get(format!("{origin}/health"))
        .send()
        .await
        .ok()?
        .json::<ServerHealth>()
        .await
        .ok()
}

/// The `models.json` provider key used when no suitable entry exists yet.
pub const DEFAULT_PROVIDER_KEY: &str = "rapid-mlx";

/// Which `models.json` provider a served alias should be registered under.
///
/// An existing provider already pointing at the rapid-mlx port wins —
/// whatever the user named it (`rapid-mlx-local`, …) — because adding a
/// second provider for the same `baseUrl` would silently duplicate their
/// config. Falls back to [`DEFAULT_PROVIDER_KEY`].
pub fn provider_key_for_port(
    providers: Option<&serde_json::Map<String, serde_json::Value>>,
    port: u16,
) -> String {
    let needle = format!(":{port}");
    providers
        .and_then(|map| {
            map.iter()
                .find(|(_, entry)| {
                    entry
                        .get("baseUrl")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|url| url.contains(&needle) && is_loopback_url(url))
                })
                .map(|(key, _)| key.clone())
        })
        .unwrap_or_else(|| DEFAULT_PROVIDER_KEY.to_string())
}

fn is_loopback_url(url: &str) -> bool {
    ["localhost", "127.0.0.1", "0.0.0.0", "[::1]"]
        .iter()
        .any(|host| url.contains(host))
}

/// Builds (or extends) the rapid-mlx provider entry so pi can actually
/// select `alias` — spawning `rapid-mlx serve <alias>` tells pi nothing, so
/// without a matching `models.json` entry `set_model` fails with "Model not
/// found".
///
/// Merges into `existing` when it already looks like a provider entry,
/// rather than replacing it: only one rapid-mlx model is served at a time,
/// but pi should keep every alias registered so far, and any hand-edited
/// fields (a custom `baseUrl`, `contextWindow`, …) must survive. Registers
/// the **alias** as the model id, matching how this app serves.
pub fn provider_preset(
    existing: Option<&serde_json::Value>,
    port: u16,
    alias: &str,
) -> serde_json::Value {
    let mut preset = existing
        .filter(|v| v.get("models").is_some_and(serde_json::Value::is_array))
        .cloned()
        .unwrap_or_else(|| {
            serde_json::json!({
                "name": "Rapid-MLX Local",
                "baseUrl": format!("http://localhost:{port}/v1"),
                "api": "openai-completions",
                "apiKey": DEFAULT_PROVIDER_KEY,
                "compat": {
                    "supportsDeveloperRole": false,
                    "supportsReasoningEffort": false,
                },
                "models": [],
            })
        });
    let models = preset
        .get_mut("models")
        .and_then(serde_json::Value::as_array_mut)
        .expect("either filtered on `models` being an array, or just built fresh");
    let already_registered = models
        .iter()
        .any(|m| m.get("id").and_then(serde_json::Value::as_str) == Some(alias));
    if !already_registered {
        models.push(serde_json::json!({
            "id": alias,
            "name": format!("{alias} (Rapid-MLX)"),
            "contextWindow": 128000,
            "maxTokens": 32000,
        }));
    }
    preset
}

pub struct RapidMlx {
    binary: String,
}

impl Default for RapidMlx {
    fn default() -> Self {
        Self::new(DEFAULT_BINARY)
    }
}

impl RapidMlx {
    pub fn new(binary: impl Into<String>) -> Self {
        Self {
            binary: binary.into(),
        }
    }

    /// `rapid-mlx --version`, e.g. `"rapid-mlx 0.11.0"`. `None` means the
    /// binary isn't on PATH (or isn't rapid-mlx).
    pub async fn version(&self) -> Option<String> {
        let output = Command::new(&self.binary)
            .arg("--version")
            .output()
            .await
            .ok()?;
        if !output.status.success() {
            return None;
        }
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    pub async fn running_servers(&self) -> Result<Vec<RunningServer>, RapidMlxError> {
        Ok(parse_ps(&self.run(&["ps"]).await?))
    }

    pub async fn catalog(&self) -> Result<Vec<CatalogEntry>, RapidMlxError> {
        Ok(parse_catalog(&self.run(&["models"]).await?))
    }

    pub async fn cached_models(&self) -> Result<Vec<CachedModel>, RapidMlxError> {
        Ok(parse_cached(&self.run(&["models", "--cached"]).await?))
    }

    #[allow(dead_code)] // see AliasProfile's doc comment
    pub async fn info(&self, alias: &str) -> Result<AliasProfile, RapidMlxError> {
        let text = self.run(&["info", alias]).await?;
        parse_info(&text)
            .ok_or_else(|| RapidMlxError::Command(format!("unparseable `info` output for {alias}")))
    }

    async fn run(&self, args: &[&str]) -> Result<String, RapidMlxError> {
        let output = Command::new(&self.binary).args(args).output().await?;
        if !output.status.success() {
            return Err(RapidMlxError::Command(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

fn is_rule(line: &str) -> bool {
    let t = line.trim();
    !t.is_empty() && t.chars().all(|c| c == '─' || c == '-')
}

fn none_if_dash(s: &str) -> Option<String> {
    if s == "—" || s == "-" {
        None
    } else {
        Some(s.to_string())
    }
}

/// Lines between the line containing `header_marker` (the table's header
/// row) and the next blank/rule line, skipping one rule line right after
/// the header if present.
fn extract_table_rows<'a>(output: &'a str, header_marker: &str) -> Vec<&'a str> {
    let mut lines = output.lines();
    for line in &mut lines {
        if line.contains(header_marker) {
            break;
        }
    }
    let mut rest: Vec<&str> = lines.collect();
    if rest.first().is_some_and(|l| is_rule(l)) {
        rest.remove(0);
    }
    rest.into_iter()
        .take_while(|l| !l.trim().is_empty() && !is_rule(l))
        .collect()
}

fn parse_ps(output: &str) -> Vec<RunningServer> {
    extract_table_rows(output, "PORT")
        .into_iter()
        .filter_map(|line| {
            let mut tok = line.split_whitespace();
            let pid = tok.next()?.parse().ok()?;
            let port = tok.next()?.parse().ok()?;
            let model = tok.next()?.to_string();
            let uptime = tok.next().unwrap_or_default().to_string();
            Some(RunningServer {
                pid,
                port,
                model,
                uptime,
            })
        })
        .collect()
}

fn parse_catalog(output: &str) -> Vec<CatalogEntry> {
    // 0.11.3 inserted a `Size` column right after `Alias`. A row alone is
    // ambiguous (`—` size vs. `—` tool format shift the same way), so key
    // the format off the header row instead of guessing per row.
    let has_size = output
        .lines()
        .find(|l| l.contains("Spec-Decode"))
        .is_some_and(|l| l.split_whitespace().any(|t| t == "Size"));
    extract_table_rows(output, "Spec-Decode")
        .into_iter()
        .filter_map(|line| {
            let tokens: Vec<&str> = line.split_whitespace().collect();
            let (alias, rest) = tokens.split_first()?;
            let (size_bytes, rest) = if has_size {
                match rest {
                    ["—", rest @ ..] => (None, rest),
                    [value, unit, rest @ ..] => (size_to_bytes(value.parse().ok()?, unit), rest),
                    _ => return None,
                }
            } else {
                (None, rest)
            };
            // Tools, Reasoning, Spec-Decode, [hybrid], Suffix Tier, DFlash,
            // DDTree.
            let hybrid = match rest.len() {
                6 => false,
                7 => true,
                _ => return None,
            };
            Some(CatalogEntry {
                alias: alias.to_string(),
                size_bytes,
                tool_format: none_if_dash(rest[0]),
                reasoning_parser: none_if_dash(rest[1]),
                spec_decode: rest[2] == "✓",
                hybrid,
                suffix_tier: none_if_dash(rest[3 + hybrid as usize]),
                dflash: none_if_dash(rest[rest.len() - 2]),
                ddtree: none_if_dash(rest[rest.len() - 1]),
            })
        })
        .collect()
}

fn size_to_bytes(value: f64, unit: &str) -> Option<u64> {
    let mult: f64 = match unit {
        "B" => 1.0,
        "KiB" => 1024.0,
        "MiB" => 1024.0 * 1024.0,
        "GiB" => 1024.0 * 1024.0 * 1024.0,
        "TiB" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    Some((value * mult) as u64)
}

fn parse_cached(output: &str) -> Vec<CachedModel> {
    extract_table_rows(output, "HF repo")
        .into_iter()
        .filter_map(|line| {
            let tokens: Vec<&str> = line.split_whitespace().collect();
            if tokens.len() != 6 || tokens[5] != "ago" {
                return None;
            }
            let size_bytes = size_to_bytes(tokens[2].parse().ok()?, tokens[3])?;
            Some(CachedModel {
                alias: tokens[0].to_string(),
                hf_repo: tokens[1].to_string(),
                size_bytes,
                modified: format!("{} {}", tokens[4], tokens[5]),
            })
        })
        .collect()
}

#[allow(dead_code)] // see AliasProfile's doc comment
fn parse_info(output: &str) -> Option<AliasProfile> {
    let mut model_path = None;
    let mut fields = BTreeMap::new();
    let mut in_box = false;
    for raw in output.lines() {
        let line = raw.trim();
        if line.starts_with('┌') {
            in_box = true;
            continue;
        }
        if line.starts_with('└') {
            break; // only the first box; later boxes reuse field labels
        }
        if !in_box {
            continue;
        }
        let Some(inner) = line.strip_prefix('│').and_then(|s| s.strip_suffix('│')) else {
            continue;
        };
        let inner = inner.trim();
        if inner.is_empty() || inner.chars().all(|c| c == '─') {
            continue;
        }
        if let Some(rest) = inner.strip_prefix("Model:") {
            model_path = Some(rest.trim().to_string());
            continue;
        }
        if let Some((key, value)) = inner.split_once(':') {
            fields.insert(key.trim().to_string(), value.trim().to_string());
        }
    }
    if fields.is_empty() {
        None
    } else {
        Some(AliasProfile { model_path, fields })
    }
}

/// A supervised `rapid-mlx serve <model>` child (one model per process, per
/// upstream's design — no hot-swap). Owning the process lets the app switch
/// models by restarting it and surfacing that honestly as a restart, not a
/// hot swap (see M3 plan risks).
pub struct ManagedServer {
    child: Child,
    stdout_lines: Lines<BufReader<ChildStdout>>,
}

impl ManagedServer {
    pub fn spawn(binary: &str, model: &str, port: u16) -> Result<Self, RapidMlxError> {
        let mut child = Command::new(binary)
            .arg("serve")
            .arg(model)
            .arg("--port")
            .arg(port.to_string())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let stdout = child.stdout.take().expect("stdout piped");
        Ok(Self {
            child,
            stdout_lines: BufReader::new(stdout).lines(),
        })
    }

    /// Reads stdout lines until the `  Ready: http://...` marker (confirmed
    /// against a live `rapid-mlx serve` run — it follows an earlier
    /// "Starting server on ... (warming up)" line that is NOT the ready
    /// signal), the process exits, or `timeout` elapses.
    pub async fn wait_ready(&mut self, timeout: Duration) -> Result<(), RapidMlxError> {
        let lines = &mut self.stdout_lines;
        tokio::time::timeout(timeout, async move {
            loop {
                match lines.next_line().await? {
                    Some(line) if line.trim_start().starts_with("Ready:") => return Ok(()),
                    Some(_) => continue,
                    None => return Err(RapidMlxError::ExitedBeforeReady),
                }
            }
        })
        .await
        .unwrap_or(Err(RapidMlxError::ReadyTimeout))
    }

    /// Continue reading server log lines after `wait_ready` returns. Not
    /// used yet — the panel surfaces success/failure via `transcript.note`
    /// only; wiring this up is for a future live-progress display.
    #[allow(dead_code)]
    pub async fn next_log_line(&mut self) -> std::io::Result<Option<String>> {
        self.stdout_lines.next_line().await
    }

    /// Whether the child process is still running. `false` once it has
    /// exited (crash or external kill) — drives the status-bar server dot's
    /// needs-attention state.
    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    pub async fn shutdown(mut self) -> std::io::Result<()> {
        self.child.start_kill()?;
        self.child.wait().await?;
        Ok(())
    }
}

#[cfg(test)]
mod provider_tests {
    use super::*;

    #[test]
    fn reuses_an_existing_provider_on_the_same_port_whatever_its_name() {
        let providers = serde_json::json!({
            "rapid-mlx-local": {"baseUrl": "http://localhost:8000/v1"},
            "bifrost": {"baseUrl": "https://bifrost.example.com/openai"},
        });
        assert_eq!(
            provider_key_for_port(providers.as_object(), 8000),
            "rapid-mlx-local",
            "must merge into the user's existing entry, not add a duplicate provider"
        );
    }

    #[test]
    fn ignores_a_remote_provider_that_merely_shares_the_port_number() {
        let providers = serde_json::json!({
            "remote": {"baseUrl": "https://models.example.com:8000/v1"},
        });
        assert_eq!(
            provider_key_for_port(providers.as_object(), 8000),
            DEFAULT_PROVIDER_KEY
        );
    }

    #[test]
    fn falls_back_to_the_default_key_when_nothing_matches() {
        assert_eq!(provider_key_for_port(None, 8000), DEFAULT_PROVIDER_KEY);
        let providers = serde_json::json!({"bifrost": {"baseUrl": "https://x.example.com"}});
        assert_eq!(
            provider_key_for_port(providers.as_object(), 8000),
            DEFAULT_PROVIDER_KEY
        );
    }

    #[test]
    fn preset_builds_a_fresh_entry_when_none_exists() {
        let preset = provider_preset(None, 8000, "lfm2.5-1b-4bit");
        assert_eq!(preset["baseUrl"], "http://localhost:8000/v1");
        assert_eq!(preset["api"], "openai-completions");
        assert_eq!(preset["models"].as_array().unwrap().len(), 1);
        assert_eq!(preset["models"][0]["id"], "lfm2.5-1b-4bit");
    }

    #[test]
    fn preset_appends_without_dropping_previously_registered_aliases() {
        let existing = serde_json::json!({
            "name": "Rapid-MLX Local",
            "baseUrl": "http://localhost:8000/v1",
            "models": [{"id": "qwen3.5-9b-4bit", "name": "qwen3.5-9b-4bit (Rapid-MLX)"}],
        });
        let preset = provider_preset(Some(&existing), 8000, "lfm2.5-1b-4bit");
        let models = preset["models"].as_array().unwrap();
        assert_eq!(models.len(), 2, "the prior alias must survive");
        assert_eq!(models[0]["id"], "qwen3.5-9b-4bit");
        assert_eq!(models[1]["id"], "lfm2.5-1b-4bit");
        assert_eq!(
            preset["name"], "Rapid-MLX Local",
            "untouched fields survive"
        );
    }

    #[test]
    fn preset_does_not_duplicate_or_clobber_an_already_registered_alias() {
        let existing = serde_json::json!({
            "models": [{"id": "lfm2.5-1b-4bit", "name": "hand-edited name"}],
        });
        let preset = provider_preset(Some(&existing), 8000, "lfm2.5-1b-4bit");
        let models = preset["models"].as_array().unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0]["name"], "hand-edited name");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PS_FIXTURE: &str = "\
  PID     PORT    MODEL                                   UPTIME
  ------------------------------------------------------------------
  26101   8000    mlx-community/Qwen3.6-35B-A3B-8bit      2h11m
";

    // 0.11.3 format: `Size` column between `Alias` and `Tools`, "—" allowed
    // for unknown sizes.
    const CATALOG_FIXTURE: &str = "\
  Available models (173 aliases)
  ─────────────────────────────────────────────────────────────────────────────────────────────────────────────────
  Alias                             Size       Tools            Reasoning    Spec-Decode Suffix Tier DFlash  DDTree
  ─────────────────────────────────────────────────────────────────────────────────────────────────────────────────
  bonsai-1.7b-2bit                  472.6 MiB  hermes           —            ✗          unknown     —       —
  bonsai-27b-2bit                   7.9 GiB    hermes           qwen3        ✗          unknown     —       —
  deepseek-coder-v2-lite-16b-4bit   8.2 GiB    deepseek_v3      —            ✓          unknown     —       —
  diffusion-gemma-26b-4bit          15.4 GiB   gemma4           —            ✗ hybrid   n/a         —       —
  vibethinker-1.5b-4bit             —          hermes           vibethinker  ✓          unknown     —       —
  ─────────────────────────────────────────────────────────────────────────────────────────────────────────────────

  Audio models (41 aliases)
  ──────────────────────────────────────────────────────────────────────────────────────────────────────
  Alias                    Size       Kind       Family       HF id
  ──────────────────────────────────────────────────────────────────────────────────────────────────────
  kokoro-82m               313.2 MiB  [audio:tts] kokoro      mlx-community/Kokoro-82M-bf16
";

    // Pre-0.11.3 format (no `Size` column), still parsed for older installs.
    const CATALOG_FIXTURE_LEGACY: &str = "\
  Available models (165 aliases)
  ──────────────────────────────────────────────────────────────────────────────────────────────────────
  Alias                             Tools            Reasoning    Spec-Decode Suffix Tier DFlash  DDTree
  ──────────────────────────────────────────────────────────────────────────────────────────────────────
  bonsai-1.7b-2bit                  hermes           —            ✗          unknown     —       —
  diffusion-gemma-26b-4bit          gemma4           —            ✗ hybrid   n/a         —       —
  vibethinker-1.5b-4bit             hermes           vibethinker  ✓          unknown     —       —
  ──────────────────────────────────────────────────────────────────────────────────────────────────────
";

    const CACHED_FIXTURE: &str = "\
  Cached models (4 on disk)
  ────────────────────────────────────────────────────────────────────────────────────────────────
  Alias                  HF repo                                            Size      Modified
  ────────────────────────────────────────────────────────────────────────────────────────────────
  gpt-oss-120b           mlx-community/gpt-oss-120b-MXFP4-Q8                118.1 GiB 1d ago
  qwen3.6-35b-8bit       mlx-community/Qwen3.6-35B-A3B-8bit                 70.3 GiB  7h ago
  qwen3.5-4b-4bit        mlx-community/Qwen3.5-4B-MLX-4bit                  5.7 GiB   2d ago
  qwen3-4b-instruct-2507-4bit mlx-community/Qwen3-4B-Instruct-2507-4bit          4.2 GiB   2d ago
  ────────────────────────────────────────────────────────────────────────────────────────────────
  Total: 198.4 GiB

  Tip: `rapid-mlx rm <hf-repo>` to free disk space
";

    const INFO_FIXTURE: &str = "\
  Alias: qwen3.5-4b-4bit → mlx-community/Qwen3.5-4B-MLX-4bit

┌──────────────────────────────────────────────────────────────┐
│ Model: mlx-community/Qwen3.5-4B-MLX-4bit                     │
│ ──────────────────────────────────────────────────────────── │
│ Tool format      : hermes                                    │
│ Reasoning parser : qwen3                                     │
│ Architecture     : pure attention                            │
│ Spec decode      : ✗ disabled (no MTP/drafter trained)       │
│ MTP path         : disabled                                  │
│ KV-share         : no                                        │
│ Throttle         : ✗ not needed                              │
│ Suffix tier      : n/a (no MTP/drafter — spec decode off)    │
└──────────────────────────────────────────────────────────────┘

┌──────────────────────────────────────────────────────────────┐
│ DFlash eligibility: ✗ ineligible                             │
│ ──────────────────────────────────────────────────────────── │
│ Declared support  : ✗ no                                     │
└──────────────────────────────────────────────────────────────┘
";

    #[test]
    fn parses_ps_table() {
        let servers = parse_ps(PS_FIXTURE);
        assert_eq!(
            servers,
            vec![RunningServer {
                pid: 26101,
                port: 8000,
                model: "mlx-community/Qwen3.6-35B-A3B-8bit".into(),
                uptime: "2h11m".into(),
            }]
        );
    }

    #[test]
    fn parses_ps_table_with_no_running_servers() {
        let output = "  PID     PORT    MODEL                                   UPTIME    \n  ------------------------------------------------------------------\n";
        assert_eq!(parse_ps(output), vec![]);
    }

    #[test]
    fn parses_catalog_stopping_before_audio_section() {
        let entries = parse_catalog(CATALOG_FIXTURE);
        assert_eq!(entries.len(), 5);

        assert_eq!(
            entries[0],
            CatalogEntry {
                alias: "bonsai-1.7b-2bit".into(),
                size_bytes: Some((472.6 * 1024.0 * 1024.0) as u64),
                tool_format: Some("hermes".into()),
                reasoning_parser: None,
                spec_decode: false,
                hybrid: false,
                suffix_tier: Some("unknown".into()),
                dflash: None,
                ddtree: None,
            }
        );

        let deepseek = entries
            .iter()
            .find(|e| e.alias.starts_with("deepseek"))
            .unwrap();
        assert!(deepseek.spec_decode);

        let hybrid_entry = entries
            .iter()
            .find(|e| e.alias == "diffusion-gemma-26b-4bit")
            .unwrap();
        assert!(hybrid_entry.hybrid);
        assert!(!hybrid_entry.spec_decode);
        assert_eq!(hybrid_entry.suffix_tier.as_deref(), Some("n/a"));

        // "—" size parses as unknown, not a dropped row.
        let unknown_size = entries
            .iter()
            .find(|e| e.alias == "vibethinker-1.5b-4bit")
            .unwrap();
        assert_eq!(unknown_size.size_bytes, None);
        assert_eq!(
            unknown_size.reasoning_parser.as_deref(),
            Some("vibethinker")
        );
    }

    #[test]
    fn parses_legacy_catalog_without_size_column() {
        let entries = parse_catalog(CATALOG_FIXTURE_LEGACY);
        assert_eq!(entries.len(), 3);
        assert!(entries.iter().all(|e| e.size_bytes.is_none()));
        // The legacy "—" tool-format token must not be eaten as a size.
        assert_eq!(
            entries
                .iter()
                .find(|e| e.alias == "diffusion-gemma-26b-4bit")
                .map(|e| (e.hybrid, e.tool_format.as_deref() == Some("gemma4"))),
            Some((true, true))
        );
    }

    #[test]
    fn parses_cached_models_despite_alias_column_overflow() {
        let cached = parse_cached(CACHED_FIXTURE);
        assert_eq!(cached.len(), 4);

        let overflowing = cached
            .iter()
            .find(|c| c.alias == "qwen3-4b-instruct-2507-4bit")
            .expect("row with an alias wider than the rendered column still parses");
        assert_eq!(
            overflowing.hf_repo,
            "mlx-community/Qwen3-4B-Instruct-2507-4bit"
        );
        assert_eq!(overflowing.modified, "2d ago");
        assert_eq!(
            overflowing.size_bytes,
            (4.2 * 1024.0 * 1024.0 * 1024.0) as u64
        );

        let biggest = cached.iter().find(|c| c.alias == "gpt-oss-120b").unwrap();
        assert_eq!(
            biggest.size_bytes,
            (118.1 * 1024.0 * 1024.0 * 1024.0) as u64
        );
    }

    #[test]
    fn parses_info_first_box_only() {
        let profile = parse_info(INFO_FIXTURE).expect("parses");
        assert_eq!(
            profile.model_path.as_deref(),
            Some("mlx-community/Qwen3.5-4B-MLX-4bit")
        );
        assert_eq!(profile.tool_format(), Some("hermes"));
        assert_eq!(profile.reasoning_parser(), Some("qwen3"));
        assert!(!profile.spec_decode_enabled());
        // The DFlash box's "Declared support" must not appear (would collide
        // with nothing here, but confirms we stopped at the first box).
        assert!(profile.field("DFlash eligibility").is_none());
        assert!(profile.field("Declared support").is_none());
    }

    #[test]
    fn parse_info_returns_none_for_garbage() {
        assert!(parse_info("not a rapid-mlx info output at all").is_none());
    }

    fn rapid_mlx_available() -> bool {
        std::process::Command::new(DEFAULT_BINARY)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Extracts the `N` out of a rapid-mlx section title like `"Available
    /// models (165 aliases)"` or `"Cached models (4 on disk)"` — the
    /// section's own claimed row count, used as an oracle to catch the
    /// token-count parsers silently dropping rows they don't recognize.
    fn claimed_row_count(text: &str, marker: &str) -> Option<usize> {
        let line = text.lines().find(|l| l.contains(marker))?;
        let after_paren = line.split('(').nth(1)?;
        let digits: String = after_paren
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        digits.parse().ok()
    }

    #[tokio::test]
    async fn live_introspection_round_trips_when_installed() {
        if !rapid_mlx_available() {
            eprintln!("skipping: rapid-mlx binary not found");
            return;
        }
        let rmlx = RapidMlx::default();
        let version = rmlx.version().await.expect("version");
        assert!(version.contains("rapid-mlx"));

        // Read-only introspection only — no `pull`/`serve` here, which would
        // download models or spawn a real server.
        let catalog_text = rmlx.run(&["models"]).await.expect("models");
        let catalog = parse_catalog(&catalog_text);
        let claimed = claimed_row_count(&catalog_text, "Available models")
            .expect("catalog header reports its own row count");
        assert_eq!(
            catalog.len(),
            claimed,
            "parser dropped {} of {claimed} catalog rows — a column no longer matches the \
             expected token count (see the module doc comment on the token-counting approach)",
            claimed.saturating_sub(catalog.len())
        );

        let cached_text = rmlx
            .run(&["models", "--cached"])
            .await
            .expect("cached models");
        let cached = parse_cached(&cached_text);
        if let Some(claimed_cached) = claimed_row_count(&cached_text, "Cached models") {
            assert_eq!(
                cached.len(),
                claimed_cached,
                "parser dropped cached-model rows: the `Modified` column may not always end in \
                 \"ago\""
            );
        }

        let running = rmlx.running_servers().await.expect("ps");
        // Not asserting on contents: legitimately empty if nothing's running.
        let _ = running;
    }
}
