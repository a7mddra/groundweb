# opensearch

Free, local, best-effort grounded web-search tool for Rust AI apps, wired through **OpenRouter tool calling** (OpenAI-compatible `tools` array).

Any Rust app using an OpenRouter provider can attach the exported tool definition; when the model detects a prompt needs search (e.g. “search github for …”, “search xxxx …”), it calls the tool, receives fetched URLs + briefs + summaries + grounded thinking, and generates the final response from those results. Frontends render the `GroundedReasoning` trace + per-source favicons.

> Status: **Mojeek branch is live.** `execute()` runs the Mojeek query branch (keyless, free) with a keyless safe-source fallback. No DuckDuckGo anywhere, no Gemini API — the fallback URL suggester speaks OpenRouter chat-completions.

## Layout

- `opensearch-rs/src/lib.rs` — crate `opensearch`: `TOOL_NAME`, `tool_definition()`, `SearchArgs`, `SearchOutput`, `execute()`.
- `opensearch-rs/src/branches.rs` — `SearchBranch` fan-out (`Mojeek` today; the real web scraper lands here next).
- `opensearch-rs/src/mojeek.rs` — Mojeek query path. `fetch.rs` — allowlist URL fetch. `html.rs` — parse/rerank/page-text. `safe_sources.rs` + `assets/safe_sources.json` — trusted catalog + keyless fallback. `suggester.rs` — OpenRouter fallback-URL suggester. `favicon.rs` — GLOBAL favicon layer for all branches. `transport.rs` / `retry.rs` / `url_utils.rs` / `constants.rs` / `types.rs` — shared leaves.
- `xtask/src/main.rs` — task runner: `build`, `doctor`, `fmt`, `dev`.
- `.cargo/config.toml` — `cargo xtask` alias. `rust-toolchain.toml` — pinned stable + rustfmt/clippy.

## Quickstart

```sh
cargo xtask doctor
cargo xtask build
cargo xtask dev --live --prompt "search rust openrouter tool calling"   # keyless, real Mojeek branch
cp .env.example .env        # only needed for the OpenRouter model loop below
cargo xtask dev --prompt "search github for rust openrouter tool calling examples and summarize the best approaches"
```

Attach in your app (OpenRouter chat completions):

```rust
let tools = vec![opensearch::tool_definition()];
// POST {base}/chat/completions { model, messages, tools, tool_choice: "auto" }
// on tool_call name == opensearch::TOOL_NAME -> opensearch::execute(args).await
```

## xtask

- `cargo xtask build [--release]` — `cargo build --workspace`.
- `cargo xtask doctor` — toolchain + layout checks; warns (never fails) on missing `OPENROUTER_API_KEY`.
- `cargo xtask fmt [--all]` — default formats only git-changed `*.rs`; `--all` runs `cargo fmt --all`.
- `cargo xtask dev [--prompt ...] [--model ...] [--base-url ...] [--max-iters N]` — OpenRouter tool-call loop with the placeholder tool (needs key). Add `--live [--max-results N]` for the real keyless Mojeek branch instead (no model involved).

## Notes

- `SearchArgs.urls` hints are placeholder-trace-only for now; gated fetch lives in `fetch_url_from_allowed` (URL must come from a previous result in the turn).
- `suggest_fallback_urls` needs an OpenRouter key + model; pair its output with `filter_suggested_urls_to_safe_sources` before fetching.
