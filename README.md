# groundweb

Free, best-effort web grounding for Rust apps and OpenRouter tool calling. HTTP retrieval, parsing, ranking, caching and favicon hydration run on the user's machine. Web indexes and source websites remain remote. There is no search API key, browser runtime, Python sidecar or paid search fallback.

`web_search` now executes real searches **and reads pasted URLs**. The default `auto` branch merges healthy free sources. Frontends keep the same `CitationSource` and `GroundedReasoning` shapes.

## Discovery branches

| Branch | Source | Access |
| --- | --- | --- |
| `auto` | Merge the branches below, deduplicate, rerank, diversify hosts | Default |
| `mojeek` | Mojeek search HTML | Keyless; may block automation |
| `bing` | Bing search RSS | Keyless; may return empty/limited results |
| `public_sources` | Wikipedia, GitHub repositories/issues, Hacker News, Stack Exchange, arXiv, crates.io, relevant catalog RSS/Atom feeds | Direct public endpoints; selected by query topic |
| `exa` | Exa's anonymous hosted MCP search | Free/keyless, service-controlled limits |
| `parallel` | Parallel's anonymous hosted MCP search | Free/keyless, service-controlled limits |

Exa and Parallel run search infrastructure remotely. Their anonymous tiers can change or rate-limit; they are never given search credentials and never upgraded to paid requests. Provider errors appear in the returned context. See [Exa's anonymous access documentation](https://github.com/exa-labs/exa-mcp-server) and [Parallel's free-tier documentation](https://github.com/parallel-web/search-mcp).

More sources improve coverage, but source relevance, freshness and reading actual pages determine grounding quality. The tool prefers matching titles/snippets, agreement across branches, current Rust documentation and host diversity. It preserves meaningful URL parameters and removes fragments/tracking parameters. A catalog link alone is never treated as fetched evidence.

## URL reading

Supply `urls`, or embed HTTP(S) URLs in `query`. That call reads the URLs instead of performing discovery. A subsequent call without URLs searches normally.

- GitHub repository: public metadata and README; `.git` repository URLs work.
- GitHub `/blob/` file: raw file text.
- GitHub issue/pull request: issue body and up to ten comments; this does not retrieve a PR diff.
- GitHub releases, latest release, release tag: public release notes; lists are capped at five.
- Public X/Twitter status: official oEmbed text, then best-effort public widget JSON. Profiles, timelines, protected/deleted posts and full threads are unsupported.
- Wikipedia article: public article-text API, avoiding oversized page chrome.
- Other pages: native Readability Markdown extraction with a static HTML fallback; text, Markdown, JSON and RSS/Atom are also readable.

Failures retain whatever evidence is available, with explicit limitations. Search-discovered pages that fail retrieval keep their discovery excerpts. An inaccessible pasted URL contributes no fabricated source. `grounded.urls_fetched` includes only successfully retrieved pages; `sources` also includes discovery excerpts.

## Use in an app

```sh
cargo add groundweb
```

```rust
let tools = vec![groundweb::tool_definition()];
// POST OpenRouter /chat/completions with tools and tool_choice: "auto".
// When the model calls web_search, parse arguments and execute locally:
let mut args = groundweb::SearchArgs::from_json(&tool_arguments)?;
// Carry user-pasted links into the first call if the model omits them.
args.urls.extend(groundweb::urls_from_text(user_prompt));
args.urls.sort();
args.urls.dedup();
let output = groundweb::execute(args).await?;
// Preserve the assistant's entire tool_calls array, then append each tool
// result with the matching tool_call_id. Ask the model to cite returned URLs.
```

Carry pasted links only into the first call; subsequent calls should use the model's requested sources. Treat downloaded content as untrusted evidence. Send the model `context_markdown` and source titles/URLs/summaries; keep inline favicon bytes and duplicate UI traces out of its context. `xtask dev` demonstrates this complete loop.

The result limit defaults to eight and accepts up to twenty. Search reads up to three pages, trying up to six candidates when earlier pages fail; URL mode accepts up to eight URLs and respects `max_results`. Keep `TOOL_NAME = "web_search"`, `CitationSource`, and `GroundedReasoning` stable in consumers. The old placeholder API has been removed.

## Run locally

```sh
cargo xtask doctor
cargo xtask build
cargo xtask dev --live --prompt "rust reqwest ClientBuilder timeout rustls"
cargo xtask dev --live --url https://github.com/tokio-rs/axum --prompt "Read the README" --json
cargo xtask dev --live --branch public_sources --prompt "retrieval augmented generation papers"

# Only the model loop needs .env; retrieval is keyless.
cp .env.example .env
# Set OPENROUTER_API_KEY; select a currently available free tool-capable model.
cargo xtask dev --model openrouter/free --prompt "Read https://github.com/tokio-rs/axum and cite its middleware design"

# Manual live workloads, timing and source/page/icon counts:
cargo xtask bench --repeat 2 --output benchmarks/local.json
cargo xtask bench --model openrouter/free --output benchmarks/model.json
cargo test --workspace
cargo xtask build
```

`OPENROUTER_MODEL` overrides the `openrouter/free` model-loop default. Existing `.env` files using `openrouter/auto` may select paid models; use an explicit free endpoint for zero-cost runs. The benchmark runner rejects paid model IDs. Anonymous model availability and shared rate limits still apply.

`cargo xtask fmt` formats changed Rust files; `--all` formats the workspace. `dev --live` bypasses the model; ordinary `dev` executes the real tool loop. `--branch`, `--max-results`, repeated `--url`, `--max-iters` and `--json` control manual runs.

## Bounds and limitations

Discovery branches have a ten-second deadline. Public source jobs have four-second deadlines. Each page has a ten-second deadline, direct URL batches stop after twenty seconds, and favicon hydration has a five-second batch deadline. Pages use concurrency three; icons use four. Responses are capped at 1 MiB, readable text at 12,000 characters per page, and combined context at 64,000 characters. Page/icon caches are bounded to 64 entries, with five/ten-minute success TTLs. Blocks/rate limits trigger cooldowns; numeric `Retry-After` is honored where available.

Every redirect is checked, credentials/non-HTTP URLs are rejected, and private/reserved IP targets are blocked. Direct DNS connections validate the actual resolved addresses; configured proxies must also be trusted to resolve public destinations correctly.

There is no JavaScript execution, login, CAPTCHA bypass, paywall bypass, PDF/binary extraction or complete repository crawl. Direct search APIs have quotas; GitHub's unauthenticated REST/search limits are particularly small. News searches can include stale snippets, previews or prereleases. Short excerpts do not establish every claim an LLM makes; verify citations and dates. Public X widget JSON is undocumented and may change.

Measured workloads and model limitations are recorded in [benchmarks/RESULTS.md](benchmarks/RESULTS.md).

## Layout

`groundweb/` contains package `groundweb`; `xtask/` is the task runner. `branches.rs` is the single discovery dispatcher. `mojeek.rs`, `bing.rs`, `public_sources.rs`, and `mcp_search.rs` implement discovery. `fetch.rs` handles URL reading and site adapters; `extract.rs` handles native readability. `html.rs` ranks and normalizes result context. `safe_sources.rs` and `assets/safe_sources.json` select catalog seeds/feeds. `favicon.rs` is the shared public icon layer. `transport.rs`, `retry.rs`, `types.rs` and `url_utils.rs` are shared leaves. `suggester.rs` remains an optional OpenRouter URL suggestion helper; suggestions still require retrieval before grounding.

## Publishing

After committing release changes, run `cargo xtask publish --dry-run` to verify the standalone package, then `cargo xtask publish` to upload `groundweb` to crates.io. Both commands use locked dependencies; `xtask` itself is never published.

## License

Licensed under [MIT](LICENSE).
