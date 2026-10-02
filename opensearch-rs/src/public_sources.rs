//! Direct, keyless discovery: public site APIs plus relevant catalog feeds.
use crate::{
    constants::FETCH_CONCURRENCY,
    extract::{truncate_chars, visible_fragment},
    favicon::citation_source,
    fetch::{fetch_document, fetch_json, parse_feed},
    html::{build_query_result, query_terms, rerank_sources},
    transport::TransportClients,
    types::{CitationSource, SearchError, SearchFailureClass, WebSearchResult},
};
use futures_util::{stream, StreamExt};
use serde_json::Value;
use std::time::Duration;
use url::Url;

#[derive(Clone, Copy)]
enum Kind {
    Wikipedia,
    Github,
    HackerNews,
    StackExchange,
    Crates,
    Arxiv,
    Feed,
}

pub(crate) async fn search(
    query: &str,
    limit: usize,
    clients: &TransportClients,
) -> Result<WebSearchResult, SearchError> {
    let terms = query_terms(query);
    let primary_crate = terms
        .iter()
        .find(|t| {
            matches!(
                t.as_str(),
                "reqwest"
                    | "tokio"
                    | "axum"
                    | "serde"
                    | "clap"
                    | "rayon"
                    | "sqlx"
                    | "hyper"
                    | "tower"
                    | "actix-web"
            )
        })
        .cloned();
    let q = terms
        .clone()
        .into_iter()
        .take(16)
        .collect::<Vec<_>>()
        .join(" ");
    if q.is_empty() {
        return Err(SearchError::fatal(
            SearchFailureClass::NoResults,
            "No searchable query terms",
        ));
    }
    let lower = query.to_lowercase();
    let site_query = primary_crate
        .clone()
        .unwrap_or_else(|| terms.iter().take(4).cloned().collect::<Vec<_>>().join(" "));
    let tech = [
        "rust",
        "python",
        "github",
        "programming",
        "javascript",
        "api",
        "model",
        "linux",
        "software",
        "code",
        "llm",
        "ai",
        "database",
        "reqwest",
        "tokio",
    ]
    .iter()
    .any(|word| query_terms(&lower).iter().any(|t| t == word));
    let science = [
        "paper",
        "research",
        "arxiv",
        "retrieval",
        "quantum",
        "physics",
        "scientific",
        "rag",
        "llm",
    ]
    .iter()
    .any(|word| lower.contains(word));
    let n = limit.min(10).to_string();
    let mut jobs = vec![(
        "wikipedia".to_string(),
        Kind::Wikipedia,
        with_query(
            "https://en.wikipedia.org/w/api.php",
            &[
                ("action", "query"),
                ("list", "search"),
                ("srsearch", &q),
                ("srlimit", &n),
                ("format", "json"),
            ],
        ),
    )];
    if tech || lower.contains("repo") {
        let issues = lower.contains("issue") || lower.contains("bug");
        jobs.push((
            "github".into(),
            Kind::Github,
            with_query(
                if issues {
                    "https://api.github.com/search/issues"
                } else {
                    "https://api.github.com/search/repositories"
                },
                &[("q", &site_query), ("per_page", &n)],
            ),
        ));
        jobs.push((
            "hacker_news".into(),
            Kind::HackerNews,
            with_query(
                "https://hn.algolia.com/api/v1/search",
                &[
                    ("query", &site_query),
                    ("tags", "story"),
                    ("hitsPerPage", &n),
                ],
            ),
        ));
        jobs.push((
            "stack_exchange".into(),
            Kind::StackExchange,
            with_query(
                "https://api.stackexchange.com/2.3/search/advanced",
                &[
                    ("q", &q),
                    ("site", "stackoverflow"),
                    ("pagesize", &n),
                    ("sort", "relevance"),
                    ("filter", "withbody"),
                ],
            ),
        ));
    }
    if let Some(name) = query_terms(query).into_iter().find(|t| {
        matches!(
            t.as_str(),
            "reqwest"
                | "tokio"
                | "axum"
                | "serde"
                | "clap"
                | "rayon"
                | "sqlx"
                | "hyper"
                | "tower"
                | "actix-web"
        )
    }) {
        jobs.push((
            "crates.io".into(),
            Kind::Crates,
            format!("https://crates.io/api/v1/crates/{name}"),
        ));
    }
    if science {
        jobs.push((
            "arxiv".into(),
            Kind::Arxiv,
            with_query(
                "https://export.arxiv.org/api/query",
                &[("search_query", &format!("all:{q}")), ("max_results", &n)],
            ),
        ));
    }
    for source in crate::safe_sources::relevant_feed_candidates(query, 3) {
        jobs.push((source.title, Kind::Feed, source.url));
    }
    let mut sources = Vec::new();
    let mut notes = Vec::new();
    let mut tasks = stream::iter(jobs)
        .map(|(name, kind, url)| {
            let terms = &terms;
            async move {
                let work = async {
                    if matches!(kind, Kind::Feed | Kind::Arxiv) {
                        let doc = fetch_document(
                            &url,
                            "application/atom+xml,application/rss+xml,application/xml,text/xml",
                            clients,
                        )
                        .await?;
                        let hits = parse_feed(&doc.body, &url, 100)
                            .into_iter()
                            .filter(|s| {
                                crate::html::term_overlap(
                                    query,
                                    &format!("{} {}", s.title, s.summary),
                                ) >= if terms.len() > 2 { 2 } else { 1 }
                            })
                            .collect();
                        Ok::<Vec<CitationSource>, SearchError>(hits)
                    } else {
                        let value = fetch_json(&url, clients).await?;
                        Ok(parse_api(kind, &value))
                    }
                };
                (
                    name,
                    tokio::time::timeout(Duration::from_secs(4), work).await,
                )
            }
        })
        .buffer_unordered(FETCH_CONCURRENCY + 1);
    while let Some((name, result)) = tasks.next().await {
        match result {
            Ok(Ok(hits)) => {
                let hits: Vec<CitationSource> = hits;
                notes.push(format!("{name}: {} hits", hits.len()));
                sources.extend(hits);
            }
            Ok(Err(error)) => notes.push(format!("{name}: {}", error.public_message())),
            Err(_) => notes.push(format!("{name}: 4s deadline exceeded")),
        }
    }
    // API ordering is preserved within each source, tie breaking is stable.
    sources.sort_by(|a, b| a.url.cmp(&b.url));
    let sources = rerank_sources(query, sources, limit);
    if sources.is_empty() {
        return Err(SearchError::fatal(
            SearchFailureClass::NoResults,
            notes.join("; "),
        ));
    }
    Ok(build_query_result(query, sources, Some(notes.join("; "))))
}
fn with_query(base: &str, params: &[(&str, &str)]) -> String {
    let mut url = Url::parse(base).expect("static URL");
    url.query_pairs_mut().extend_pairs(params.iter().copied());
    url.to_string()
}
fn parse_api(kind: Kind, value: &Value) -> Vec<CitationSource> {
    let mut out = Vec::new();
    if matches!(kind, Kind::Crates) {
        let item = &value["crate"];
        if let Some(name) = item["name"].as_str() {
            let doc = format!("https://docs.rs/{name}/latest/{}/", name.replace('-', "_"));
            out.push(citation_source(
                format!("{name} latest documentation"),
                doc,
                format!(
                    "{}\nLatest stable version: {}\nUpdated: {}",
                    item["description"].as_str().unwrap_or(""),
                    item["max_stable_version"].as_str().unwrap_or("unknown"),
                    item["updated_at"].as_str().unwrap_or("unknown")
                ),
            ));
        }
        return out;
    }

    let items = match kind {
        Kind::Wikipedia => value["query"]["search"].as_array(),
        Kind::Github | Kind::StackExchange => value["items"].as_array(),
        Kind::HackerNews => value["hits"].as_array(),
        _ => None,
    };
    for item in items.into_iter().flatten() {
        let (title, url, text) = match kind {
            Kind::Wikipedia => {
                let title = item["title"].as_str().unwrap_or("");
                let mut url = Url::parse("https://en.wikipedia.org/wiki/").unwrap();
                url.path_segments_mut()
                    .unwrap()
                    .pop_if_empty()
                    .push(&title.replace(' ', "_"));
                (
                    title.to_string(),
                    url.to_string(),
                    visible_fragment(item["snippet"].as_str().unwrap_or("")),
                )
            }
            Kind::Github => (
                item["full_name"]
                    .as_str()
                    .or_else(|| item["title"].as_str())
                    .unwrap_or("")
                    .to_string(),
                item["html_url"].as_str().unwrap_or("").to_string(),
                format!(
                    "{}\nUpdated: {}",
                    item["description"]
                        .as_str()
                        .or_else(|| item["body"].as_str())
                        .unwrap_or(""),
                    item["updated_at"].as_str().unwrap_or("unknown")
                ),
            ),
            // Link to the discussion we actually searched. External stories are separate evidence.
            Kind::HackerNews => (
                item["title"].as_str().unwrap_or("").to_string(),
                format!(
                    "https://news.ycombinator.com/item?id={}",
                    item["objectID"].as_str().unwrap_or("")
                ),
                format!(
                    "Hacker News discussion; points: {}; posted: {}. Story URL: {}",
                    item["points"],
                    item["created_at"].as_str().unwrap_or(""),
                    item["url"].as_str().unwrap_or("")
                ),
            ),
            Kind::StackExchange => (
                visible_fragment(item["title"].as_str().unwrap_or("")),
                item["link"].as_str().unwrap_or("").to_string(),
                format!(
                    "Question (answers not fetched): {}",
                    visible_fragment(item["body"].as_str().unwrap_or(""))
                ),
            ),
            _ => continue,
        };
        if title.is_empty() {
            continue;
        }
        if let Ok(url) = crate::url_utils::canonicalize_url(&url) {
            out.push(citation_source(title, url, truncate_chars(&text, 1600)));
        }
    }
    out
}
