# opensearch

Free, local, best-effort grounded web-search tool for Rust AI apps, wired through **OpenRouter tool calling** (OpenAI-compatible `tools` array).

Any Rust app using an OpenRouter provider can attach the exported tool definition; when the model detects a prompt needs search (e.g. “search github for …”, “search xxxx …”), it calls the tool, receives fetched URLs + briefs + summaries + grounded thinking, and generates the final response from those results. Frontends render the `GroundedReasoning` trace.

> Status: **generic harness only — search logic is a placeholder.** The crate exports the tool schema and grounded types plus `SearchOutput::placeholder`; `execute()` returns `NotImplemented`. No fetch/summarize network code lives here yet.

## Layout

- `opensearch-rs/src/lib.rs` — crate `opensearch`: `TOOL_NAME`, `tool_definition()`, `SearchArgs`, `FetchedUrl`, `GroundedReasoning`, `SearchOutput`, `execute()` (placeholder).
- `xtask/src/main.rs` — task runner: `build`, `doctor`, `fmt`, `dev`.
- `.cargo/config.toml` — `cargo xtask` alias. `rust-toolchain.toml` — pinned stable + rustfmt/clippy.

## Quickstart

```sh
cp .env.example .env        # set OPENROUTER_API_KEY
cargo xtask doctor
cargo xtask build
cargo xtask dev --prompt "search github for rust openrouter tool calling examples and summarize the best approaches"
```

Attach in your app (OpenRouter chat completions):

```rust
let tools = vec![opensearch::tool_definition()];
// POST {base}/chat/completions { model, messages, tools, tool_choice: "auto" }
// on tool_call name == opensearch::TOOL_NAME -> SearchOutput::placeholder(args) for now
```

## xtask

- `cargo xtask build [--release]` — `cargo build --workspace`.
- `cargo xtask doctor` — toolchain + layout checks; warns (never fails) on missing `OPENROUTER_API_KEY`.
- `cargo xtask fmt [--all]` — default formats only git-changed `*.rs`; `--all` runs `cargo fmt --all`.
- `cargo xtask dev [--prompt ...] [--model ...] [--base-url ...] [--max-iters N]` — live OpenRouter tool-call loop with the placeholder tool. Forces search-requiring prompts, prints the grounded trace, then the model’s final grounded answer.

## Roadmap (search logic explicitly out of scope for now)

Generic part first (this repo): schema, grounded types, `xtask dev` loop. Real fetch/rank/summarize comes later without changing the tool name or harness shape.
