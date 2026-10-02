# Live observations — 3 October 2026

The expansion retrieves real source content locally, returns citations and favicons, and completes grounded answers through free OpenRouter models. This sample demonstrates functionality and failure handling; it does not establish an answer-accuracy score or guarantee service availability.

## Method

`cargo xtask bench --repeat 2 --output benchmarks/retrieval-2026-10-03.json` ran twelve workloads twice in one process, using `auto` with twelve sources per search. Timings start inside `execute()` and exclude compilation/CLI startup. The first pass starts with empty page/icon caches, but overlapping workloads warm shared entries: the two-repository comparison reuses the earlier axum read. The second pass reuses caches and provider cooldowns. Zero milliseconds means below the reporting resolution.

OpenRouter tasks loaded the existing `.env` without changing it. Every tested model ID used `:free`. Usage responses reported zero cost for every successful completion. Model latencies include inference and networking; Ling's tasks shared already warmed page caches. Citation counts measure returned URLs present in answers, not whether every claim is supported.

The earlier Mojeek-only probe of `rust reqwest ClientBuilder` was blocked and returned six unfetched catalog links. The new pipeline retrieves page bodies; the reference model loop also executes real retrieval instead of placeholders. There is no comparable pre-change latency measurement.

## Retrieval results

| Workload | First pass | Second pass | Sources / pages read, first pass | Inline icons |
| --- | ---: | ---: | ---: | ---: |
| GitHub repository README + metadata | 1.179 s | 4 ms | 1 / 1 | 1 |
| GitHub raw Cargo.toml | 0.353 s | 1 ms | 1 / 1 | 1 |
| GitHub issue + limited comments | 0.826 s | <1 ms | 1 / 1 | 1 |
| GitHub releases | 0.503 s | <1 ms | 1 / 1 | 1 |
| Public X post | 1.498 s | <1 ms | 1 / 1 | 1 |
| Two repository comparison | 0.488 s | 1 ms | 2 / 2 | 2 |
| Wikipedia article | 0.492 s | <1 ms | 1 / 1 | 1 |
| Technical search: reqwest/timeouts/rustls | 5.218 s | 1.173 s | 12 / 3 | 8 |
| Research search: RAG evaluation | 7.412 s | 4.518 s | 12 / 3 | 6 |
| News search: latest Rust release | 3.827 s | 2.073 s | 12 / 3 | 5 |
| X profile/timeline (unsupported) | 1.123 s | 1.290 s | 0 / 0 | 0 |
| Private IPv4 target (blocked) | <1 ms | <1 ms | 0 / 0 | 0 |

All seven positive URL workloads succeeded on both passes. All three search workloads returned twelve sources and three retrieved pages on both passes. The two negative workloads returned explicit limitations and zero fabricated sources. First-pass positive URL median: **0.503 s**. First-pass search median: **5.218 s**; second-pass search median: **2.073 s**. These are small, network-dependent samples.

Mojeek was blocked in this environment and entered cooldown. Exa and Parallel both returned anonymous search results through the Rust client. Bing RSS worked for some queries and returned no usable results for others. Several direct APIs worked; the Hacker News RSS endpoint returned HTTP 419, and arXiv sometimes hit its four-second job deadline. Other providers preserved results in those cases.

A separate direct-only `public_sources` probe of the technical query returned current reqwest documentation, repository and discussion links without hosted search. This branch has narrower coverage; a general search engine's relevance is not reproduced by querying public vertical APIs.

Additional manual probes confirmed readable Chinese Wikipedia text, static Rust documentation, rejection of private/mapped IPv6 and credential-bearing URLs, and respect for `max_results=1` with two requested URLs. Edge-case CLI wall times include Cargo/startup and are not comparable with the table above.

## Free-model observations

| Model / task | Outcome | Tool calls | Pages read | Total time | Reported cost |
| --- | --- | ---: | ---: | ---: | ---: |
| Ling: compare axum and actix-web READMEs | Answer, both repo URLs cited | 1 | 2 | 5.413 s | $0 |
| Ling: public X post + axum release changes | Exact post text and release notes; both URLs cited | 1 | 2 | 4.018 s | $0 |
| Ling: reqwest total/connect/read timeouts | Follow-up searches and current documentation citations | 4 | 6 | 38.989 s | $0 |
| Dots: axum README + public X post, final-code check | Answer with exact post text and both URLs cited | 1 | 2 | 12.120 s | $0 |
| Qwen 3.8 27B free, two attempts | HTTP 429 from upstream shared free pool; no completion | — | — | — | No completion usage returned |

Model IDs: `inclusionai/ling-3.0-flash-sante:free`, `dots-studio/dots-3-note-preview:free`, `qwen/qwen3.8-27b:free`. Availability was read from OpenRouter's live model catalog. These observations do not promise continued availability.

The first Ling prototype searched for pasted repos rather than sending their URLs and used unresolved numbered references. The final reference loop carries pasted user URLs into the first tool call and requests exact URL citations. Subsequent Ling tasks returned both pasted sources through one real tool call. The underlying library also extracts links from `query` and exposes `urls_from_text` for app integration.

Some generated statements still exceeded the returned evidence, such as identifying the example X post as historically the first tweet. The retrieved widget only establishes the post text. Tool grounding improves the evidence available to a free model; it does not enforce factual support for every sentence. Apps should display retrieval gaps and inspect source links.

## Boundaries

- There is no local web index. Mojeek/Bing and hosted MCP providers supply remote discovery; fetching and native extraction run locally. Exa/Parallel query data is sent to those services.
- Anonymous search and model tiers can block, rate-limit, change limits, or disappear. GitHub's unauthenticated API/search quotas are small. Numeric `Retry-After` and cooldowns reduce repeated traffic but do not eliminate quotas.
- Dynamic/login-only pages, paywalls and CAPTCHAs remain inaccessible. X profiles/timelines, protected/deleted posts, whole threads, media and engagement history are unsupported.
- GitHub retrieval reads a README, file, issue/comments or limited release list, not an entire repository or PR diff. Large/long documents are truncated and marked. PDF/binary extraction is unsupported.
- Search excerpts may be stale; latest-release searches can include nightly/prerelease pages. Current-doc ranking and dates help but are not a freshness guarantee. Public source topic routing is heuristic and Wikipedia discovery currently uses English.
- Favicon hydration is best-effort: all tested direct URLs had inline icons, while searches had 5–8 of 12. Every source retains an origin icon pointer when available; hydration is capped at five seconds per batch.
- Page bodies are capped at 1 MiB, extracted text at 12,000 characters per page, and combined context at 64,000 characters. Direct URL batches stop after twenty seconds. A search tries up to six candidate pages under a ten-second batch budget, stopping after three successful reads. Provider jobs and icon hydration have their own deadlines.
- Configured proxies must be trusted to resolve public destinations correctly. Direct connections validate actual DNS answers; public-only scheme/IP/redirect checks prevent ordinary local-network fetching.

The optimized CLI is approximately **14 MiB** in this environment; no browser/Python runtime was installed. This is the CLI executable size, not the incremental size of the embedded library or a runtime-memory measurement.

## Reproduce and inspect

```sh
cargo xtask bench --repeat 2 --output benchmarks/local.json
cargo xtask bench --model openrouter/free --output benchmarks/model.json
cargo xtask dev --live --branch public_sources --prompt "rust reqwest ClientBuilder timeout rustls" --json
cargo test --workspace
cargo xtask build
cargo xtask build --release
```

The existing **15 tests pass**, and debug build, release build and doctor checks pass. No new test suite was added; the benchmark command runs manual live workloads. Raw observations: [retrieval](retrieval-2026-10-03.json), [Ling model runs](models-2026-10-03.json), [additional models](additional-models-2026-10-03.json), [edge cases](edge-cases-2026-10-03.json).

Anonymous access documentation: [Exa](https://github.com/exa-labs/exa-mcp-server), [Parallel](https://github.com/parallel-web/search-mcp). Public API limits: [GitHub REST rate limits](https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api), [GitHub search](https://docs.github.com/en/rest/search/search).
