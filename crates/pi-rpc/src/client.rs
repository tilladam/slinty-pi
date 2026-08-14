use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command as ProcessCommand;
use tokio::sync::{mpsc, oneshot};

use crate::types::{
    Command, Event, ExtensionUiReply, ImageContent, Response, StreamingBehavior, ThinkingLevel,
};

#[derive(Debug, thiserror::Error)]
pub enum PiError {
    #[error("failed to spawn pi: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("pi process exited or channel closed")]
    Closed,
    #[error("timed out waiting for response to `{0}`")]
    Timeout(String),
    #[error("command `{command}` failed: {message}")]
    Command { command: String, message: String },
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
}

/// Runs `pi update --models` ("Refresh model catalogs only").
///
/// Not an RPC call — a separate one-shot invocation of the same binary this
/// module otherwise spawns for `--mode rpc`, which is why it lives here. pi
/// caches its model catalog, so a freshly-written `models.json` entry stays
/// invisible until this runs *and* a new child starts; both frontends need
/// exactly that sequence after registering a local model.
pub async fn refresh_model_catalog(binary: &str) -> Result<(), PiError> {
    let output = ProcessCommand::new(binary)
        .arg("update")
        .arg("--models")
        .output()
        .await
        .map_err(PiError::Spawn)?;
    if output.status.success() {
        return Ok(());
    }
    Err(PiError::Command {
        command: "update --models".to_string(),
        message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
    })
}

/// Options for spawning `pi --mode rpc`.
#[derive(Debug, Clone)]
pub struct PiOptions {
    /// Path or name of the pi binary.
    pub binary: String,
    /// Working directory for the agent (determines session storage and context).
    pub cwd: Option<PathBuf>,
    /// Extra CLI arguments appended after `--mode rpc` (e.g. `--no-session`).
    pub extra_args: Vec<String>,
    /// Timeout for command responses.
    pub request_timeout: Duration,
}

impl Default for PiOptions {
    fn default() -> Self {
        Self {
            binary: "pi".into(),
            cwd: None,
            extra_args: vec![],
            request_timeout: Duration::from_secs(30),
        }
    }
}

type Pending = Arc<Mutex<HashMap<String, oneshot::Sender<Response>>>>;

/// Handle to a running `pi --mode rpc` child process.
///
/// Commands are correlated by request id; everything else pi emits arrives on
/// the event receiver returned by [`PiClient::spawn`]. The child is killed when
/// the client is dropped.
pub struct PiClient {
    stdin_tx: mpsc::UnboundedSender<String>,
    pending: Pending,
    next_id: AtomicU64,
    request_timeout: Duration,
}

impl PiClient {
    /// Spawn pi and return the client plus the stream of agent events.
    pub async fn spawn(opts: PiOptions) -> Result<(Self, mpsc::UnboundedReceiver<Event>), PiError> {
        let mut cmd = ProcessCommand::new(&opts.binary);
        cmd.arg("--mode")
            .arg("rpc")
            .args(&opts.extra_args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(cwd) = &opts.cwd {
            cmd.current_dir(cwd);
        }
        let mut child = cmd.spawn().map_err(PiError::Spawn)?;

        let mut stdin = child.stdin.take().expect("stdin piped");
        let stdout = child.stdout.take().expect("stdout piped");
        let stderr = child.stderr.take().expect("stderr piped");

        let (stdin_tx, mut stdin_rx) = mpsc::unbounded_channel::<String>();
        let (event_tx, event_rx) = mpsc::unbounded_channel::<Event>();
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));

        // Writer task: serialized lines -> child stdin.
        tokio::spawn(async move {
            while let Some(line) = stdin_rx.recv().await {
                if stdin.write_all(line.as_bytes()).await.is_err() {
                    break;
                }
                if stdin.write_all(b"\n").await.is_err() {
                    break;
                }
                let _ = stdin.flush().await;
            }
        });

        // Stderr task: forward to tracing for diagnostics.
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!(target: "pi_rpc::stderr", "{line}");
            }
        });

        // Reader task: route responses to pending requests, events to the channel.
        // tokio's `lines()` splits on `\n` and strips a trailing `\r`, which
        // matches pi's strict JSONL framing (U+2028/2029 stay inside strings).
        let pending_reader = Arc::clone(&pending);
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if line.is_empty() {
                    continue;
                }
                let value: Value = match serde_json::from_str(&line) {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!("unparseable line from pi: {e}: {line}");
                        continue;
                    }
                };
                if value.get("type").and_then(Value::as_str) == Some("response") {
                    match serde_json::from_value::<Response>(value) {
                        Ok(resp) => {
                            let waiter = resp
                                .id
                                .as_ref()
                                .and_then(|id| pending_reader.lock().unwrap().remove(id));
                            match waiter {
                                Some(tx) => {
                                    let _ = tx.send(resp);
                                }
                                None => tracing::debug!(
                                    "response without waiter: {} success={}",
                                    resp.command,
                                    resp.success
                                ),
                            }
                        }
                        Err(e) => tracing::warn!("bad response from pi: {e}"),
                    }
                } else {
                    match serde_json::from_value::<Event>(value) {
                        Ok(ev) => {
                            if event_tx.send(ev).is_err() {
                                break;
                            }
                        }
                        Err(e) => tracing::warn!("bad event from pi: {e}: {line}"),
                    }
                }
            }
            // EOF: fail all in-flight requests.
            pending_reader.lock().unwrap().clear();
            // Reap the child so it doesn't linger as a zombie.
            let _ = child.wait().await;
        });

        Ok((
            Self {
                stdin_tx,
                pending,
                next_id: AtomicU64::new(1),
                request_timeout: opts.request_timeout,
            },
            event_rx,
        ))
    }

    /// Send a command and wait for its correlated response.
    /// Returns the response `data` payload (if any) on success.
    pub async fn request(&self, command: Command) -> Result<Option<Value>, PiError> {
        let id = format!("req-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let mut value = serde_json::to_value(&command)?;
        value
            .as_object_mut()
            .expect("commands serialize to objects")
            .insert("id".into(), Value::String(id.clone()));

        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id.clone(), tx);
        self.stdin_tx
            .send(serde_json::to_string(&value)?)
            .map_err(|_| PiError::Closed)?;

        let resp = match tokio::time::timeout(self.request_timeout, rx).await {
            Ok(Ok(resp)) => resp,
            Ok(Err(_)) => {
                self.pending.lock().unwrap().remove(&id);
                return Err(PiError::Closed);
            }
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                return Err(PiError::Timeout(command_name(&value)));
            }
        };

        if resp.success {
            Ok(resp.data)
        } else {
            Err(PiError::Command {
                command: resp.command,
                message: resp.error.unwrap_or_else(|| "unknown error".into()),
            })
        }
    }

    /// Answer a dialog-type extension UI request (`select`, `confirm`, `input`, `editor`).
    pub fn reply_extension_ui(
        &self,
        request_id: &str,
        reply: ExtensionUiReply,
    ) -> Result<(), PiError> {
        let mut obj = serde_json::Map::new();
        obj.insert("type".into(), "extension_ui_response".into());
        obj.insert("id".into(), request_id.into());
        match reply {
            ExtensionUiReply::Value(v) => {
                obj.insert("value".into(), v.into());
            }
            ExtensionUiReply::Confirmed(c) => {
                obj.insert("confirmed".into(), c.into());
            }
            ExtensionUiReply::Cancelled => {
                obj.insert("cancelled".into(), true.into());
            }
        }
        self.stdin_tx
            .send(serde_json::to_string(&Value::Object(obj))?)
            .map_err(|_| PiError::Closed)
    }

    // Convenience wrappers for the common commands.

    pub async fn prompt(&self, message: impl Into<String>) -> Result<(), PiError> {
        self.request(Command::Prompt {
            message: message.into(),
            images: None,
            streaming_behavior: None,
        })
        .await
        .map(drop)
    }

    /// Prompt with inline image attachments (composer attach button).
    pub async fn prompt_with_images(
        &self,
        message: impl Into<String>,
        images: Vec<ImageContent>,
    ) -> Result<(), PiError> {
        self.request(Command::Prompt {
            message: message.into(),
            images: if images.is_empty() {
                None
            } else {
                Some(images)
            },
            streaming_behavior: None,
        })
        .await
        .map(drop)
    }

    /// Prompt while the agent is streaming, queueing as a steering message.
    pub async fn prompt_steering(&self, message: impl Into<String>) -> Result<(), PiError> {
        self.request(Command::Prompt {
            message: message.into(),
            images: None,
            streaming_behavior: Some(StreamingBehavior::Steer),
        })
        .await
        .map(drop)
    }

    pub async fn abort(&self) -> Result<(), PiError> {
        self.request(Command::Abort).await.map(drop)
    }

    pub async fn get_state(&self) -> Result<Value, PiError> {
        Ok(self.request(Command::GetState).await?.unwrap_or_default())
    }

    pub async fn get_available_models(&self) -> Result<Value, PiError> {
        Ok(self
            .request(Command::GetAvailableModels)
            .await?
            .unwrap_or_default())
    }

    pub async fn set_model(&self, provider: &str, model_id: &str) -> Result<Value, PiError> {
        Ok(self
            .request(Command::SetModel {
                provider: provider.into(),
                model_id: model_id.into(),
            })
            .await?
            .unwrap_or_default())
    }

    pub async fn set_thinking_level(&self, level: ThinkingLevel) -> Result<(), PiError> {
        self.request(Command::SetThinkingLevel { level })
            .await
            .map(drop)
    }

    pub async fn get_session_stats(&self) -> Result<Value, PiError> {
        Ok(self
            .request(Command::GetSessionStats)
            .await?
            .unwrap_or_default())
    }

    pub async fn get_messages(&self) -> Result<Value, PiError> {
        Ok(self
            .request(Command::GetMessages)
            .await?
            .unwrap_or_default())
    }

    /// Load a different session file (the child stays the same process; only
    /// valid within the same cwd — a project change requires a respawn).
    pub async fn switch_session(&self, session_path: impl Into<String>) -> Result<Value, PiError> {
        Ok(self
            .request(Command::SwitchSession {
                session_path: session_path.into(),
            })
            .await?
            .unwrap_or_default())
    }

    pub async fn new_session(&self, parent_session: Option<String>) -> Result<Value, PiError> {
        Ok(self
            .request(Command::NewSession { parent_session })
            .await?
            .unwrap_or_default())
    }

    /// Fork from a previous user message on the active branch. Returns the
    /// forked-from prompt text.
    pub async fn fork(&self, entry_id: impl Into<String>) -> Result<Value, PiError> {
        Ok(self
            .request(Command::Fork {
                entry_id: entry_id.into(),
            })
            .await?
            .unwrap_or_default())
    }

    pub async fn clone_session(&self) -> Result<Value, PiError> {
        Ok(self.request(Command::Clone).await?.unwrap_or_default())
    }

    pub async fn get_fork_messages(&self) -> Result<Value, PiError> {
        Ok(self
            .request(Command::GetForkMessages)
            .await?
            .unwrap_or_default())
    }

    pub async fn get_tree(&self) -> Result<Value, PiError> {
        Ok(self.request(Command::GetTree).await?.unwrap_or_default())
    }

    pub async fn set_session_name(&self, name: impl Into<String>) -> Result<(), PiError> {
        self.request(Command::SetSessionName { name: name.into() })
            .await
            .map(drop)
    }

    pub async fn get_commands(&self) -> Result<Value, PiError> {
        Ok(self
            .request(Command::GetCommands)
            .await?
            .unwrap_or_default())
    }
}

fn command_name(value: &Value) -> String {
    value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("?")
        .to_string()
}
