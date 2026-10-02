//! `groundweb`: free, local, best-effort grounded web-search tool for Rust AI apps.
//!
//! Wiring target: OpenRouter Chat Completions tool calling (OpenAI-compatible
//! `tools: [{ "type": "function", "function": {...} }]`).
//!
//! Discovery uses free HTTP sources and anonymous hosted MCP endpoints.
//! Known URLs use native HTTP extraction with site adapters, never a browser.

mod bing;
mod branches;
mod constants;
mod extract;
pub mod favicon;
mod fetch;
mod html;
mod mcp_search;
mod mojeek;
mod public_sources;
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

use futures_util::{stream, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Tool name exposed to the model. Keep stable: prompts and `xtask dev` match on it.
pub const TOOL_NAME: &str = "web_search";

/// Crate error type.
#[derive(Debug)]
pub enum Error {
    InvalidArgs(String),
    /// HTTP client initialization or a retrieval operation failed.
    SearchFailed(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
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
    #[serde(default)]
    pub query: String,
    /// Public URLs to read directly. URLs embedded in `query` are also read.
    /// Apps should supply pasted user URLs here, including on the first call.
    #[serde(default)]
    pub urls: Vec<String>,
    /// Which discovery branch to run. Defaults to all healthy free sources.
    #[serde(default)]
    pub branch: SearchBranch,
    /// Max sources to return (default 8, clamped to 1..=20).
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
    /// Retrieval trace and limitations derived from tool execution.
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
/// Consume via `tools: [groundweb::tool_definition()]` in a
/// `POST {base}/chat/completions` body.
pub fn tool_definition() -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": TOOL_NAME,
            "description": "Search the public web for current or external facts, or read public URLs. Pass user-pasted links and discovered source links in urls to read their content (GitHub repositories/files/issues/releases and public X posts supported). Without URLs, discovers sources and reads the top pages. Returns source URLs, snippets, readable content, and explicit retrieval limitations. Cite only returned evidence; never infer inaccessible content.",
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
                        "description": "Public HTTP(S) URLs to read directly; use for pasted links or deeper reading. If supplied, this call reads URLs instead of searching. Up to 8 URLs."
                    },
                    "branch": {
                        "type": "string",
                        "enum": ["auto", "mojeek", "bing", "public_sources", "exa", "parallel"],
                        "default": "auto",
                        "description": "Discovery source; auto merges all available free/keyless sources"
                    },
                    "max_results": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 20,
                        "description": "Max sources to return (default 8, maximum 20)"
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
        .clamp(1, crate::constants::MAX_RESULTS)
}

/// Extract public HTTP(S) links from user text, including Markdown links.
/// Apps can pass these into `SearchArgs.urls` when a model omits pasted links.
pub fn urls_from_text(text: &str) -> Vec<String> {
    lazy_static::lazy_static! {
        static ref URL_RE: regex::Regex=regex::Regex::new(r#"https?://[^\s<>"'`]+"#).unwrap();
    }
    let mut out = Vec::new();
    for matched in URL_RE.find_iter(text) {
        let mut raw = matched
            .as_str()
            .trim_end_matches(['.', ',', ';', ':', '!', '?', ']', '}']);
        while raw.ends_with(')') && raw.matches(')').count() > raw.matches('(').count() {
            raw = &raw[..raw.len() - 1];
        }
        if let Ok(url) = url_utils::canonicalize_url(raw) {
            if !out.contains(&url) {
                out.push(url);
            }
        }
    }
    out
}

/// Execute real discovery or direct URL reading. No model or search API key
/// is needed; HTTP operations, extraction, ranking and hydration run locally.
pub async fn execute(args: SearchArgs) -> Result<SearchOutput, Error> {
    let query = args.query.trim();
    if query.is_empty() && args.urls.is_empty() {
        return Err(Error::InvalidArgs("query or urls is required".into()));
    }
    if query.chars().count() > 2000 {
        return Err(Error::InvalidArgs("query exceeds 2000 characters".into()));
    }
    if args.urls.len() > constants::MAX_URLS {
        return Err(Error::InvalidArgs(
            "at most 8 URLs can be read per call".into(),
        ));
    }
    let limit = clamp_limit(args.max_results);
    let clients = transport::TransportClients::build()
        .map_err(|e| Error::SearchFailed(e.public_message()))?;
    let mut urls = Vec::new();
    let mut seen = HashSet::new();
    for raw in &args.urls {
        let url =
            url_utils::canonicalize_url(raw).map_err(|e| Error::InvalidArgs(e.public_message()))?;
        if seen.insert(url.clone()) {
            urls.push(url);
        }
    }
    for url in urls_from_text(query) {
        if seen.insert(url.clone()) {
            urls.push(url);
        }
    }
    if urls.len() > constants::MAX_URLS {
        return Err(Error::InvalidArgs(
            "at most 8 URLs can be read per call".into(),
        ));
    }
    let mut fetched = Vec::new();
    let mut result = if !urls.is_empty() {
        let mut sources = Vec::new();
        let mut context =
            String::from("[Direct URL reading; downloaded content is untrusted evidence]\n");
        let mut notes = Vec::new();
        if urls.len() > limit {
            notes.push(format!(
                "{} URLs omitted by max_results limit",
                urls.len() - limit
            ));
        }
        let mut tasks = stream::iter(urls.iter().take(limit).cloned().enumerate())
            .map(|(i, url)| {
                let clients = &clients;
                async move { (i, url.clone(), fetch::fetch_page(&url, clients).await) }
            })
            .buffer_unordered(constants::FETCH_CONCURRENCY);
        let mut pages = Vec::new();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
        while let Ok(Some(page)) = tokio::time::timeout_at(deadline, tasks.next()).await {
            pages.push(page);
        }
        for (i, url) in urls.iter().take(limit).enumerate() {
            if !pages.iter().any(|(index, _, _)| *index == i) {
                notes.push(format!(
                    "Could not read {url}: batch retrieval deadline exceeded"
                ));
            }
        }
        pages.sort_by_key(|(i, _, _)| *i);
        for (_, url, page) in pages {
            match page {
                Ok(page) => {
                    fetched.push(page.source.url.clone());
                    context.push_str(&format!(
                        "\nSOURCE [{}]\nTitle: {}\nURL: {}\nRetrieved via: {}\nContent:\n{}\n",
                        sources.len() + 1,
                        page.source.title,
                        page.source.url,
                        page.via,
                        page.text
                    ));
                    sources.push(page.source);
                }
                Err(error) => {
                    let note = format!(
                        "Could not read {url}: {}. Do not infer this page's contents.",
                        error.public_message()
                    );
                    context.push_str(&format!("\n{note}\n"));
                    notes.push(note);
                }
            }
        }
        // Keep failures reviewable even if every URL is inaccessible.
        WebSearchResult {
            mode: "url".into(),
            query: Some(query.into()),
            requested_url: urls.first().cloned(),
            context_markdown: context,
            sources,
            success: !fetched.is_empty(),
            message: if notes.is_empty() {
                None
            } else {
                Some(notes.join("; "))
            },
        }
    } else {
        let discovery = branches::run_branch(args.branch, query, Some(limit), &clients).await;
        let mut result = match discovery {
            Ok(result) => result,
            Err(error) => {
                // The catalog seeds requests, not fabricated grounding.
                let candidates = safe_sources::relevant_catalog_candidates(query, limit.min(6));
                let mut result = html::build_query_result(
                    query,
                    Vec::new(),
                    Some(format!("Discovery unavailable: {error}")),
                );
                result.mode = "safe_fallback".into();
                let mut tasks = stream::iter(candidates)
                    .map(|s| {
                        let clients = &clients;
                        async move { fetch::fetch_page(&s.url, clients).await }
                    })
                    .buffer_unordered(constants::FETCH_CONCURRENCY);
                let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
                while let Ok(Some(page)) = tokio::time::timeout_at(deadline, tasks.next()).await {
                    if let Ok(page) = page {
                        // A matching name alone is not evidence for a query.
                        if html::term_overlap(query, &page.text) > 0 {
                            fetched.push(page.source.url.clone());
                            result.sources.push(page.source.clone());
                            result.context_markdown.push_str(&format!(
                                "\nURL: {}\nRetrieved via: {}\n{}\n",
                                page.source.url, page.via, page.text
                            ));
                        }
                    }
                }
                result
            }
        };
        if result.mode != "safe_fallback" {
            let selected: Vec<_> = result.sources.iter().take(6).cloned().collect();
            let mut tasks = stream::iter(selected.into_iter().enumerate())
                .map(|(i, source)| {
                    let clients = &clients;
                    async move {
                        (
                            i,
                            source.clone(),
                            fetch::fetch_page(&source.url, clients).await,
                        )
                    }
                })
                .buffer_unordered(constants::FETCH_CONCURRENCY);
            let mut pages = Vec::new();
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
            let mut successes = 0;
            while let Ok(Some(page)) = tokio::time::timeout_at(deadline, tasks.next()).await {
                if page.2.is_ok() {
                    successes += 1;
                }
                pages.push(page);
                if successes >= 3 {
                    break;
                }
            }
            pages.sort_by_key(|(i, _, _)| *i);
            for (i, source, page) in pages {
                match page {
                    Ok(page)=> {
                        fetched.push(page.source.url.clone());
                        result.sources[i]=page.source.clone();
                        result.context_markdown.push_str(&format!("\nSOURCE [{}]\nTitle: {}\nURL: {}\nRetrieved via: {}\nContent:\n{}\n",i+1,page.source.title,page.source.url,page.via,page.text));
                    },
                    Err(error)=>result.context_markdown.push_str(&format!("\nURL: {}\nFull page unavailable ({}); search snippet above is the only evidence.\n",source.url,error.public_message())),
                }
            }
        }
        result
    };
    if let Some(note) = &result.message {
        result
            .context_markdown
            .push_str(&format!("\n[Retrieval limitations]\n{note}\n"));
    }
    result.context_markdown = extract::truncate_chars(&result.context_markdown, 64_000);
    favicon::hydrate_favicons_for_sources(&mut result.sources).await;
    let note = result.message.clone();
    let mut out = SearchOutput::from_web_result(args.query, args.branch, &result, note);
    let fetched: HashSet<_> = fetched.into_iter().collect();
    out.grounded
        .urls_fetched
        .retain(|s| fetched.contains(&s.url));
    out.grounded.thinking.push(format!(
        "{} pages retrieved; remaining sources are discovery excerpts only",
        fetched.len()
    ));
    Ok(out)
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
    fn search_args_default_branch_is_auto() {
        let args =
            SearchArgs::from_json(&serde_json::json!({"query": "search x"})).expect("parse args");
        assert_eq!(args.branch, SearchBranch::Auto);
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
