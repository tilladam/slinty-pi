//! Integration tests against a real `pi` binary. Skipped (with a note) when pi
//! is not installed. These stay offline and session-less: no LLM calls, no
//! session files written, extensions disabled for isolation.

use std::time::Duration;

use pi_rpc::{PiClient, PiOptions};

fn pi_available() -> bool {
    std::process::Command::new("pi")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn test_options() -> PiOptions {
    PiOptions {
        extra_args: vec![
            "--no-session".into(),
            "--offline".into(),
            "--no-extensions".into(),
            "--no-skills".into(),
        ],
        request_timeout: Duration::from_secs(20),
        ..Default::default()
    }
}

#[tokio::test]
async fn get_state_round_trips() {
    if !pi_available() {
        eprintln!("skipping: pi binary not found");
        return;
    }
    let (client, _events) = PiClient::spawn(test_options()).await.expect("spawn pi");
    let state = client.get_state().await.expect("get_state");
    assert!(
        state.get("isStreaming").is_some(),
        "state should contain isStreaming, got: {state}"
    );
    assert_eq!(state["isStreaming"], false);
}

#[tokio::test]
async fn get_available_models_round_trips() {
    if !pi_available() {
        eprintln!("skipping: pi binary not found");
        return;
    }
    let (client, _events) = PiClient::spawn(test_options()).await.expect("spawn pi");
    let data = client.get_available_models().await.expect("models");
    assert!(
        data.get("models").and_then(|m| m.as_array()).is_some(),
        "expected a models array, got: {data}"
    );
}

#[tokio::test]
async fn failed_command_reports_error() {
    if !pi_available() {
        eprintln!("skipping: pi binary not found");
        return;
    }
    let (client, _events) = PiClient::spawn(test_options()).await.expect("spawn pi");
    let err = client
        .set_model("no-such-provider", "no-such-model")
        .await
        .expect_err("bogus model must fail");
    let msg = err.to_string();
    assert!(msg.contains("set_model"), "unexpected error: {msg}");
}
