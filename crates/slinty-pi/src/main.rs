//! slinty-pi: native desktop app for the pi coding agent.
//!
//! Slint owns the main thread; a tokio runtime on background threads owns the
//! pi child process (see `backend`). `SLINTY_DEMO=1` runs a synthetic stream
//! through the same rendering path instead of spawning pi.

mod backend;
mod highlight;
mod segmenter;

use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use slint::{ComponentHandle, ModelRc, VecModel};
use tokio::sync::mpsc;

use backend::UiCmd;

slint::include_modules!();

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let app = AppWindow::new()?;
    let transcript: Rc<VecModel<Row>> = Rc::new(VecModel::default());
    app.set_transcript(ModelRc::from(transcript.clone()));

    let dark = Arc::new(AtomicBool::new(app.get_dark_mode()));

    let rt = tokio::runtime::Runtime::new()?;
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<UiCmd>();

    {
        let tx = cmd_tx.clone();
        app.on_send(move |text| {
            let _ = tx.send(UiCmd::Send(text.to_string()));
        });
        let tx = cmd_tx.clone();
        app.on_abort(move || {
            let _ = tx.send(UiCmd::Abort);
        });
        let tx = cmd_tx.clone();
        app.on_model_selected(move |i| {
            if i >= 0 {
                let _ = tx.send(UiCmd::SetModel(i as usize));
            }
        });
        let tx = cmd_tx.clone();
        app.on_thinking_selected(move |i| {
            if i >= 0 {
                let _ = tx.send(UiCmd::SetThinking(i as usize));
            }
        });
        app.on_copy_text(move |text| {
            if let Ok(mut clipboard) = arboard::Clipboard::new() {
                let _ = clipboard.set_text(text.to_string());
            }
        });
        app.on_open_link(move |link| open_url(&link));
        let weak = app.as_weak();
        app.on_composer_edited(move |text| {
            if let Some(app) = weak.upgrade() {
                app.set_composer_lines(text.chars().filter(|c| *c == '\n').count() as i32 + 1);
            }
        });
    }

    let weak = app.as_weak();
    let dark_flag = dark.clone();
    if std::env::var("SLINTY_DEMO").is_ok() {
        rt.spawn(backend::demo_backend(weak, dark_flag, cmd_rx));
    } else {
        rt.spawn(backend::pi_backend(weak, dark_flag, cmd_rx));
    }

    // Keep the highlighter's theme choice in sync with the OS color scheme.
    {
        let dark = dark.clone();
        let weak = app.as_weak();
        let timer = slint::Timer::default();
        timer.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_secs(2),
            move || {
                if let Some(app) = weak.upgrade() {
                    dark.store(app.get_dark_mode(), Ordering::Relaxed);
                }
            },
        );
        // Leak the timer so it keeps firing for the app's lifetime.
        std::mem::forget(timer);
    }

    app.invoke_focus_input();
    app.run()?;
    rt.shutdown_background();
    Ok(())
}

fn open_url(url: &str) {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return;
    }
    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(target_os = "linux")]
    let cmd = "xdg-open";
    #[cfg(target_os = "windows")]
    let cmd = "explorer";
    let _ = std::process::Command::new(cmd).arg(url).spawn();
}
