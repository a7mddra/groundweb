# AGENTS.md

## Scope guard

- `opensearch-rs/` is a **placeholder**: `execute()` must keep returning `Error::NotImplemented`. Do not add fetch/rank/search network logic unless explicitly tasked. Generic harness first.
- Keep `TOOL_NAME = "web_search"` and `GroundedReasoning` shape stable — frontends render against it.

## Layout

- Cargo workspace root: `Cargo.toml` with members `opensearch-rs`, `xtask`. Library sources live in `opensearch-rs/**`, not repo-root `src/`.
- Crate folder is `opensearch-rs/` but package name is `opensearch` (collides with the official OpenSearch client on crates.io — do not rename/publish without asking).
- `opensearch-rs/src/lib.rs`: `TOOL_NAME`, `tool_definition()`, `SearchArgs`, `FetchedUrl`, `GroundedReasoning`, `SearchOutput::placeholder`, `execute()`.
- `xtask/src/main.rs`: task runner. Alias in `.cargo/config.toml` (`cargo xtask` = `cargo run -p xtask --`).

## Commands (use these, not raw cargo)

- `cargo xtask build [--release]` — builds `--workspace`.
- `cargo xtask doctor` — toolchain + layout check; missing `OPENROUTER_API_KEY` is a warn, not a failure.
- `cargo xtask fmt` — formats **git-changed `*.rs` only**; `cargo xtask fmt --all` = `cargo fmt --all`. Fails outside a git repo unless `--all`.
- `cargo xtask dev [--prompt "..."] [--model ...] [--base-url ...] [--max-iters N]` — live OpenRouter tool-call loop. Needs `.env` (see below). Default prompt is a `search github for ...` task.
- Verify with: `cargo test --workspace`, then `cargo xtask build`. Toolchain is pinned in `rust-toolchain.toml` (stable + rustfmt/clippy), edition 2021.

## OpenRouter wiring (`xtask dev` is the reference)

- Env: `cp .env.example .env`, set `OPENROUTER_API_KEY`. `OPENROUTER_MODEL` defaults to `openrouter/auto`, base `https://openrouter.ai/api/v1`. `.env` is gitignored — never commit it.
- Request: `POST {base}/chat/completions` with `tools: [opensearch::tool_definition()]`, `tool_choice: "auto"`, headers `Authorization: Bearer …`, `HTTP-Referer`, `X-Title`.
- Loop (max-iters default 6): model text → `tool_calls` on `web_search` → parse via `SearchArgs::from_json` → inject `SearchOutput::placeholder` as `role: "tool"` with matching `tool_call_id` → resend. Unknown tool names get a JSON error payload, not a panic.
- Force tool use with search-demanding prompts (`search github for …`, `search xxxx and compare …`); plain Q&A will not trigger the loop, which is expected.
