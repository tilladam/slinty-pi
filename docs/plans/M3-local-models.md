# M3 — Local models, delightfully

**Goal:** the differentiator — a fresh machine with only llama.cpp (or Ollama) installed reaches
a working agent chat without touching a terminal config file. First token in under a minute.

## Verified facts this plan builds on

- pi's `/llama` TUI flow is **not** reachable over RPC; but its HTTP client (read from pi-mono
  `packages/coding-agent/src/extensions/llama/client.ts`) talks straight to the llama.cpp
  **router** (`llama-server` without `--model`):
  - `GET /health` — reachability
  - `GET /models` (`?reload=1` to rescan the models dir) — all models + loaded state
  - `POST /models/load` `{model}` / `POST /models/unload` `{model}`
  - `POST /models` `{model}` — download a model (e.g. `owner/repo:quant` from Hugging Face;
    the *server* performs the download)
  - `GET /models/sse` — server-sent events for load/download progress
- Only **loaded** router models appear in pi's `get_available_models`; selection then goes
  through the normal `set_model` RPC (provider `llama.cpp`). Router URL comes from `/login
  llama.cpp` or `LLAMA_BASE_URL` (default `http://127.0.0.1:8080`).
- Ollama / LM Studio / vLLM / SGLang are plain OpenAI-compatible providers declared in
  `~/.pi/agent/models.json` (hot-reloaded by pi when the model list opens; keyless servers need a
  dummy `apiKey`; `compat` flags like `supportsDeveloperRole`, `thinkingFormat` handle server
  quirks). Ollama's native API (`GET http://localhost:11434/api/tags`) lists installed models.
- Provider API keys live in `~/.pi/agent/auth.json` (0600, supports `$ENV` and `!command`
  interpolation — the editor must preserve those forms verbatim).

## Design

### New module: `backend/local.rs` (+ `reqwest` with rustls)

- `LlamaRouter` client for the endpoints above, including an SSE reader mapping progress events
  into UI state. All state refreshes go through `GET /models` (the router is shared — never
  assume our view is the only writer; mirror pi's behavior of always re-reading server state).
- `OllamaProbe`: `GET /api/tags` to detect a running Ollama and list models.
- `SystemFit`: total/available RAM + VRAM detection (`sysinfo`; on Apple Silicon unified memory,
  treat RAM as the budget) → fit labels for a model size: **Fits / May be slow / Won't fit**
  (thresholds: size ≤ 0.7×free → Fits; ≤ 1.0× → May be slow; else Won't fit; label the
  heuristic as an estimate in the UI).

### Models panel (new `ui/models.slint`, opened from status bar / palette)

Three sections, progressive disclosure:

1. **llama.cpp router**: connection status + URL field; model cards (name, size, quant, context,
   fit label, loaded badge); one-click Load/Unload with progress from SSE; "Download model…"
   with Hugging Face search (`GET https://huggingface.co/api/models?search=…&filter=gguf`,
   `HF_TOKEN` honored when present), quant picker, download progress, gated-repo warning
   linking to the HF page (mirrors pi's own warning behavior).
2. **Ollama / OpenAI-compatible endpoints**: detected servers with one-click "Add to pi" — a
   guided form writing `models.json` (preset templates for Ollama/LM Studio/vLLM; compat flags
   as plain-language toggles, e.g. "server rejects `developer` role"). Show existing entries;
   edits round-trip through serde while preserving unknown fields (`#[serde(flatten)]` maps).
3. **Cloud providers**: API-key entry writing `auth.json` (never log or echo keys; preserve
   `$ENV`/`!cmd` entries read-only). OAuth providers list with "run `pi /login <provider>` in a
   terminal" instruction — out of scope to reimplement in v1.

After any change that affects the catalog: nudge pi (`get_available_models` re-read picks up
loaded router models; models.json changes require re-opening the picker — verify at impl time
whether a `new_session`-free refresh needs anything else) and refresh the composer's model
picker. Model rows selected here call `set_model` directly.

### Onboarding flow (first launch, and empty-model states)

State machine shown in the transcript's empty state, not a wizard window:

1. pi missing → install instruction with copy button (brew + npm variants), re-check button.
2. pi present, no model configured → probe router + Ollama in parallel:
   - Router up with models → "Load & chat" one-click (load → set_model → focus composer).
   - Router up, no models → starter-model suggestion (small default, e.g. a ~4 GB Qwen coder
     quant chosen by fit label) → download with progress → load → set_model.
   - Only Ollama up → one-click add to models.json → set_model.
   - Neither → cards for "start llama-server" (copy-paste command from the pi docs) / cloud key.
3. Every step reports elapsed state honestly; the target is first token < 60 s on the happy path.

### Composer touches

- Model picker entries get a local/cloud badge and "free · local" instead of a price.
- Status bar shows the router state dot when the active model is a router model; clicking opens
  the models panel.

## Work breakdown

1. `LlamaRouter` client + SSE progress + unit tests against recorded fixtures; integration test
   against a real `llama-server` when present (skip otherwise, like the pi-rpc tests).
2. `SystemFit` + fit labels + tests (fixed fixtures, no hardware assumptions).
3. Models panel UI: router section (list/load/unload/progress).
4. HF search + download flow (+ gated-repo warning).
5. `models.json` guided editor with round-trip-preserving serde + Ollama detection.
6. `auth.json` key entry (secure field, 0600 preserved, interpolation entries untouched).
7. Onboarding state machine + empty-state UI + probes.
8. Composer badges, status-bar router dot, palette entries ("Load model…", "Models panel").
9. Demo mode: fake router state so the panel is demoable offline.

## Risks

- **Router API drift** (llama.cpp moves fast): pin known-good behavior behind one client module;
  degrade to "open pi TUI /llama" hint on unknown responses.
- **Fit labels overpromise**: quantization, context size, and KV cache all shift real memory
  needs — label as estimate, never block an action on it.
- **models.json corruption**: always write atomically (tmp + rename) and keep one `.bak`;
  malformed existing file → read-only warning, never rewrite what we can't parse.
- **HF search rate limits** without token: debounce, cache, and surface the 429 politely.

## Acceptance criteria

- Fresh machine + running empty router: download starter model, load, chat — no terminal, < 60 s
  after download completes; progress visible throughout.
- Running Ollama with models: added to pi and chatting in ≤ 3 clicks.
- `models.json` edits made by the app are accepted by pi TUI unchanged (round-trip test), and
  hand-written entries survive app edits byte-for-byte outside the edited entry.
- Router shared with a concurrent pi TUI stays consistent (both see loads/unloads).
