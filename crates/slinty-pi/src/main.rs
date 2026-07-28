//! slinty-pi: native desktop app for the pi coding agent.
//!
//! Slint owns the main thread; a tokio runtime on background threads owns the
//! pi child process (see `backend`). `SLINTY_DEMO=1` runs a synthetic stream
//! through the same rendering path instead of spawning pi.

mod attach;
mod backend;
mod demo_sessions;
mod density;
mod highlight;
mod local;
mod palette;
mod segmenter;

use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use slint::{ComponentHandle, ModelRc, VecModel};
use tokio::sync::mpsc;

use backend::UiCmd;

slint::include_modules!();

/// Forwards winit's `WindowEvent::DroppedFile` (real Finder/Explorer/file-manager
/// drops) into the same `UiCmd::AttachPath` the attach button sends per picked
/// file. Slint's own winit event loop never surfaces this event (see
/// `attach.rs`), so this hook is the only way to see it — it runs before
/// Slint's handling and only ever forwards, never suppresses.
struct DropFileHandler {
    tx: mpsc::UnboundedSender<UiCmd>,
}

impl i_slint_backend_winit::CustomApplicationHandler for DropFileHandler {
    fn window_event(
        &mut self,
        _event_loop: &winit::event_loop::ActiveEventLoop,
        _window_id: winit::window::WindowId,
        _winit_window: Option<&winit::window::Window>,
        _slint_window: Option<&slint::Window>,
        event: &winit::event::WindowEvent,
    ) -> i_slint_backend_winit::EventResult {
        if let winit::event::WindowEvent::DroppedFile(path) = event {
            let _ = self.tx.send(UiCmd::AttachPath(path.clone()));
        }
        i_slint_backend_winit::EventResult::Propagate
    }
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let rt = tokio::runtime::Runtime::new()?;
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<UiCmd>();

    // Must run before any Slint window is created (AppWindow::new() below
    // lazily picks the default platform otherwise).
    let backend = i_slint_backend_winit::Backend::builder()
        .with_custom_application_handler(Box::new(DropFileHandler { tx: cmd_tx.clone() }))
        .build()?;
    slint::platform::set_platform(Box::new(backend))
        .map_err(|e| anyhow::anyhow!("failed to install winit platform: {e}"))?;

    let app = AppWindow::new()?;
    let transcript: Rc<VecModel<Row>> = Rc::new(VecModel::default());
    app.set_transcript(ModelRc::from(transcript.clone()));

    let dark = Arc::new(AtomicBool::new(app.get_dark_mode()));

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
        let tx = cmd_tx.clone();
        app.on_project_selected(move |path| {
            let _ = tx.send(UiCmd::SwitchProject(std::path::PathBuf::from(
                path.as_str(),
            )));
        });
        let tx = cmd_tx.clone();
        app.on_new_session(move || {
            let _ = tx.send(UiCmd::NewSession);
        });
        let tx = cmd_tx.clone();
        app.on_resume_session(move |path| {
            let _ = tx.send(UiCmd::SwitchSession(path.to_string()));
        });
        let tx = cmd_tx.clone();
        app.on_delete_session(move |path| {
            let _ = tx.send(UiCmd::DeleteSession(path.to_string()));
        });
        let tx = cmd_tx.clone();
        app.on_rename_session(move |name| {
            let _ = tx.send(UiCmd::RenameSession(name.to_string()));
        });
        let tx = cmd_tx.clone();
        app.on_sidebar_search_edited(move |query| {
            let _ = tx.send(UiCmd::SidebarSearch(query.to_string()));
        });
        let tx = cmd_tx.clone();
        app.on_open_tree(move || {
            let _ = tx.send(UiCmd::OpenTree);
        });
        let tx = cmd_tx.clone();
        app.on_fork_from(move |entry_id| {
            let _ = tx.send(UiCmd::ForkFrom(entry_id.to_string()));
        });
        let tx = cmd_tx.clone();
        app.on_open_models(move || {
            let _ = tx.send(UiCmd::OpenModels);
        });
        let tx = cmd_tx.clone();
        app.on_serve_rapid_mlx(move |alias| {
            let _ = tx.send(UiCmd::ServeRapidMlxModel(alias.to_string()));
        });
        app.on_density_changed(density::save);
        let tx = cmd_tx.clone();
        app.on_attach_clicked(move || {
            // rfd's blocking picker opens a native modal; run it off both the
            // UI thread and the tokio runtime so neither blocks on it.
            let tx = tx.clone();
            std::thread::spawn(move || {
                if let Some(paths) = rfd::FileDialog::new().pick_files() {
                    for path in paths {
                        let _ = tx.send(UiCmd::AttachPath(path));
                    }
                }
            });
        });
        let tx = cmd_tx.clone();
        app.on_remove_attachment(move |index| {
            if index >= 0 {
                let _ = tx.send(UiCmd::RemoveAttachment(index as usize));
            }
        });
        let tx = cmd_tx.clone();
        app.on_open_palette(move || {
            let _ = tx.send(UiCmd::OpenPalette);
        });
        let tx = cmd_tx.clone();
        app.on_palette_query(move |query| {
            let _ = tx.send(UiCmd::PaletteQuery(query.to_string()));
        });
        let tx = cmd_tx.clone();
        let weak = app.as_weak();
        app.on_palette_exec(move |id| {
            let id = id.as_str();
            tracing::debug!(id, "palette: exec");
            if let Some(path) = id.strip_prefix("session:") {
                let _ = tx.send(UiCmd::SwitchSession(path.to_string()));
            } else if let Some(name) = id.strip_prefix("command:") {
                let _ = tx.send(UiCmd::Send(format!("/{name}")));
            } else if let Some(action) = id.strip_prefix("action:") {
                match action {
                    "new-session" => {
                        let _ = tx.send(UiCmd::NewSession);
                    }
                    "open-tree" => {
                        let _ = tx.send(UiCmd::OpenTree);
                    }
                    "open-models" => {
                        let _ = tx.send(UiCmd::OpenModels);
                    }
                    "clone-session" => {
                        let _ = tx.send(UiCmd::CloneSession);
                    }
                    "abort" => {
                        let _ = tx.send(UiCmd::Abort);
                    }
                    "cycle-density" => {
                        if let Some(app) = weak.upgrade() {
                            app.invoke_cycle_density();
                        }
                    }
                    "toggle-sidebar" => {
                        if let Some(app) = weak.upgrade() {
                            let visible = app.get_sidebar_visible();
                            app.set_sidebar_visible(!visible);
                        }
                    }
                    _ => {}
                }
            }
        });
    }
    app.set_density(density::load());

    let weak = app.as_weak();
    let dark_flag = dark.clone();
    if std::env::var("SLINTY_DEMO").is_ok() {
        rt.spawn(backend::demo_backend(weak, dark_flag, cmd_rx));
    } else {
        rt.spawn(backend::pi_backend(weak, dark_flag, cmd_rx));
    }

    // The sidebar sends these from real UI actions; each is also reachable
    // for testing/scripting via env var ("<delay_ms>:<arg>"), which is how
    // this backend is verified without relying on macOS accessibility
    // automation (Slint doesn't expose list rows as distinct AX elements,
    // so row clicks aren't drivable that way).
    spawn_delayed_cmd(&rt, &cmd_tx, "SLINTY_SWITCH_PROJECT_AFTER", |p| {
        UiCmd::SwitchProject(std::path::PathBuf::from(p))
    });
    spawn_delayed_cmd(
        &rt,
        &cmd_tx,
        "SLINTY_SWITCH_SESSION_AFTER",
        UiCmd::SwitchSession,
    );
    spawn_delayed_cmd(
        &rt,
        &cmd_tx,
        "SLINTY_SIDEBAR_SEARCH_AFTER",
        UiCmd::SidebarSearch,
    );
    spawn_delayed_cmd(
        &rt,
        &cmd_tx,
        "SLINTY_DELETE_SESSION_AFTER",
        UiCmd::DeleteSession,
    );
    spawn_delayed_cmd(
        &rt,
        &cmd_tx,
        "SLINTY_RENAME_SESSION_AFTER",
        UiCmd::RenameSession,
    );
    spawn_delayed_cmd(&rt, &cmd_tx, "SLINTY_NEW_SESSION_AFTER", |_| {
        UiCmd::NewSession
    });
    spawn_delayed_cmd(&rt, &cmd_tx, "SLINTY_OPEN_TREE_AFTER", |_| UiCmd::OpenTree);
    spawn_delayed_cmd(&rt, &cmd_tx, "SLINTY_OPEN_MODELS_AFTER", |_| {
        UiCmd::OpenModels
    });
    spawn_delayed_cmd(
        &rt,
        &cmd_tx,
        "SLINTY_SERVE_RAPID_MLX_AFTER",
        UiCmd::ServeRapidMlxModel,
    );
    spawn_delayed_cmd(&rt, &cmd_tx, "SLINTY_FORK_FROM_AFTER", UiCmd::ForkFrom);
    // Same as SLINTY_DEMO_AUTOSEND but for the real (non-demo) backend.
    spawn_delayed_cmd(&rt, &cmd_tx, "SLINTY_SEND_AFTER", UiCmd::Send);
    spawn_delayed_cmd(&rt, &cmd_tx, "SLINTY_OPEN_PALETTE_AFTER", |_| {
        UiCmd::OpenPalette
    });
    spawn_delayed_cmd(
        &rt,
        &cmd_tx,
        "SLINTY_PALETTE_QUERY_AFTER",
        UiCmd::PaletteQuery,
    );
    // Bypasses the native file dialog, which (like screenshots and
    // keystrokes) has no display to run against in a headless/test launch.
    spawn_delayed_cmd(&rt, &cmd_tx, "SLINTY_ATTACH_AFTER", |p| {
        UiCmd::AttachPath(std::path::PathBuf::from(p))
    });
    // Density is UI-only (no backend command), so it's driven directly via
    // `invoke_cycle_density` rather than through `cmd_tx`.
    spawn_delayed_cycle_density(&rt, app.as_weak());
    // Palette exec dispatch (session/command/action routing) is also
    // UI-only until it reaches `cmd_tx` inside `on_palette_exec`, so it's
    // driven the same way, via the real `palette-exec` callback.
    spawn_delayed_invoke(
        &rt,
        app.as_weak(),
        "SLINTY_PALETTE_EXEC_AFTER",
        |app, id| {
            app.invoke_palette_exec(id.as_str().into());
        },
    );

    // Keep the highlighter's theme choice and the code-card background in
    // sync with the OS color scheme. Code cards use the syntect theme's own
    // background color so span colors keep their designed contrast.
    let apply_code_theme = |app: &AppWindow| {
        let (r, g, b) = highlight::theme_background(app.get_dark_mode());
        app.set_code_background(slint::Color::from_rgb_u8(r, g, b));
    };
    apply_code_theme(&app);
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
                    apply_code_theme(&app);
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

/// Parse `env_var` as `"<delay_ms>:<arg>"` and, after that delay, send
/// `build(arg)` on `cmd_tx`. No-op if the var is unset or malformed. `arg` is
/// ignored (but still required, even if empty) for commands that take none.
fn spawn_delayed_cmd(
    rt: &tokio::runtime::Runtime,
    cmd_tx: &mpsc::UnboundedSender<UiCmd>,
    env_var: &str,
    build: impl FnOnce(String) -> UiCmd + Send + 'static,
) {
    let Ok(spec) = std::env::var(env_var) else {
        return;
    };
    let Some((delay_ms, arg)) = spec.split_once(':') else {
        return;
    };
    let Ok(delay_ms) = delay_ms.parse::<u64>() else {
        return;
    };
    let arg = arg.to_string();
    let tx = cmd_tx.clone();
    rt.spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
        let _ = tx.send(build(arg));
    });
}

/// Parse `SLINTY_CYCLE_DENSITY_AFTER` as `"<delay_ms>:<times>"` and, after
/// each delay, invoke the Slint-side `cycle-density` function `times` times.
/// No-op if unset or malformed.
fn spawn_delayed_cycle_density(rt: &tokio::runtime::Runtime, weak: slint::Weak<AppWindow>) {
    let Ok(spec) = std::env::var("SLINTY_CYCLE_DENSITY_AFTER") else {
        return;
    };
    let Some((delay_ms, times)) = spec.split_once(':') else {
        return;
    };
    let Ok(delay_ms) = delay_ms.parse::<u64>() else {
        return;
    };
    let Ok(times) = times.parse::<u32>() else {
        return;
    };
    rt.spawn(async move {
        for _ in 0..times {
            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            let _ = weak.upgrade_in_event_loop(|app| {
                app.invoke_cycle_density();
            });
        }
    });
}

/// Parse `env_var` as `"<delay_ms>:<arg>"` and, after that delay, run
/// `invoke(app, arg)` on the UI thread. No-op if unset or malformed. Used
/// for test hooks that need to fire a real Slint callback (as opposed to
/// `spawn_delayed_cmd`, which sends straight to the backend).
fn spawn_delayed_invoke(
    rt: &tokio::runtime::Runtime,
    weak: slint::Weak<AppWindow>,
    env_var: &str,
    invoke: impl FnOnce(&AppWindow, String) + Send + 'static,
) {
    let Ok(spec) = std::env::var(env_var) else {
        return;
    };
    let Some((delay_ms, arg)) = spec.split_once(':') else {
        return;
    };
    let Ok(delay_ms) = delay_ms.parse::<u64>() else {
        return;
    };
    let arg = arg.to_string();
    rt.spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
        let _ = weak.upgrade_in_event_loop(move |app| invoke(&app, arg));
    });
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
