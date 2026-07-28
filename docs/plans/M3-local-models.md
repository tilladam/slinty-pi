# M3 — Local models, delightfully

**Goal:** the differentiator — a fresh machine with only rapid-mlx, llama.cpp, or Ollama
installed reaches a working agent chat without touching a terminal config file. First token in
under a minute. First-class targets: **rapid-mlx** (recommended default on Apple Silicon),
**llama.cpp router**, **Ollama**; LM Studio/vLLM/SGLang via the generic OpenAI-compatible path.

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
- **rapid-mlx** (Apache-2.0, Apple Silicon M1+/macOS 14+, MLX-based; researched 2026-07 from
  rapidmlx.com/docs and the GitHub README):
  - OpenAI-compatible server at `http://localhost:8000/v1` (`/v1/chat/completions`,
    `/v1/responses`, `/v1/embeddings`, `/v1/audio/*`) **plus** Anthropic-style `/v1/messages`;
    no API key required. Strong tool calling (17 parsers) — a good match for pi's tool loop —
    with prompt caching, continuous batching, and speculative decoding (~4× Ollama throughput,
    per its own benchmarks).
  - **Management is CLI-first, not HTTP**: `rapid-mlx pull|rm|models|info <alias>|ps|serve
    <model>|doctor`; models are named by alias (e.g. `qwen3.5-4b-4bit`), weights download on
    first use with progress output; **one model per `serve` process, no hot-swap or documented
    admin/health endpoints** (whether `/v1/models` exists must be verified at impl time; assume
    process-level probing otherwise). Install: `brew install rapid-mlx`; state in `~/.rapid-mlx/`.
  - Model catalog with RAM guidance lives at models.rapidmlx.com; `rapid-mlx info` exposes
    per-alias profiles for local fit-labeling.

## Design

### New module: `backend/local.rs` (+ `reqwest` with rustls)

- `LlamaRouter` client for the endpoints above, including an SSE reader mapping progress events
  into UI state. All state refreshes go through `GET /models` (the router is shared — never
  assume our view is the only writer; mirror pi's behavior of always re-reading server state).
- `RapidMlx` integration — CLI-driven, and unlike the router the app can **own the server
  lifecycle** (one model per `serve` process makes it a natural managed child):
  - Detection: `rapid-mlx` binary on PATH (`--version`), running server via port-8000 probe
    (`GET /v1/models` if it exists, else a cheap `/v1/chat/completions` OPTIONS/error probe —
    settle at impl time) and `rapid-mlx ps`.
  - Catalog: `rapid-mlx models` / `rapid-mlx info <alias>` parsed for aliases + RAM profiles
    (feeds the same fit labels as GGUF sizes elsewhere).
  - Model switch = supervised process restart: spawn `rapid-mlx serve <alias>` (tokio child,
    kill_on_drop, stdout/stderr parsed for ready + download-progress lines), wait ready, then
    `set_model` in pi. If the user runs their own server externally, detect and *don't* manage.
  - `rapid-mlx pull <alias>` for explicit downloads with parsed progress.
  - pi wiring: **resolved 2026-07-26** by running both wire APIs end-to-end through a real
    `pi --mode json --print` tool-call turn (bash tool, `--no-session`) against this machine's
    live rapid-mlx server — **use `api: "anthropic-messages"`, with `baseUrl` as the bare
    origin (`http://localhost:8000`, no `/v1` suffix)** as the preset default:
    - `anthropic-messages` with `baseUrl: "http://localhost:8000/v1"` (the same convention as
      the `openai-completions` preset) **404s** — pi's anthropic client appends `/v1/messages`
      itself, so the `/v1` must NOT be in `baseUrl` for this API type. This is the one deviation
      from `models.md`'s own custom-header example (which shows a proxy `baseUrl` ending in
      `/v1` for `anthropic-messages`); trust the empirical result over that doc snippet.
    - With the bare-origin `baseUrl`, `anthropic-messages` round-tripped a full tool-call turn
      correctly (`toolCall` → bash execution → `toolResult` → follow-up turn) **and** surfaced
      genuine `thinking` content blocks automatically — no per-model `compat`/`thinkingFormat`
      needed, because rapid-mlx's own `reasoning_parser` abstraction (visible per-alias via
      `RapidMlx::catalog()`/`info()`) already normalizes whatever the underlying model family
      emits into that one endpoint. `thinkingSignature` came back empty; the anthropic-messages
      `compat.allowEmptySignature: true` flag should be set proactively in the preset in case a
      longer multi-turn conversation is stricter about replaying it than this single-turn test.
    - `openai-completions` (`baseUrl: ".../v1"`, current hand-written `~/.pi/agent/models.json`
      convention) round-tripped the tool-call turn correctly too, but surfaced **no** thinking
      content by default. A `chat_template_kwargs: {"enable_thinking": true}` request (verified
      via raw curl) does unlock a separate `reasoning_content` field — matching pi's
      `compat.thinkingFormat: "qwen-chat-template"` exactly — but that's a per-model-family
      compat mapping (`qwen-chat-template` only fits Qwen; rapid-mlx's 165-alias catalog spans
      deepseek_r1/gemma4/hy_v3/ui_tars/... families with different conventions), which cuts
      against a single preset that "just works" across the whole catalog.
    - Net: ship `anthropic-messages` + bare-origin `baseUrl` as the models.json preset default;
      `openai-completions` remains a documented fallback (e.g. for a future non-rapid-mlx
      OpenAI-compatible router that doesn't implement `/v1/messages` at all).
- `OllamaProbe`: `GET /api/tags` to detect a running Ollama and list models.
- `SystemFit`: total/available RAM + VRAM detection (`sysinfo`; on Apple Silicon unified memory,
  treat RAM as the budget) → fit labels for a model size: **Fits / May be slow / Won't fit**
  (thresholds: size ≤ 0.7×free → Fits; ≤ 1.0× → May be slow; else Won't fit; label the
  heuristic as an estimate in the UI).

### Models panel (new `ui/models.slint`, opened from status bar / palette)

Sections in recommendation order (rapid-mlx first on Apple Silicon), progressive disclosure:

1. **rapid-mlx** (shown on Apple Silicon): install state (offer `brew install rapid-mlx` with
   copy button when missing), server state (managed / external / stopped), active model card,
   alias catalog from the CLI with fit labels and one-click Pull + Serve; switching models
   restarts the managed server with visible progress ("stopping · loading qwen3.5-4b-4bit ·
   ready"). A "manage yourself" escape hatch stops supervision without stopping the server.
2. **llama.cpp router**: connection status + URL field; model cards (name, size, quant, context,
   fit label, loaded badge); one-click Load/Unload with progress from SSE; "Download model…"
   with Hugging Face search (`GET https://huggingface.co/api/models?search=…&filter=gguf`,
   `HF_TOKEN` honored when present), quant picker, download progress, gated-repo warning
   linking to the HF page (mirrors pi's own warning behavior).
3. **Ollama / OpenAI-compatible endpoints**: detected servers with one-click "Add to pi" — a
   guided form writing `models.json` (preset templates for Ollama/LM Studio/vLLM; compat flags
   as plain-language toggles, e.g. "server rejects `developer` role"). Show existing entries;
   edits round-trip through serde while preserving unknown fields (`#[serde(flatten)]` maps).
4. **Cloud providers**: API-key entry writing `auth.json` (never log or echo keys; preserve
   `$ENV`/`!cmd` entries read-only). OAuth providers list with "run `pi /login <provider>` in a
   terminal" instruction — out of scope to reimplement in v1.

After any change that affects the catalog: nudge pi (`get_available_models` re-read picks up
loaded router models; models.json changes require re-opening the picker — verify at impl time
whether a `new_session`-free refresh needs anything else) and refresh the composer's model
picker. Model rows selected here call `set_model` directly.

### Onboarding flow (first launch, and empty-model states)

State machine shown in the transcript's empty state, not a wizard window:

1. pi missing → install instruction with copy button (brew + npm variants), re-check button.
2. pi present, no model configured → probe rapid-mlx (binary + port 8000), llama.cpp router,
   and Ollama in parallel:
   - rapid-mlx serving → add preset to models.json → set_model → chat (fastest path).
   - rapid-mlx installed but idle → starter-alias suggestion by fit label → Pull + Serve with
     progress → set_model.
   - Router up with models → "Load & chat" one-click (load → set_model → focus composer).
   - Router up, no models → starter-model suggestion (small default, e.g. a ~4 GB Qwen coder
     quant chosen by fit label) → download with progress → load → set_model.
   - Only Ollama up → one-click add to models.json → set_model.
   - Nothing running → recommendation cards, ordered by platform: on Apple Silicon rapid-mlx
     first (`brew install rapid-mlx`), then llama-server (copy-paste command from the pi docs),
     then cloud key.
3. Every step reports elapsed state honestly; the target is first token < 60 s on the happy path.

### Composer touches

- Model picker entries get a local/cloud badge and "free · local" instead of a price.
- Status bar shows a server state dot when the active model is a managed local server
  (rapid-mlx or router); clicking opens the models panel. For a managed rapid-mlx child,
  server death shows as needs-attention, with one-click restart.

## Work breakdown

1. `LlamaRouter` client + SSE progress + unit tests against recorded fixtures; integration test
   against a real `llama-server` when present (skip otherwise, like the pi-rpc tests).
2. `RapidMlx` module: detection, CLI catalog parsing (`models`/`info`), managed `serve` child
   with ready/progress parsing, `pull` progress, `models.json` preset (decide
   openai-completions vs anthropic-messages by testing both against pi); integration test
   against a real `rapid-mlx` when present (skip otherwise).
3. `SystemFit` + fit labels + tests (fixed fixtures, no hardware assumptions).
4. Models panel UI: rapid-mlx section (install/serve/switch/pull) **[done]** + router section
   (list/load/unload/progress) **[done, 2026-07-28]** — progress is polled from `GET /models`
   (whose `status.progress` field already carries it) rather than the `/models/sse` stream;
   `LlamaRouter::subscribe_events`/`SseReader` stay unused for now (see their doc comments).
5. HF search + download flow (+ gated-repo warning).
6. `models.json` guided editor with round-trip-preserving serde + Ollama detection.
7. `auth.json` key entry (secure field, 0600 preserved, interpolation entries untouched).
8. Onboarding state machine + empty-state UI + probes (rapid-mlx / router / Ollama).
9. Composer badges, status-bar server dot, palette entries ("Load model…", "Models panel").
10. Demo mode: fake router + fake rapid-mlx state so the panel is demoable offline. **[done,
    2026-07-28]** Both sections seed deterministic fixtures and flow through the same
    `format_router_models`/`format_rapid_mlx_panel` the live path uses (not a parallel
    formatter) — see `backend.rs`'s `models_panel_tests`.

## Risks

- **Router API drift** (llama.cpp moves fast): pin known-good behavior behind one client module;
  degrade to "open pi TUI /llama" hint on unknown responses.
- **rapid-mlx management is CLI-scraping**: no documented admin HTTP API means parsing
  `models`/`info`/`pull`/`serve` output, which can change between releases. Mitigations: prefer
  machine-readable flags if they exist (check for `--json` at impl time), pin a tested version
  range like we do for pi, and degrade to "server detected — manage with the rapid-mlx CLI"
  rather than breaking. Single-model-per-process also means model switching drops the prompt
  cache — surface the restart honestly rather than pretending it's a hot swap.
- **rapid-mlx is macOS/Apple-Silicon-only**: every rapid-mlx surface is platform-gated; Linux
  and Intel Macs must see a coherent panel without it (router/Ollama first).
- **Fit labels overpromise**: quantization, context size, and KV cache all shift real memory
  needs — label as estimate, never block an action on it.
- **models.json corruption**: always write atomically (tmp + rename) and keep one `.bak`;
  malformed existing file → read-only warning, never rewrite what we can't parse.
- **HF search rate limits** without token: debounce, cache, and surface the 429 politely.

## Acceptance criteria

- Fresh machine + running empty router: download starter model, load, chat — no terminal, < 60 s
  after download completes; progress visible throughout.
- Apple Silicon with rapid-mlx installed but idle: pick starter alias → Pull + Serve → chatting,
  no terminal, progress visible; model switch from the panel restarts the managed server and
  lands back in a working chat.
- Running Ollama with models: added to pi and chatting in ≤ 3 clicks.
- `models.json` edits made by the app are accepted by pi TUI unchanged (round-trip test), and
  hand-written entries survive app edits byte-for-byte outside the edited entry.
- Router shared with a concurrent pi TUI stays consistent (both see loads/unloads).
