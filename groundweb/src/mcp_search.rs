//! Keyless hosted discovery from the research. No credentials or paid fallback.
use crate::{
    branches::SearchBranch,
    constants::MAX_FETCH_BYTES,
    extract::{truncate_chars, visible_fragment},
    favicon::citation_source,
    html::build_query_result,
    transport::{read_capped_response_body, send_with_transport, TransportClients},
    types::{CitationSource, SearchError, SearchFailureClass, WebSearchResult},
    url_utils::{canonicalize_url, ensure_public_target},
};
use reqwest::header;
use serde_json::{json, Value};

pub(crate) async fn search(
    branch: SearchBranch,
    query: &str,
    limit: usize,
    clients: &TransportClients,
) -> Result<WebSearchResult, SearchError> {
    let (endpoint, tool, arguments) = match branch {
        SearchBranch::Exa => (
            "https://mcp.exa.ai/mcp",
            "web_search_exa",
            json!({"query":query,"numResults":limit,"type":"auto"}),
        ),
        SearchBranch::Parallel => (
            "https://search.parallel.ai/mcp",
            "web_search",
            json!({"objective":query,"search_queries":[query]}),
        ),
        _ => unreachable!(),
    };
    ensure_public_target(&url::Url::parse(endpoint).expect("static URL")).await?;
    let body = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":tool,"arguments":arguments}});
    let (response, _) = send_with_transport(clients, |client| {
        client
            .post(endpoint)
            .header(header::ACCEPT, "application/json, text/event-stream")
            .json(&body)
    })
    .await?;
    if !response.status().is_success() {
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(60);
        return Err(SearchError::fatal(
            if response.status().as_u16() == 403 {
                SearchFailureClass::Challenge
            } else {
                SearchFailureClass::HttpStatus
            },
            format!(
                "HTTP {} from anonymous hosted search",
                response.status().as_u16()
            ),
        )
        .with_retry_after(retry_after));
    }
    let raw = read_capped_response_body(response, MAX_FETCH_BYTES).await?;
    let payload = parse_rpc(&raw)?;
    if payload.get("isError").and_then(Value::as_bool) == Some(true) {
        return Err(SearchError::fatal(
            SearchFailureClass::Other,
            format!(
                "Hosted tool failed: {}",
                truncate_chars(&payload.to_string(), 500)
            ),
        ));
    }
    let mut sources = Vec::new();
    if let Some(structured) = payload.get("structuredContent") {
        parse_structured(structured, &mut sources);
    }
    if let Some(content) = payload.get("content").and_then(Value::as_array) {
        for part in content {
            if part["type"] != "text" {
                continue;
            }
            let Some(text) = part["text"].as_str() else {
                continue;
            };
            if let Ok(value) = serde_json::from_str::<Value>(text) {
                parse_structured(&value, &mut sources);
            } else if branch == SearchBranch::Exa {
                parse_exa_text(text, &mut sources);
            }
        }
    }
    sources = crate::html::rerank_sources(query, sources, limit);
    if sources.is_empty() {
        return Err(SearchError::fatal(
            SearchFailureClass::NoResults,
            "Hosted search returned no usable source URLs",
        ));
    }
    Ok(build_query_result(query, sources, None))
}

fn parse_rpc(raw: &str) -> Result<Value, SearchError> {
    fn result(value: Value) -> Result<Value, SearchError> {
        if value["jsonrpc"] != "2.0" || value["id"] != 1 {
            return Err(SearchError::fatal(
                SearchFailureClass::Parse,
                "Mismatched JSON-RPC response",
            ));
        }
        if let Some(error) = value.get("error") {
            return Err(SearchError::fatal(
                SearchFailureClass::Other,
                truncate_chars(&error.to_string(), 500),
            ));
        }
        value
            .get("result")
            .cloned()
            .ok_or_else(|| SearchError::fatal(SearchFailureClass::Parse, "Missing MCP result"))
    }
    if let Ok(value) = serde_json::from_str::<Value>(raw) {
        return result(value);
    }
    // SSE events may contain multiple data lines, CRLF, and notifications.
    let normalized = raw.replace("\r\n", "\n");
    for event in normalized.split("\n\n") {
        let data = event
            .lines()
            .filter_map(|l| l.strip_prefix("data:").map(str::trim_start))
            .collect::<Vec<_>>()
            .join("\n");
        if let Ok(value) = serde_json::from_str::<Value>(&data) {
            if value["id"] == 1 {
                return result(value);
            }
        }
    }
    Err(SearchError::fatal(
        SearchFailureClass::Parse,
        "No matching MCP JSON/SSE response",
    ))
}

fn parse_structured(value: &Value, out: &mut Vec<CitationSource>) {
    let items = value
        .get("results")
        .and_then(Value::as_array)
        .or_else(|| value.as_array());
    if let Some(items) = items {
        for item in items {
            let Some(url) = item["url"].as_str().and_then(|u| canonicalize_url(u).ok()) else {
                continue;
            };
            let excerpts = item.get("excerpts").and_then(Value::as_array).map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("\n\n")
            });
            let text = excerpts
                .or_else(|| {
                    item.get("text")
                        .or_else(|| item.get("content"))
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .unwrap_or_default();
            let date = item
                .get("publish_date")
                .or_else(|| item.get("publishedDate"))
                .and_then(Value::as_str)
                .map(|s| format!("Published: {s}\n"))
                .unwrap_or_default();
            out.push(citation_source(
                item["title"].as_str().unwrap_or(&url).to_string(),
                url,
                truncate_chars(&format!("{date}{text}"), 1600),
            ));
        }
    }
}

fn parse_exa_text(text: &str, out: &mut Vec<CitationSource>) {
    let mut title = String::new();
    let mut url = None;
    let mut content = Vec::new();
    let flush =
        |title: &str, url: &Option<String>, content: &[String], out: &mut Vec<CitationSource>| {
            if let Some(url) = url {
                out.push(citation_source(
                    if title.is_empty() {
                        url.clone()
                    } else {
                        title.to_string()
                    },
                    url.clone(),
                    truncate_chars(&visible_fragment(&content.join("\n")), 1600),
                ));
            }
        };
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("Title:") {
            flush(&title, &url, &content, out);
            title = value.trim().to_string();
            url = None;
            content.clear();
        } else if let Some(value) = line.strip_prefix("URL:") {
            url = canonicalize_url(value.trim()).ok();
        } else if !line.starts_with("ID:") {
            content.push(line.to_string());
        }
    }
    flush(&title, &url, &content, out);
}
