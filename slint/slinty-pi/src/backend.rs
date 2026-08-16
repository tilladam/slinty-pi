//! Slint-side `UiSink` implementation: turns `pi_core::backend::RowSpec`
//! values into this app's generated `Row`/`AppWindow` types and pushes them
//! through `Weak::upgrade_in_event_loop`. Everything UI-toolkit-agnostic
//! (the state machine that drives pi, row projection, session/model
//! orchestration) lives in `pi_core::backend` — this file is only the
//! Slint-specific render/dispatch half.
//!
//! Threading contract: this struct is handed to `pi_core::backend::pi_backend`/
//! `demo_backend`, which call its methods from tokio worker threads; every
//! UI touch goes through `Weak::upgrade_in_event_loop`, whose closures run on
//! the Slint thread in submission order.

use slint::{Color, Model, ModelRc, SharedString, StyledText, VecModel, Weak};

use pi_core::backend::{RapidMlxModelState, RapidMlxPanelData, RouterPanelData, RowSpec, UiSink};
use pi_core::palette;

use crate::{
    AppWindow, CachedModelRow, CodeLine, ColoredSpan, HfResultRow, PaletteRow, QueueItem,
    RouterModelRow, Row, SessionRow, TableCell, TableRowCells, TreeRow,
};

pub struct SlintUi {
    weak: Weak<AppWindow>,
}

impl SlintUi {
    pub fn new(weak: Weak<AppWindow>) -> Self {
        Self { weak }
    }

    fn with_app(&self, f: impl FnOnce(&AppWindow) + Send + 'static) {
        let _ = self.weak.upgrade_in_event_loop(move |app| f(&app));
    }

    fn with_transcript(&self, f: impl FnOnce(&AppWindow, &VecModel<Row>) + Send + 'static) {
        self.with_app(move |app| {
            let model = app.get_transcript();
            let model = model
                .as_any()
                .downcast_ref::<VecModel<Row>>()
                .expect("transcript is a VecModel");
            f(app, model);
        });
    }
}

impl UiSink for SlintUi {
    fn push(&self, spec: RowSpec) {
        self.with_transcript(move |app, model| {
            model.push(row_to_slint(&spec));
            app.invoke_scroll_to_end();
        });
    }

    /// Replace a row, preserving its user-toggled expansion state.
    fn set(&self, index: usize, spec: RowSpec) {
        self.with_transcript(move |app, model| {
            if index < model.row_count() {
                let mut row = row_to_slint(&spec);
                if let Some(old) = model.row_data(index) {
                    row.expanded = old.expanded;
                    row.expanded_overridden = old.expanded_overridden;
                }
                model.set_row_data(index, row);
                app.invoke_scroll_to_end();
            }
        });
    }

    /// Append many rows in a handful of event-loop hops instead of one per
    /// row, so hydrating a large session stays responsive.
    fn push_all(&self, specs: Vec<RowSpec>) {
        const BATCH: usize = 100;
        let mut iter = specs.into_iter().peekable();
        while iter.peek().is_some() {
            let chunk: Vec<RowSpec> = iter.by_ref().take(BATCH).collect();
            self.with_transcript(move |app, model| {
                for spec in &chunk {
                    model.push(row_to_slint(spec));
                }
                app.invoke_scroll_to_end();
            });
        }
    }

    fn clear(&self) {
        self.with_transcript(|_, model| {
            while model.row_count() > 0 {
                model.remove(model.row_count() - 1);
            }
        });
    }

    fn truncate(&self, len: usize) {
        self.with_transcript(move |_, model| {
            while model.row_count() > len {
                model.remove(model.row_count() - 1);
            }
        });
    }

    fn set_streaming(&self, streaming: bool) {
        self.with_app(move |app| app.set_streaming(streaming));
    }

    fn set_status(&self, status: String) {
        self.with_app(move |app| app.set_status_text(SharedString::from(status)));
    }

    fn set_context_percent(&self, percent: f32) {
        self.with_app(move |app| app.set_context_percent(percent));
    }

    fn set_queue(&self, items: Vec<(&'static str, String)>) {
        self.with_app(move |app| {
            let rows: Vec<QueueItem> = items
                .into_iter()
                .map(|(kind, text)| QueueItem {
                    kind: kind.into(),
                    text: text.as_str().into(),
                })
                .collect();
            app.set_queue(ModelRc::new(VecModel::from(rows)));
        });
    }

    fn set_models(&self, labels: Vec<String>, index: i32) {
        self.with_app(move |app| {
            let labels: Vec<SharedString> = labels.iter().map(|l| l.as_str().into()).collect();
            app.set_model_list(ModelRc::new(VecModel::from(labels)));
            app.set_model_index(index);
        });
    }

    fn set_server_dot(&self, state: i32) {
        self.with_app(move |app| app.set_server_dot(state));
    }

    fn set_thinking(&self, labels: Vec<String>, index: i32) {
        self.with_app(move |app| {
            let labels: Vec<SharedString> = labels.iter().map(|l| l.as_str().into()).collect();
            app.set_thinking_list(ModelRc::new(VecModel::from(labels)));
            app.set_thinking_index(index);
        });
    }

    /// `labels`/`paths` are parallel arrays (projects other than the current
    /// one). A placeholder ("Switch project…") is prepended to what Slint
    /// sees as index 0 — *not* to `paths`, which stays real-projects-only —
    /// so `project-index: 0` is a genuine, truthfully-displayed selection.
    /// Slint's `ComboBoxBase` has no concept of "no selection": its
    /// `reset-current()` clamps whatever `current-index` we send into
    /// `[0, model.length-1]` on every model change, so a bare `-1` "meaning
    /// unselected" silently becomes index 0 and displays `model[0]` as if
    /// it were genuinely chosen — which, before this placeholder existed,
    /// meant the box showed some *other* project as though it were current
    /// (see the sidebar bug report this fixed: clicking that misleadingly-
    /// shown entry silently switched the whole app into it).
    fn set_projects(&self, labels: Vec<String>, paths: Vec<String>, current_name: String) {
        self.with_app(move |app| {
            let mut label_model: Vec<SharedString> = vec!["Switch project…".into()];
            label_model.extend(labels.iter().map(|l| l.as_str().into()));
            let path_model: Vec<SharedString> = paths.iter().map(|p| p.as_str().into()).collect();
            app.set_project_list(ModelRc::new(VecModel::from(label_model)));
            app.set_project_paths(ModelRc::new(VecModel::from(path_model)));
            app.set_project_index(0);
            app.set_current_project_name(SharedString::from(current_name));
        });
    }

    /// `rows` are `(path, title, relative_time, active, cost)`.
    fn set_sidebar_sessions(&self, rows: Vec<(String, String, String, bool, String)>) {
        self.with_app(move |app| {
            let rows: Vec<SessionRow> = rows
                .into_iter()
                .map(|(path, title, relative_time, active, cost)| SessionRow {
                    path: path.as_str().into(),
                    title: title.as_str().into(),
                    relative_time: relative_time.as_str().into(),
                    active,
                    cost: cost.as_str().into(),
                })
                .collect();
            app.set_sidebar_sessions(ModelRc::new(VecModel::from(rows)));
        });
    }

    /// `rows` are `(id, depth, summary, label, can_fork, active)`. Opens the
    /// overlay once the (freshly-fetched) rows land.
    fn set_tree(&self, rows: Vec<(String, i32, String, String, bool, bool)>) {
        self.with_app(move |app| {
            let rows: Vec<TreeRow> = rows
                .into_iter()
                .map(|(id, depth, summary, label, can_fork, active)| TreeRow {
                    id: id.as_str().into(),
                    depth,
                    summary: summary.as_str().into(),
                    label: label.as_str().into(),
                    can_fork,
                    active,
                })
                .collect();
            app.set_tree_rows(ModelRc::new(VecModel::from(rows)));
            app.set_tree_visible(true);
        });
    }

    /// Rapid-mlx section only. Deliberately separate from
    /// [`Self::set_router_panel`]: collecting rapid-mlx state spawns the CLI
    /// a handful of times, which the router's load/unload poll loop must not
    /// repeat on every tick.
    fn set_rapid_mlx_panel(&self, data: RapidMlxPanelData) {
        self.with_app(move |app| {
            // Only a server we spawned can be stopped from the UI, and the
            // `managed` flag is panel-level — fold it into the served row so
            // `CachedModelItem` stays self-contained.
            let managed = data.running.as_ref().is_some_and(|r| r.managed);
            let rows: Vec<CachedModelRow> = data
                .cached
                .into_iter()
                .map(|row| {
                    let served = row.state == RapidMlxModelState::KnownServed;
                    CachedModelRow {
                        alias: row.alias.as_str().into(),
                        hf_repo: row.hf_repo.as_str().into(),
                        size: row.size.as_str().into(),
                        fit_label: row.fit_label.as_str().into(),
                        state: match row.state {
                            RapidMlxModelState::KnownServed => "served",
                            RapidMlxModelState::KnownIdle => "idle",
                            RapidMlxModelState::Unknown => "unknown",
                        }
                        .into(),
                        can_stop: served && managed,
                    }
                })
                .collect();
            app.set_rapid_mlx_version(SharedString::from(data.version.unwrap_or_default()));
            app.set_rapid_mlx_running_known(data.running.as_ref().is_none_or(|r| r.known_to_pi));
            app.set_rapid_mlx_running(SharedString::from(
                data.running.map(|r| r.summary).unwrap_or_default(),
            ));
            app.set_rapid_mlx_cached(ModelRc::new(VecModel::from(rows)));
            app.set_rapid_mlx_catalog_count(data.catalog_count as i32);
        });
    }

    /// llama.cpp router section only — see [`Self::set_rapid_mlx_panel`]'s
    /// doc comment for why the two are separate setters.
    fn set_router_panel(&self, data: RouterPanelData) {
        self.with_app(move |app| {
            let rows: Vec<RouterModelRow> = data
                .models
                .into_iter()
                .map(|(id, status, loaded, busy)| RouterModelRow {
                    id: id.as_str().into(),
                    status: status.as_str().into(),
                    loaded,
                    busy,
                })
                .collect();
            app.set_router_status(SharedString::from(data.status_label));
            app.set_router_url(SharedString::from(data.base_url));
            app.set_router_models(ModelRc::new(VecModel::from(rows)));
        });
    }

    fn show_models_panel(&self) {
        self.with_app(move |app| {
            app.set_models_visible(true);
        });
    }

    /// `results` rows are `(id, gated, downloads, quants)`.
    fn set_hf_search_results(&self, results: Vec<(String, bool, i32, Vec<String>)>) {
        self.with_app(move |app| {
            let rows: Vec<HfResultRow> = results
                .into_iter()
                .map(|(id, gated, downloads, quants)| HfResultRow {
                    id: id.as_str().into(),
                    gated,
                    downloads,
                    quants: ModelRc::new(VecModel::from(
                        quants
                            .into_iter()
                            .map(|q| SharedString::from(q.as_str()))
                            .collect::<Vec<_>>(),
                    )),
                })
                .collect();
            app.set_hf_search_results(ModelRc::new(VecModel::from(rows)));
        });
    }

    /// `summary` is a pre-formatted "N model(s): a, b, c" string (or empty
    /// when not detected) — built in Rust rather than exposing a raw row
    /// list, since this section is a one-click "add all" affordance, not a
    /// browsable list like the router/rapid-mlx sections.
    fn set_auth_entries(&self, labels: Vec<String>) {
        self.with_app(move |app| {
            let labels: Vec<SharedString> = labels.iter().map(|l| l.as_str().into()).collect();
            app.set_auth_entries(ModelRc::new(VecModel::from(labels)));
        });
    }

    fn set_ollama_panel(&self, detected: bool, summary: String, model_count: i32) {
        self.with_app(move |app| {
            app.set_ollama_detected(detected);
            app.set_ollama_summary(SharedString::from(summary));
            app.set_ollama_model_count(model_count);
        });
    }

    fn set_palette_entries(&self, entries: Vec<palette::PaletteEntry>) {
        self.with_app(move |app| {
            let rows: Vec<PaletteRow> = entries
                .into_iter()
                .map(|e| PaletteRow {
                    id: e.id.as_str().into(),
                    kind: e.kind.into(),
                    label: e.label.as_str().into(),
                    detail: e.detail.as_str().into(),
                })
                .collect();
            app.set_palette_entries(ModelRc::new(VecModel::from(rows)));
        });
    }

    /// Prefill the composer (used after a fork, which hands back the text of
    /// the message it forked from instead of keeping it in context).
    fn set_composer_text(&self, text: String) {
        self.with_app(move |app| {
            app.invoke_set_composer_text(text.as_str().into());
        });
    }

    /// Append `@path` to the composer (non-image attachment); Slint owns
    /// the spacing, since it has the current text and this doesn't.
    fn append_composer_text(&self, path: &std::path::Path) {
        let text = format!("@{}", path.display());
        self.with_app(move |app| {
            app.invoke_append_to_composer(text.as_str().into());
        });
    }

    /// Chip labels (file names) for queued image attachments.
    fn set_pending_attachments(&self, names: Vec<String>) {
        self.with_app(move |app| {
            let rows: Vec<SharedString> = names.into_iter().map(|n| n.as_str().into()).collect();
            app.set_pending_attachments(ModelRc::new(VecModel::from(rows)));
        });
    }
}

/// Convert a toolkit-agnostic `RowSpec` into this app's generated `Row`.
fn row_to_slint(spec: &RowSpec) -> Row {
    let styled = match &spec.markdown {
        Some(md) => {
            StyledText::from_markdown(md).unwrap_or_else(|_| StyledText::from_plain_text(md))
        }
        None => StyledText::default(),
    };
    let code_lines = code_lines_model(&spec.code_lines);
    let (table_rows, table_pref_width) = table_rows_model(&spec.table_rows);
    Row {
        kind: spec.kind.into(),
        styled,
        text: spec.text.as_str().into(),
        lang: spec.lang.as_str().into(),
        level: spec.level,
        expanded: false,
        expanded_overridden: false,
        detail: spec.detail.as_str().into(),
        running: spec.running,
        elapsed: spec.elapsed.as_str().into(),
        first: spec.first,
        raw: spec.raw.as_str().into(),
        code_lines,
        table_rows,
        table_pref_width,
    }
}

/// Convert highlighted lines into the Slint model shape. Shared with the
/// scheme-change re-highlight in `main.rs`, which rebuilds `code-lines` for
/// rows already on screen.
pub fn code_lines_model(lines: &[pi_core::highlight::CodeLine]) -> ModelRc<CodeLine> {
    let rows: Vec<CodeLine> = lines
        .iter()
        .map(|line| {
            let spans: Vec<ColoredSpan> = line
                .spans
                .iter()
                .map(|s| ColoredSpan {
                    text: s.text.as_str().into(),
                    color: Color::from_rgb_u8(s.color.0, s.color.1, s.color.2),
                })
                .collect();
            CodeLine {
                spans: ModelRc::new(VecModel::from(spans)),
            }
        })
        .collect();
    ModelRc::new(VecModel::from(rows))
}

/// Convert row-major table cells into the Slint model, attaching each cell
/// its column's width share (from the column's longest cell, clamped so one
/// verbose column can't starve the others entirely), normalized so a row's
/// weights sum to 1.0. The UI multiplies the block width by the share to get
/// explicit, identical column boundaries in every row — explicit rather than
/// stretch-negotiated, because stretch weights resolve before wrapped cell
/// text knows its height at the final width, which made rows too short.
/// The second return value is the table's estimated natural width in
/// logical px (the UI caps it at the available span), so narrow tables
/// don't stretch across the whole transcript.
fn table_rows_model(rows: &[Vec<pi_core::segmenter::TableCell>]) -> (ModelRc<TableRowCells>, f32) {
    let col_count = rows.first().map(Vec::len).unwrap_or(0);
    let weights: Vec<f32> = (0..col_count)
        .map(|i| {
            let max_chars = rows
                .iter()
                .filter_map(|row| row.get(i))
                .map(|c| c.text.chars().count())
                .max()
                .unwrap_or(1);
            max_chars.clamp(3, 60) as f32
        })
        .collect();
    // ~7px per character at the 12.5px table font, plus cell padding.
    let pref_width: f32 = weights.iter().map(|w| w * 7.0 + 18.0).sum();
    let total: f32 = weights.iter().sum::<f32>().max(1.0);
    let weights: Vec<f32> = weights.iter().map(|w| w / total).collect();
    let rows: Vec<TableRowCells> = rows
        .iter()
        .map(|row| {
            let cells: Vec<TableCell> = row
                .iter()
                .enumerate()
                .map(|(i, c)| TableCell {
                    text: c.text.as_str().into(),
                    header: c.header,
                    weight: weights.get(i).copied().unwrap_or(1.0),
                })
                .collect();
            TableRowCells {
                cells: ModelRc::new(VecModel::from(cells)),
            }
        })
        .collect();
    (ModelRc::new(VecModel::from(rows)), pref_width)
}
