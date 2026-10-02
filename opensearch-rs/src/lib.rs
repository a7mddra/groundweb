//! `opensearch`: free, local, best-effort grounded web-search tool for Rust AI apps.
//!
//! Wiring target: OpenRouter Chat Completions tool calling (OpenAI-compatible
//! `tools: [{ "type": "function", "function": {...} }]`).
//!
//! # Branches
//!
//! `execute()` fans out to ONE [`SearchBranch`] per call. Today only
//! [`SearchBranch::Mojeek`] exists (Mojeek HTML search + safe-source rerank,
//! keyless and free); the planned real web scraper lands as a new variant.
//! All branches share the global [`favicon`] layer, `safe_sources`,
//! `transport` and `fetch` modules.
//!
//! There is intentionally NO DuckDuckGo backend (bot blocking / paid tier)
//! and NO Gemini API usage — the fallback URL suggester speaks
//! OpenRouter/OpenAI chat-completions syntax.
//!
//! `thread_search.rs` from the donor was NOT ported: it is squigit-local
//! thread scoring over `squigit_storage`, not web search.

mod branches;
mod constants;
pub mod favicon;
mod fetch;
mod html;
mod mojeek;
mod retry;
mod safe_sources;
mod suggester;
mod transport;
mod types;
mod url_utils;

pub use branches::SearchBranch;
pub use favicon::{citation_source, favicon_for_url, hydrate_favicons_for_sources};
pub use fetch::{
    collect_allowed_sources, fetch_url_from_allowed, fetch_url_from_allowed_with_progress,
};
pub use html::build_query_result;
pub use mojeek::{search_query, search_query_with_progress};
pub use safe_sources::{filter_suggested_urls_to_safe_sources, local_safe_source_candidates};
pub use suggester::suggest_fallback_urls;
pub use types::{CitationSource, WebSearchResult};
pub use url_utils::domain_from_url;

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Tool name exposed to the model. Keep stable: prompts and `xtask dev` match on it.
pub const TOOL_NAME: &str = "web_search";

/// Crate error type.
#[derive(Debug)]
pub enum Error {
    /// Reserved for branches that are declared but not built yet.
    NotImplemented(&'static str),
    InvalidArgs(String),
    /// The branch ran and failed (Mojeek down AND safe-source fallback empty).
    SearchFailed(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotImplemented(msg) => write!(f, "not implemented: {msg}"),
            Error::InvalidArgs(msg) => write!(f, "invalid tool args: {msg}"),
            Error::SearchFailed(msg) => write!(f, "search failed: {msg}"),
        }
    }
}

impl std::error::Error for Error {}

/// Arguments the model passes when it calls the tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchArgs {
    /// The search task, e.g. "search github for rust openrouter tool calling examples".
    pub query: String,
    /// Optional hint URLs the model wants grounded. Currently echoed back in
    /// the placeholder trace only; branch retrieval ignores them (allowlist
    /// fetch for hint URLs is future work — see `fetch_url_from_allowed`).
    #[serde(default)]
    pub urls: Vec<String>,
    /// Which branch of the fan-out to run. Defaults to Mojeek.
    #[serde(default)]
    pub branch: SearchBranch,
    /// Max sources to return (clamped to 1..=6).
    #[serde(default)]
    pub max_results: Option<usize>,
}

impl SearchArgs {
    pub fn from_json(value: &serde_json::Value) -> Result<Self, Error> {
        serde_json::from_value(value.clone()).map_err(|e| Error::InvalidArgs(e.to_string()))
    }
}

/// One fetched source, for frontend rendering.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchedUrl {
    pub url: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub snippet: String,
}

/// Grounded reasoning trace. `xtask dev` prints this; frontends render it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GroundedReasoning {
    /// URLs the tool claims it fetched/grounded on.
    #[serde(default)]
    pub urls_fetched: Vec<FetchedUrl>,
    /// Short per-source briefs.
    #[serde(default)]
    pub briefs: Vec<String>,
    /// Longer combined summaries.
    #[serde(default)]
    pub summaries: Vec<String>,
    /// Model thinking derived from the fetches (not pre-search thinking).
    #[serde(default)]
    pub thinking: Vec<String>,
}

/// Tool output. Shape is what frontends should render against:
/// `sources` carries favicons (`favicon_url` pointer + best-effort inlined
/// `favicon_base64`), `grounded` carries the render trace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchOutput {
    pub query: String,
    /// Which branch produced this result (`mojeek`, `safe_fallback`, ...).
    #[serde(default)]
    pub mode: String,
    pub answer_stub: String,
    #[serde(default)]
    pub context_markdown: String,
    #[serde(default)]
    pub sources: Vec<CitationSource>,
    pub grounded: GroundedReasoning,
}

impl SearchOutput {
    /// Deterministic placeholder payload used by `xtask dev` to prove the
    /// OpenRouter tool-call loop without doing any real search.
    pub fn placeholder(args: &SearchArgs) -> Self {
        Self {
            query: args.query.clone(),
            mode: "placeholder".to_string(),
            answer_stub: format!(
                "placeholder result for {:?} (search not executed)",
                args.query
            ),
            context_markdown: String::new(),
            sources: vec![],
            grounded: GroundedReasoning {
                urls_fetched: args
                    .urls
                    .iter()
                    .map(|u| FetchedUrl {
                        url: u.clone(),
                        title: String::new(),
                        snippet: "placeholder: no fetch performed".into(),
                    })
                    .collect(),
                briefs: vec!["placeholder brief: no sources fetched".into()],
                summaries: vec![],
                thinking: vec![
                    "placeholder thinking: tool was called, real grounding pending".into(),
                ],
            },
        }
    }

    /// Build real output from a branch result. `fallback_note` is set when this
    /// came from the safe-source fallback instead of the branch itself.
    pub fn from_web_result(
        query: String,
        branch: SearchBranch,
        result: &WebSearchResult,
        fallback_note: Option<String>,
    ) -> Self {
        let urls_fetched = result
            .sources
            .iter()
            .map(|s| FetchedUrl {
                url: s.url.clone(),
                title: s.title.clone(),
                snippet: s.summary.clone(),
            })
            .collect::<Vec<_>>();
        let briefs = result
            .sources
            .iter()
            .map(|s| {
                if s.summary.trim().is_empty() {
                    format!("{} — {}", s.title, s.url)
                } else {
                    s.summary.clone()
                }
            })
            .collect::<Vec<_>>();
        let mut thinking = vec![format!(
            "branch={} mode={} results={}",
            branch.as_str(),
            result.mode,
            result.sources.len(),
        )];
        if let Some(note) = fallback_note.as_deref() {
            thinking.push(note.to_string());
        }
        let answer_stub = if result.sources.is_empty() {
            format!("no sources found for {:?}", query)
        } else {
            format!(
                "{} source(s) via {} for {:?}",
                result.sources.len(),
                result.mode,
                query
            )
        };

        Self {
            query,
            mode: result.mode.clone(),
            answer_stub,
            context_markdown: result.context_markdown.clone(),
            sources: result.sources.clone(),
            grounded: GroundedReasoning {
                urls_fetched,
                briefs,
                summaries: vec![result.context_markdown.clone()],
                thinking,
            },
        }
    }
}

/// OpenRouter / OpenAI-compatible function-tool definition.
///
/// Consume via `tools: [opensearch::tool_definition()]` in a
/// `POST {base}/chat/completions` body.
pub fn tool_definition() -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": TOOL_NAME,
            "description": "Grounded web search. Call this when the prompt needs fresh/external facts (e.g. 'search github for ...', 'search ...'). Returns fetched URLs, briefs, summaries, and grounded thinking for frontend rendering.",
            "parameters": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search task, e.g. 'search github for rust openrouter tool calling'"
                    },
                    "urls": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional hint URLs to ground on"
                    },
                    "branch": {
                        "type": "string",
                        "enum": ["mojeek"],
                        "default": "mojeek",
                        "description": "Which search branch to run (more branches coming)"
                    },
                    "max_results": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 6,
                        "description": "Max sources to return (default 6)"
                    }
                },
                "required": ["query"],
                "additionalProperties": false
            }
        }
    })
}

fn clamp_limit(max_results: Option<usize>) -> usize {
    max_results
        .unwrap_or(crate::constants::DEFAULT_MAX_RESULTS)
        .clamp(1, crate::constants::DEFAULT_MAX_RESULTS)
}

/// Run the requested branch. On branch failure, falls back to keyless local
/// safe-source candidates (`mode: "safe_fallback"`) so the tool stays useful
/// when Mojeek blocks automated queries.
pub async fn execute(args: SearchArgs) -> Result<SearchOutput, Error> {
    if args.query.trim().is_empty() {
        return Err(Error::InvalidArgs("query is empty".to_string()));
    }
    let limit = clamp_limit(args.max_results);

    match branches::run_branch(args.branch, args.query.trim(), Some(limit)).await {
        Ok(result) => Ok(SearchOutput::from_web_result(
            args.query.clone(),
            args.branch,
            &result,
            None,
        )),
        Err(branch_error) => {
            let sources = local_safe_source_candidates(args.query.trim(), &HashSet::new(), limit);
            if sources.is_empty() {
                return Err(Error::SearchFailed(branch_error));
            }
            let mut with_icons = sources;
            favicon::hydrate_favicons_for_sources(&mut with_icons).await;
            let mut result = build_query_result(
                args.query.trim(),
                with_icons,
                Some(format!(
                    "Mojeek unavailable ({}); showing trusted-source candidates.",
                    branch_error
                )),
            );
            result.mode = "safe_fallback".to_string();
            Ok(SearchOutput::from_web_result(
                args.query.clone(),
                args.branch,
                &result,
                Some(format!("fallback after branch error: {}", branch_error)),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn tool_definition_matches_openrouter_function_shape() {
        let def = tool_definition();
        assert_eq!(def["type"], "function");
        assert_eq!(def["function"]["name"], TOOL_NAME);
        assert!(def["function"]["parameters"]["properties"]["query"].is_object());
        assert!(def["function"]["parameters"]["properties"]["branch"].is_object());
    }

    #[test]
    fn placeholder_roundtrips_through_json() {
        let args = SearchArgs {
            query: "search github for rust openrouter tool calling".into(),
            urls: vec![],
            branch: SearchBranch::Mojeek,
            max_results: None,
        };
        let out = SearchOutput::placeholder(&args);
        let v = serde_json::to_value(&out).unwrap();
        assert_eq!(v["query"], args.query);
    }

    #[test]
    fn search_args_default_branch_is_mojeek() {
        let args =
            SearchArgs::from_json(&serde_json::json!({"query": "search x"})).expect("parse args");
        assert_eq!(args.branch, SearchBranch::Mojeek);
        assert!(args.urls.is_empty());
    }

    #[test]
    fn canonicalize_blocks_non_http() {
        let out = crate::url_utils::canonicalize_url("file:///etc/passwd");
        assert!(out.is_err());
    }

    #[test]
    fn compact_summary_truncates() {
        let text = "one two three four five six";
        let out = crate::html::compact_summary(text, 4);
        assert_eq!(out, "one two three four...");
    }

    #[test]
    fn mojeek_block_detection_works() {
        let html = "<title>403 - Forbidden</title> Sorry your network appears to be sending automated queries";
        assert!(crate::html::looks_like_mojeek_block_page(html));
    }

    #[test]
    fn parses_mojeek_results_fixture() {
        let html = r#"
        <ul>
          <li class="results-standard">
            <h2><a href="https://example.com/post">Example Post</a></h2>
            <p class="s">Example snippet from fixture.</p>
          </li>
          <li class="results-standard">
            <h2><a href="https://docs.rust-lang.org/book/">Rust Book</a></h2>
            <p class="description">Learn Rust from official docs.</p>
          </li>
        </ul>
        "#;

        let out = crate::html::parse_mojeek_results(html, 5);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].title, "Example Post");
        assert!(out[0].url.starts_with("https://example.com/post"));
    }

    #[test]
    fn safe_source_candidates_respect_attempted_domains() {
        let mut attempted = HashSet::new();
        attempted.insert("news.ycombinator.com".to_string());

        let candidates = local_safe_source_candidates("latest ai models", &attempted, 5);
        assert!(!candidates.is_empty());
        for c in candidates {
            let domain = domain_from_url(&c.url).expect("domain expected");
            assert_ne!(domain, "news.ycombinator.com");
        }
    }

    #[test]
    fn filter_suggested_urls_keeps_only_safe_domains() {
        let attempted = HashSet::new();
        let urls = vec![
            "https://news.ycombinator.com/item?id=123".to_string(),
            "https://example.org/unsafe".to_string(),
        ];
        let out = filter_suggested_urls_to_safe_sources(&urls, &attempted, 5);

        assert_eq!(out.len(), 1);
        assert!(out[0].url.contains("news.ycombinator.com"));
    }

    #[test]
    fn from_web_result_maps_sources_to_grounded_trace() {
        let result = WebSearchResult {
            mode: "query".to_string(),
            query: Some("test".to_string()),
            requested_url: None,
            context_markdown: "ctx".to_string(),
            success: true,
            message: None,
            sources: vec![CitationSource {
                title: "Example".to_string(),
                url: "https://example.com/path".to_string(),
                summary: "Summary".to_string(),
                favicon_url: None,
                favicon_base64: None,
            }],
        };
        let out =
            SearchOutput::from_web_result("test".to_string(), SearchBranch::Mojeek, &result, None);
        assert_eq!(out.grounded.urls_fetched.len(), 1);
        assert_eq!(out.sources.len(), 1);
        assert_eq!(out.context_markdown, "ctx");
    }

    #[test]
    fn collect_allowed_sources_canonicalizes_and_maps() {
        let result = WebSearchResult {
            mode: "query".to_string(),
            query: Some("test".to_string()),
            requested_url: None,
            context_markdown: String::new(),
            success: true,
            message: None,
            sources: vec![CitationSource {
                title: "Example".to_string(),
                url: "https://example.com:443/path#section".to_string(),
                summary: "Summary".to_string(),
                favicon_url: None,
                favicon_base64: None,
            }],
        };

        let allowed = collect_allowed_sources(&result);
        assert!(allowed.contains_key("https://example.com/path"));
    }
}
