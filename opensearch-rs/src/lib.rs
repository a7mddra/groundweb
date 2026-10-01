//! `opensearch`: free, local, best-effort grounded web-search tool for Rust AI apps.
//!
//! Wiring target: OpenRouter Chat Completions tool calling (OpenAI-compatible
//! `tools: [{ "type": "function", "function": {...} }]`).
//!
//! # Current status: placeholder
//!
//! The generic surface (tool JSON schema + grounded-reasoning types for
//! frontend rendering) is stable. **Search/fetch/summarize logic is
//! intentionally NOT implemented yet** — [`execute`] returns
//! [`Error::NotImplemented`]. Do not add network search logic in this crate
//! without an explicit task; build the generic harness (`xtask dev`) first.

use serde::{Deserialize, Serialize};

/// Tool name exposed to the model. Keep stable: prompts and `xtask dev` match on it.
pub const TOOL_NAME: &str = "web_search";

/// Placeholder error type. Search I/O errors will extend this later.
#[derive(Debug)]
pub enum Error {
    NotImplemented(&'static str),
    InvalidArgs(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotImplemented(msg) => write!(f, "not implemented (placeholder): {msg}"),
            Error::InvalidArgs(msg) => write!(f, "invalid tool args: {msg}"),
        }
    }
}

impl std::error::Error for Error {}

/// Arguments the model passes when it calls the tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchArgs {
    /// The search task, e.g. "search github for rust openrouter tool calling examples".
    pub query: String,
    /// Optional hint URLs the model wants grounded (echoed back in trace).
    #[serde(default)]
    pub urls: Vec<String>,
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

/// Placeholder tool output. Shape is what frontends should render against.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchOutput {
    pub query: String,
    pub answer_stub: String,
    pub grounded: GroundedReasoning,
}

impl SearchOutput {
    /// Deterministic placeholder payload used by `xtask dev` to prove the
    /// OpenRouter tool-call loop without doing any real search.
    pub fn placeholder(args: &SearchArgs) -> Self {
        Self {
            query: args.query.clone(),
            answer_stub: format!(
                "placeholder result for {:?} (search not implemented yet)",
                args.query
            ),
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
                    }
                },
                "required": ["query"],
                "additionalProperties": false
            }
        }
    })
}

/// Placeholder executor. Always returns [`Error::NotImplemented`].
///
/// Kept so call sites and `xtask dev` can be wired end-to-end before any
/// search logic lands.
pub async fn execute(_args: SearchArgs) -> Result<SearchOutput, Error> {
    Err(Error::NotImplemented(
        "search/fetch/summarize intentionally unimplemented; use SearchOutput::placeholder in dev harness",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_definition_matches_openrouter_function_shape() {
        let def = tool_definition();
        assert_eq!(def["type"], "function");
        assert_eq!(def["function"]["name"], TOOL_NAME);
        assert!(def["function"]["parameters"]["properties"]["query"].is_object());
    }

    #[test]
    fn placeholder_roundtrips_through_json() {
        let args = SearchArgs {
            query: "search github for rust openrouter tool calling".into(),
            urls: vec![],
        };
        let out = SearchOutput::placeholder(&args);
        let v = serde_json::to_value(&out).unwrap();
        assert_eq!(v["query"], args.query);
    }
}
