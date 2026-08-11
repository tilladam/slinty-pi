//! Hugging Face Hub model search, feeding the router section's "Download
//! model…" flow. `GET https://huggingface.co/api/models?search=…&filter=gguf`
//! (verified live against the real API as of 2026-07); `full=true` is what
//! makes `siblings` (the repo's file list, needed for quant extraction) and
//! `gated` appear in the response — they're absent from the default search
//! shape. `HF_TOKEN` is sent as a bearer token when present, matching the
//! Hub's own convention (raises the anonymous rate limit and unlocks gated
//! repos the token has accepted).

use std::time::Duration;

use serde::Deserialize;

pub const SEARCH_URL: &str = "https://huggingface.co/api/models";

#[derive(Debug, thiserror::Error)]
pub enum HfError {
    #[error("Hugging Face search failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("Hugging Face returned HTTP {0}")]
    Status(u16),
}

/// `gated` is `false` for a public repo, or a string (`"auto"`/`"manual"`)
/// naming the gate kind — never a bare `true` (verified live against
/// `meta-llama/Llama-3.1-8B`, which reports `"manual"`).
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Gated {
    Bool(bool),
    // The panel only shows a generic gated warning today (see
    // `models.slint`'s `HfResultItem`) — the "auto"/"manual" distinction
    // isn't surfaced yet, so this field is only ever matched on, not read.
    #[allow(dead_code)]
    Kind(String),
}

impl Default for Gated {
    fn default() -> Self {
        Gated::Bool(false)
    }
}

impl Gated {
    pub fn is_gated(&self) -> bool {
        !matches!(self, Gated::Bool(false))
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Sibling {
    pub rfilename: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HfModel {
    pub id: String,
    #[serde(default)]
    pub gated: Gated,
    #[serde(default)]
    pub downloads: u64,
    #[serde(default)]
    pub siblings: Vec<Sibling>,
}

pub struct HfSearch {
    client: reqwest::Client,
    token: Option<String>,
}

impl Default for HfSearch {
    fn default() -> Self {
        Self::new()
    }
}

impl HfSearch {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .expect("reqwest client builds"),
            token: std::env::var("HF_TOKEN").ok(),
        }
    }

    /// Search GGUF repos matching `query`. `full=true` pulls in
    /// `siblings`/`gated` (see the module doc comment) at the cost of a
    /// heavier response, which is fine for a bounded `limit`.
    pub async fn search_gguf(&self, query: &str, limit: u32) -> Result<Vec<HfModel>, HfError> {
        let mut req = self.client.get(SEARCH_URL).query(&[
            ("search", query),
            ("filter", "gguf"),
            ("full", "true"),
            ("limit", &limit.to_string()),
        ]);
        if let Some(token) = &self.token {
            req = req.bearer_auth(token);
        }
        let resp = req.send().await?;
        if !resp.status().is_success() {
            return Err(HfError::Status(resp.status().as_u16()));
        }
        Ok(resp.json().await?)
    }
}

fn is_quant_token(tok: &str) -> bool {
    let upper = tok.to_ascii_uppercase();
    if upper == "BF16" || upper == "F16" || upper == "F32" {
        return true;
    }
    let Some(rest) = upper.strip_prefix("IQ").or_else(|| upper.strip_prefix('Q')) else {
        return false;
    };
    rest.starts_with(|c: char| c.is_ascii_digit())
}

/// Extracts the quant label from one `.gguf` sibling filename by scanning
/// its `-`/`.`/`/`-separated segments for a known quant-token shape, rather
/// than diffing filenames against each other (a longest-common-prefix
/// approach breaks on a single-file repo, where the "common prefix" of one
/// filename is the whole filename — verified against a real single-file
/// repo, `TinyLlama/TinyLlama-1.1B-Chat-v0.6`). Splitting on `/` too handles
/// repos that shard large models into per-quant subdirectories (verified
/// against `unsloth/gpt-oss-120b-GGUF`'s
/// `Q4_K_M/gpt-oss-120b-Q4_K_M-00001-of-00002.gguf` layout) without treating
/// the shard suffix (`00001`, `of`, `00002`) as a quant.
fn quant_from_filename(filename: &str) -> Option<String> {
    if !filename.to_ascii_lowercase().ends_with(".gguf") {
        return None;
    }
    // ".gguf"/".GGUF" are both 5 ASCII bytes, so slicing the original
    // (case-preserved) string by that length is safe here.
    let stem = &filename[..filename.len() - 5];
    stem.split(['-', '.', '/'])
        .rfind(|seg| is_quant_token(seg))
        .map(|s| s.to_ascii_uppercase())
}

/// De-duplicated, sorted quant labels available for `model`, skipping
/// `mmproj` files (multimodal-projector side files, not a selectable model
/// quantization).
pub fn gguf_quants(model: &HfModel) -> Vec<String> {
    let mut quants: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for sibling in &model.siblings {
        let name = &sibling.rfilename;
        let lower = name.to_ascii_lowercase();
        if !lower.ends_with(".gguf") || lower.contains("mmproj") {
            continue;
        }
        if let Some(quant) = quant_from_filename(name) {
            quants.insert(quant);
        }
    }
    quants.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn siblings(names: &[&str]) -> Vec<Sibling> {
        names
            .iter()
            .map(|n| Sibling {
                rfilename: n.to_string(),
            })
            .collect()
    }

    fn model_with(siblings: Vec<Sibling>) -> HfModel {
        HfModel {
            id: "test/repo".to_string(),
            gated: Gated::Bool(false),
            downloads: 0,
            siblings,
        }
    }

    #[test]
    fn extracts_quants_from_a_flat_multi_file_repo() {
        // unsloth/gemma-3-4b-it-GGUF, fetched live 2026-07-28.
        let model = model_with(siblings(&[
            "gemma-3-4b-it-BF16.gguf",
            "gemma-3-4b-it-IQ4_NL.gguf",
            "gemma-3-4b-it-Q4_K_M.gguf",
            "gemma-3-4b-it-UD-IQ1_M.gguf",
            "gemma-3-4b-it-UD-Q2_K_XL.gguf",
            "mmproj-BF16.gguf",
            "mmproj-F16.gguf",
            "README.md",
            "config.json",
        ]));
        assert_eq!(
            gguf_quants(&model),
            vec!["BF16", "IQ1_M", "IQ4_NL", "Q2_K_XL", "Q4_K_M"]
        );
    }

    #[test]
    fn extracts_the_single_quant_from_a_single_file_repo() {
        // TinyLlama/TinyLlama-1.1B-Chat-v0.6, fetched live 2026-07-28 — a
        // longest-common-prefix approach would return an empty string here,
        // since the "common prefix" of one filename is the whole filename.
        let model = model_with(siblings(&["ggml-model-q4_0.gguf"]));
        assert_eq!(gguf_quants(&model), vec!["Q4_0"]);
    }

    #[test]
    fn dedupes_sharded_quants_and_ignores_shard_suffixes() {
        // unsloth/gpt-oss-120b-GGUF, fetched live 2026-07-28 — each quant is
        // split across two shards in its own subdirectory; the shard suffix
        // (00001/of/00002) must not be mistaken for a quant token.
        let model = model_with(siblings(&[
            "Q4_K_M/gpt-oss-120b-Q4_K_M-00001-of-00002.gguf",
            "Q4_K_M/gpt-oss-120b-Q4_K_M-00002-of-00002.gguf",
            "Q5_K_M/gpt-oss-120b-Q5_K_M-00001-of-00002.gguf",
            "Q5_K_M/gpt-oss-120b-Q5_K_M-00002-of-00002.gguf",
        ]));
        assert_eq!(gguf_quants(&model), vec!["Q4_K_M", "Q5_K_M"]);
    }

    #[test]
    fn gated_distinguishes_public_from_gated() {
        assert!(!Gated::Bool(false).is_gated());
        assert!(Gated::Kind("manual".to_string()).is_gated());
        assert!(Gated::Kind("auto".to_string()).is_gated());
    }

    #[test]
    fn deserializes_the_gated_field_shapes_seen_live() {
        let public: Gated = serde_json::from_str("false").unwrap();
        assert!(!public.is_gated());
        let gated: Gated = serde_json::from_str("\"manual\"").unwrap();
        assert!(gated.is_gated());
    }

    /// Round-trips the real HTTP call + JSON deserialization against the
    /// live API (unlike the fixture tests above, which hand-type JSON based
    /// on a manual `curl` inspection and so can't catch a mismatch between
    /// this module's structs and reqwest/serde's actual behavior). Skips on
    /// a connection-level failure, like `rapid_mlx`'s live test skips when
    /// the binary isn't installed — a sandboxed/offline CI runner isn't a
    /// regression here. Any other failure (deserialization, unexpected
    /// shape) fails loudly.
    #[tokio::test]
    async fn live_search_round_trips_against_the_real_api() {
        let results = match HfSearch::default().search_gguf("gemma-3-4b-it", 5).await {
            Ok(results) => results,
            Err(HfError::Http(e)) if e.is_connect() || e.is_timeout() => {
                eprintln!("skipping: no network access ({e})");
                return;
            }
            Err(e) => panic!("live Hugging Face search failed: {e}"),
        };
        assert!(!results.is_empty(), "expected at least one GGUF result");
        let with_quants = results.iter().find(|m| !gguf_quants(m).is_empty());
        assert!(
            with_quants.is_some(),
            "expected at least one result with a parseable quant from its siblings"
        );
    }
}
