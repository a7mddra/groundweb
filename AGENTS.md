# AGENTS.md

## Scope guard

- `execute()` is LIVE (auto merges free discovery branches; pasted URLs use native HTTP retrieval). No placeholder API remains.
- No DuckDuckGo anywhere (search backend, `/l/?uddg=` unwrapping, `icons.duckduckgo.com` favicons — all removed for bot-blocking/paid-tier reasons). No Gemini API — the suggester speaks OpenRouter/OpenAI chat-completions.
- New search providers land as a `SearchBranch` variant + one `run_branch` match arm — never as a second parallel pipeline.
- Keep `TOOL_NAME = "web_search"`, `CitationSource`, and `GroundedReasoning` shapes stable — frontends render against them.
- `tools/` donor is gone (ported); `thread_search.rs` was deliberately NOT ported — it is squigit-local scoring over `squigit_storage`, not web search.

## Layout

- Cargo workspace root: `Cargo.toml` with members `opensearch-rs`, `xtask`. Library sources live in `opensearch-rs/src/**`, not repo-root `src/`.
- Crate folder is `opensearch-rs/` but package name is `opensearch` (collides with the official OpenSearch client on crates.io — do not rename/publish without asking).
- `lib.rs`: `TOOL_NAME`, `tool_definition()`, `SearchArgs{query,urls,branch,max_results}`, `SearchOutput::from_web_result`, `execute()`.
- `branches.rs`: `SearchBranch{Auto,Mojeek,Bing,PublicSources,Exa,Parallel}` + `run_branch` dispatch. `mojeek.rs`, `bing.rs`, `public_sources.rs`, `mcp_search.rs`: discovery adapters. `fetch.rs`: public URL/site-adapter retrieval and allowlist fetch. `extract.rs`: native readability. `html.rs`: Mojeek parse + rerank + page-text utils. `safe_sources.rs` + `assets/safe_sources.json`: trusted catalog, keyless fallback. `suggester.rs`: OpenRouter fallback-URL suggester. `transport.rs`/`retry.rs`/`url_utils.rs`/`constants.rs`/`types.rs`: shared leaves. `favicon.rs` is `pub` GLOBAL — every current/future branch must use `citation_source` + `hydrate_favicons_for_sources`, never roll its own icons.
- `xtask/src/main.rs`: task runner. Alias in `.cargo/config.toml` (`cargo xtask` = `cargo run -p xtask --`).

## Commands (use these, not raw cargo)

- `cargo xtask build [--release]` — builds `--workspace`.
- `cargo xtask doctor` — toolchain + layout check; missing `OPENROUTER_API_KEY` is a warn, not a failure (Mojeek branch and `dev --live` are keyless).
- `cargo xtask fmt` — formats **git-changed `*.rs` only**; `cargo xtask fmt --all` = `cargo fmt --all`.
- `cargo xtask dev [--prompt ...] [--model ...] [--base-url ...] [--max-iters N]` — OpenRouter tool-call loop with real local retrieval (needs `.env` key). `--live [--prompt ...] [--max-results N]` — real `execute()` (keyless, no model involved); `--branch`, repeated `--url`, and `--json` select manual runs.
- `cargo xtask bench [--repeat N] [--model FREE_MODEL] [--output PATH]` — manual live workloads, source/page/icon counts and timing; optional model tasks load `.env`. Do not add tests unless asked. Never use subagents.
- Verify with: `cargo test --workspace`, then `cargo xtask build`. Toolchain is pinned in `rust-toolchain.toml` (stable + rustfmt/clippy), edition 2021. `rand` is pinned to `0.8` (`thread_rng().gen_range()` API).

## OpenRouter wiring (`xtask dev` is the reference)

- Env: `cp .env.example .env`, set `OPENROUTER_API_KEY`. `OPENROUTER_MODEL` defaults to `openrouter/free`, base `https://openrouter.ai/api/v1`. `.env` is gitignored — never commit it. Key is needed ONLY for the model loop + `suggest_fallback_urls`, never for Mojeek.
- Request: `POST {base}/chat/completions` with `tools: [opensearch::tool_definition()]`, `tool_choice: "auto"`, headers `Authorization: Bearer …`, `HTTP-Referer`, `X-Title`.
- Loop (max-iters default 6): model text → `tool_calls` on `web_search` → parse via `SearchArgs::from_json` → execute real retrieval and inject context + compact source data as `role: "tool"` with matching `tool_call_id` → resend. Unknown tool names get a JSON error payload, not a panic.
- Force tool use with search-demanding prompts (`search github for …`, `search xxxx and compare …`); plain Q&A will not trigger the loop, which is expected.
- `execute()` failure path: discovery error → relevant catalog seeds → fetch and check content → `mode: "safe_fallback"`. Catalog links are never fabricated evidence. Inaccessible pasted URLs return explicit limitations and no fake source. `SearchArgs.urls` and links in `query` are read directly; apps can use `urls_from_text` to carry pasted user links into the first model tool call. `fetch_url_from_allowed` retains the gate for callers that only permit previously discovered sources.
