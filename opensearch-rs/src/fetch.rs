//! Bounded HTTP retrieval for user URLs and previously discovered sources.
use crate::{
    constants::{
        MAX_FETCH_BYTES, MAX_FETCH_CHARS, MAX_REDIRECTS, MAX_SUMMARY_WORDS, PAGE_TIMEOUT_SECS,
    },
    extract::{looks_blocked, readable_html, truncate_chars, visible_fragment},
    favicon::{citation_source, hydrate_favicons_for_sources},
    html::compact_summary,
    transport::{read_capped_response_body, send_with_transport, TransportClients},
    types::{CitationSource, SearchError, SearchFailureClass, WebSearchResult},
    url_utils::{canonicalize_url, ensure_public_target},
};
use reqwest::{header, StatusCode};
use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};
use url::Url;

pub(crate) struct HttpDocument {
    pub url: String,
    pub content_type: String,
    pub body: String,
}
#[derive(Clone)]
pub(crate) struct Page {
    pub source: CitationSource,
    pub text: String,
    pub via: String,
}
lazy_static::lazy_static! {
    static ref PAGE_CACHE: Mutex<HashMap<String, (Instant, Page)>> = Mutex::new(HashMap::new());
    static ref HOST_COOLDOWNS: Mutex<HashMap<String, Instant>> = Mutex::new(HashMap::new());
}

pub(crate) async fn fetch_document(
    url: &str,
    accept: &str,
    clients: &TransportClients,
) -> Result<HttpDocument, SearchError> {
    let canonical = canonicalize_url(url)?;
    let mut current = Url::parse(&canonical).expect("canonical URL");
    for hop in 0..=MAX_REDIRECTS {
        ensure_public_target(&current).await?;
        let host = current.host_str().unwrap_or_default().to_string();
        if HOST_COOLDOWNS
            .lock()
            .unwrap()
            .get(&host)
            .is_some_and(|t| *t > Instant::now())
        {
            return Err(SearchError::fatal(
                SearchFailureClass::HttpStatus,
                "Host is cooling down after a rate limit",
            ));
        }
        let (response, _) = send_with_transport(clients, |client| {
            client.get(current.clone()).header(header::ACCEPT, accept)
        })
        .await?;
        if response.status().is_redirection() {
            let location = response
                .headers()
                .get(header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| {
                    SearchError::fatal(SearchFailureClass::HttpStatus, "Redirect missing Location")
                })?;
            current = current
                .join(location)
                .map_err(|e| SearchError::fatal(SearchFailureClass::InvalidUrl, e.to_string()))?;
            if hop == MAX_REDIRECTS {
                break;
            }
            continue;
        }
        if !response.status().is_success() {
            let status = response.status();
            if status == StatusCode::TOO_MANY_REQUESTS
                || (status == StatusCode::FORBIDDEN
                    && response
                        .headers()
                        .get("x-ratelimit-remaining")
                        .is_some_and(|v| v == "0"))
            {
                let seconds = response
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(60)
                    .clamp(1, 3600);
                let mut cooldowns = HOST_COOLDOWNS.lock().unwrap();
                cooldowns.retain(|_, t| *t > Instant::now());
                if cooldowns.len() < 64 {
                    cooldowns.insert(host, Instant::now() + Duration::from_secs(seconds));
                }
            }
            return Err(SearchError::fatal(
                if status == StatusCode::FORBIDDEN {
                    SearchFailureClass::Challenge
                } else {
                    SearchFailureClass::HttpStatus
                },
                format!("HTTP {} from {}", status.as_u16(), current),
            ));
        }
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("text/plain")
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_lowercase();
        if !(content_type.starts_with("text/")
            || content_type.contains("json")
            || content_type.contains("xml")
            || content_type == "application/vnd.github.raw+json")
        {
            return Err(SearchError::fatal(
                SearchFailureClass::Parse,
                format!(
                    "Unsupported content type {content_type}; binary/PDF extraction is unavailable"
                ),
            ));
        }
        let body = read_capped_response_body(response, MAX_FETCH_BYTES).await?;
        return Ok(HttpDocument {
            url: current.to_string(),
            content_type,
            body,
        });
    }
    Err(SearchError::fatal(
        SearchFailureClass::HttpStatus,
        "Too many redirects",
    ))
}

pub(crate) async fn fetch_json(
    url: &str,
    clients: &TransportClients,
) -> Result<serde_json::Value, SearchError> {
    let doc = fetch_document(url, "application/json", clients).await?;
    serde_json::from_str(&doc.body)
        .map_err(|e| SearchError::fatal(SearchFailureClass::Parse, format!("Invalid JSON: {e}")))
}

pub(crate) async fn fetch_page(url: &str, clients: &TransportClients) -> Result<Page, SearchError> {
    let canonical = canonicalize_url(url)?;
    if let Some((time, page)) = PAGE_CACHE.lock().unwrap().get(&canonical) {
        if time.elapsed() < Duration::from_secs(300) {
            return Ok(page.clone());
        }
    }
    let page = tokio::time::timeout(
        Duration::from_secs(PAGE_TIMEOUT_SECS),
        fetch_page_once(&canonical, clients),
    )
    .await
    .map_err(|_| {
        SearchError::fatal(
            SearchFailureClass::ReadTimeout,
            "Page retrieval deadline exceeded",
        )
    })??;
    let mut cache = PAGE_CACHE.lock().unwrap();
    cache.retain(|_, (time, _)| time.elapsed() < Duration::from_secs(300));
    if cache.len() >= 64 {
        if let Some(oldest) = cache
            .iter()
            .min_by_key(|(_, (time, _))| *time)
            .map(|(key, _)| key.clone())
        {
            cache.remove(&oldest);
        }
    }
    cache.insert(canonical, (Instant::now(), page.clone()));
    Ok(page)
}

async fn fetch_page_once(url: &str, clients: &TransportClients) -> Result<Page, SearchError> {
    let parsed = Url::parse(url).expect("canonical URL");
    // Validate the original even when a site adapter reads a public API.
    ensure_public_target(&parsed).await?;
    let host = parsed
        .host_str()
        .unwrap_or_default()
        .trim_start_matches("www.");
    let adapter = match host {
        "github.com" => Some(fetch_github(&parsed, clients).await),
        host if host.ends_with(".wikipedia.org") && parsed.path().starts_with("/wiki/") => {
            Some(fetch_wikipedia(&parsed, clients).await)
        }
        "x.com" | "twitter.com" | "mobile.twitter.com" => Some(fetch_tweet(&parsed, clients).await),
        _ => None,
    };
    let adapter_error = match adapter {
        Some(Ok(page)) => return Ok(page),
        Some(Err(error)) => Some(error.public_message()),
        None => None,
    };
    let doc = fetch_document(url,"text/html,application/xhtml+xml,text/plain,application/json,application/xml,application/rss+xml,application/atom+xml",clients).await?;
    if looks_blocked(&doc.body) {
        return Err(SearchError::fatal(
            SearchFailureClass::Challenge,
            "Site returned an access challenge",
        ));
    }
    let (title, text) = if doc.content_type.contains("html") {
        readable_html(&doc.body, &doc.url)
    } else if doc.content_type.contains("xml") {
        let sources = parse_feed(&doc.body, &doc.url, 30);
        if sources.is_empty() {
            return Err(SearchError::fatal(
                SearchFailureClass::Parse,
                "No readable feed entries",
            ));
        }
        (
            doc.url.clone(),
            truncate_chars(
                &sources
                    .iter()
                    .map(|s| format!("{}\n{}\n{}", s.title, s.url, s.summary))
                    .collect::<Vec<_>>()
                    .join("\n\n"),
                MAX_FETCH_CHARS,
            ),
        )
    } else {
        // Keep Markdown, source code and JSON as text, never strip `<...>`.
        (
            doc.url.clone(),
            truncate_chars(doc.body.trim(), MAX_FETCH_CHARS),
        )
    };
    if text.trim().chars().count() < 80
        || (matches!(host, "x.com" | "twitter.com" | "mobile.twitter.com")
            && adapter_error.is_some())
    {
        return Err(SearchError::fatal(
            SearchFailureClass::Parse,
            adapter_error.unwrap_or_else(|| {
                "No useful static content; page may require JavaScript/login".into()
            }),
        ));
    }
    Ok(Page {
        source: citation_source(title, doc.url, compact_summary(&text, MAX_SUMMARY_WORDS)),
        text,
        via: "direct HTTP / native extraction".into(),
    })
}

fn make_page(title: String, url: &str, text: String, via: &str) -> Page {
    let text = truncate_chars(&text, MAX_FETCH_CHARS);
    Page {
        source: citation_source(
            title,
            url.to_string(),
            compact_summary(&text, MAX_SUMMARY_WORDS),
        ),
        text,
        via: via.to_string(),
    }
}

async fn fetch_wikipedia(url: &Url, clients: &TransportClients) -> Result<Page, SearchError> {
    let title = percent_encoding::percent_decode_str(url.path().trim_start_matches("/wiki/"))
        .decode_utf8_lossy()
        .replace('_', " ");
    let mut api = url
        .join("/w/api.php")
        .map_err(|e| SearchError::fatal(SearchFailureClass::InvalidUrl, e.to_string()))?;
    api.query_pairs_mut().extend_pairs([
        ("action", "query"),
        ("prop", "extracts"),
        ("explaintext", "1"),
        ("titles", title.as_str()),
        ("format", "json"),
        ("exchars", "20000"),
        ("redirects", "1"),
    ]);
    let data = fetch_json(api.as_str(), clients).await?;
    let pages = data["query"]["pages"].as_object().ok_or_else(|| {
        SearchError::fatal(SearchFailureClass::Parse, "Wikipedia returned no article")
    })?;
    for value in pages.values() {
        if let Some(text) = value["extract"].as_str().filter(|s| !s.trim().is_empty()) {
            return Ok(make_page(
                value["title"].as_str().unwrap_or(&title).to_string(),
                url.as_str(),
                text.to_string(),
                "Wikipedia public article-text API",
            ));
        }
    }
    Err(SearchError::fatal(
        SearchFailureClass::NoResults,
        "Wikipedia article text unavailable",
    ))
}

async fn fetch_github(url: &Url, clients: &TransportClients) -> Result<Page, SearchError> {
    let segments: Vec<_> = url
        .path_segments()
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .collect();
    if segments.len() < 2 {
        return Err(SearchError::fatal(
            SearchFailureClass::Parse,
            "GitHub URL is not a repository",
        ));
    }
    let owner = segments[0];
    let repo = segments[1].trim_end_matches(".git");
    let api = format!("https://api.github.com/repos/{owner}/{repo}");
    let original = format!(
        "https://github.com/{owner}/{repo}{}",
        if segments.len() == 2 {
            String::new()
        } else {
            format!("/{}", segments[2..].join("/"))
        }
    );
    if segments.len() == 2 {
        let readme_endpoint = format!("{api}/readme");
        let (metadata, readme) = tokio::join!(
            fetch_json(&api, clients),
            fetch_document(&readme_endpoint, "application/vnd.github.raw+json", clients)
        );
        let mut text = String::new();
        match metadata {
            Ok(data) => {
                text=format!("Repository: {owner}/{repo}\nDescription: {}\nDefault branch: {}\nLanguage: {}\nStars: {}\nUpdated: {}\nLicense: {}\n\n",data["description"].as_str().unwrap_or(""),data["default_branch"].as_str().unwrap_or(""),data["language"].as_str().unwrap_or(""),data["stargazers_count"],data["updated_at"].as_str().unwrap_or(""),data["license"]["spdx_id"].as_str().unwrap_or("unknown"));
            }
            Err(error) => text.push_str(&format!(
                "[Repository metadata unavailable: {}]\n",
                error.public_message()
            )),
        }
        let readme = match readme {
            Ok(doc) => doc,
            Err(_) => {
                fetch_document(
                    &format!("https://raw.githubusercontent.com/{owner}/{repo}/HEAD/README.md"),
                    "text/plain",
                    clients,
                )
                .await?
            }
        };
        text.push_str("README:\n");
        text.push_str(&readme.body);
        return Ok(make_page(
            format!("{owner}/{repo}"),
            &original,
            text,
            &format!(
                "GitHub repository metadata (when available); README from {}",
                readme.url
            ),
        ));
    }
    if segments[2] == "blob" && segments.len() >= 5 {
        let raw = format!(
            "https://raw.githubusercontent.com/{owner}/{repo}/{}",
            segments[3..].join("/")
        );
        let doc = fetch_document(&raw, "text/plain", clients).await?;
        return Ok(make_page(
            format!("{owner}/{repo}/{}", segments[4..].join("/")),
            &original,
            doc.body,
            "GitHub raw file",
        ));
    }
    let endpoint = match segments[2] {
        "issues" | "pull"
            if segments.len() == 4 && segments[3].chars().all(|c| c.is_ascii_digit()) =>
        {
            format!("{api}/issues/{}", segments[3])
        }
        "releases" if segments.len() == 3 => format!("{api}/releases?per_page=5"),
        "releases" if segments.get(3) == Some(&"latest") => format!("{api}/releases/latest"),
        "releases" if segments.get(3) == Some(&"tag") && segments.len() >= 5 => {
            format!("{api}/releases/tags/{}", segments[4..].join("/"))
        }
        _ => {
            return Err(SearchError::fatal(
                SearchFailureClass::Parse,
                "No specialized adapter for this GitHub path; trying HTML",
            ))
        }
    };
    let data = fetch_json(&endpoint, clients).await?;
    let records = data
        .as_array()
        .cloned()
        .unwrap_or_else(|| vec![data.clone()]);
    let mut text = records
        .iter()
        .map(|v| {
            format!(
                "Title: {}\nURL: {}\nState: {}\nUpdated/published: {}\n{}",
                v["title"]
                    .as_str()
                    .or_else(|| v["name"].as_str())
                    .unwrap_or(""),
                v["html_url"].as_str().unwrap_or(&original),
                v["state"].as_str().unwrap_or(""),
                v["published_at"]
                    .as_str()
                    .or_else(|| v["updated_at"].as_str())
                    .unwrap_or(""),
                v["body"].as_str().unwrap_or("")
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    if matches!(segments[2], "issues" | "pull") && data["comments"].as_u64().unwrap_or(0) > 0 {
        match fetch_json(
            &format!("{api}/issues/{}/comments?per_page=10", segments[3]),
            clients,
        )
        .await
        {
            Ok(comments) => {
                for c in comments.as_array().into_iter().flatten() {
                    text.push_str(&format!(
                        "\n\nComment by {} ({}):\n{}",
                        c["user"]["login"].as_str().unwrap_or(""),
                        c["created_at"].as_str().unwrap_or(""),
                        c["body"].as_str().unwrap_or("")
                    ));
                }
            }
            Err(error) => text.push_str(&format!(
                "\n[Issue comments unavailable: {}]",
                error.public_message()
            )),
        }
    }
    if text.trim().len() < 80 {
        return Err(SearchError::fatal(
            SearchFailureClass::NoResults,
            "No readable GitHub records",
        ));
    }
    Ok(make_page(
        format!("{owner}/{repo}: {}", segments[2..].join("/")),
        &original,
        text,
        "GitHub public REST API (comments capped at 10)",
    ))
}

async fn fetch_tweet(url: &Url, clients: &TransportClients) -> Result<Page, SearchError> {
    let segments: Vec<_> = url.path_segments().into_iter().flatten().collect();
    let pos = segments
        .iter()
        .position(|s| *s == "status")
        .ok_or_else(|| {
            SearchError::fatal(
                SearchFailureClass::Parse,
                "X profile/timeline requires login/JavaScript; paste a public post URL",
            )
        })?;
    let id = segments
        .get(pos + 1)
        .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
        .ok_or_else(|| SearchError::fatal(SearchFailureClass::InvalidUrl, "Invalid post ID"))?;
    let original = format!(
        "https://x.com/{}/status/{id}",
        segments.first().unwrap_or(&"i")
    );
    let mut endpoint = Url::parse("https://publish.twitter.com/oembed").unwrap();
    endpoint
        .query_pairs_mut()
        .append_pair("url", &original)
        .append_pair("omit_script", "true");
    if let Ok(data) = fetch_json(endpoint.as_str(), clients).await {
        if let Some(html) = data["html"].as_str() {
            let doc = scraper::Html::parse_fragment(html);
            let selector = scraper::Selector::parse("blockquote p").unwrap();
            let text = doc
                .select(&selector)
                .map(|n| visible_fragment(&n.html()))
                .collect::<Vec<_>>()
                .join("\n");
            if !text.is_empty() {
                return Ok(make_page(
                    format!(
                        "Post by {}",
                        data["author_name"].as_str().unwrap_or("unknown")
                    ),
                    &original,
                    text,
                    "X public oEmbed (post text only; no thread/timeline)",
                ));
            }
        }
    }
    // Public widget JSON is best-effort and undocumented. No login or token secret.
    let data = fetch_json(
        &format!("https://cdn.syndication.twimg.com/tweet-result?id={id}&lang=en&token=0"),
        clients,
    )
    .await?;
    let text = data["text"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            SearchError::fatal(
                SearchFailureClass::NoResults,
                "Public post is unavailable through X widgets",
            )
        })?;
    Ok(make_page(
        format!(
            "Post by {}",
            data["user"]["name"].as_str().unwrap_or("unknown")
        ),
        &original,
        format!(
            "Published: {}\n{text}",
            data["created_at"].as_str().unwrap_or("unknown")
        ),
        "X public syndication widget (may change; post text only)",
    ))
}

pub(crate) fn parse_feed(xml: &str, base: &str, limit: usize) -> Vec<CitationSource> {
    let Ok(doc) = roxmltree::Document::parse(xml) else {
        return Vec::new();
    };
    let mut sources = Vec::new();
    for item in doc
        .descendants()
        .filter(|n| n.is_element() && matches!(n.tag_name().name(), "item" | "entry"))
    {
        let field = |name: &str| {
            item.children()
                .find(|n| n.is_element() && n.tag_name().name() == name)
                .and_then(|n| n.text())
                .unwrap_or("")
                .trim()
                .to_string()
        };
        let link = item
            .children()
            .filter(|n| n.is_element() && n.tag_name().name() == "link")
            .find_map(|n| {
                if n.attribute("rel").is_none_or(|r| r == "alternate") {
                    n.attribute("href").or_else(|| n.text()).map(str::trim)
                } else {
                    None
                }
            });
        let Some(url) = link
            .and_then(|s| Url::parse(base).ok()?.join(s).ok())
            .and_then(|u| canonicalize_url(u.as_str()).ok())
        else {
            continue;
        };
        let title = visible_fragment(&field("title"));
        if title.is_empty() {
            continue;
        }
        let mut text = field("description");
        if text.is_empty() {
            text = field("summary");
        }
        if text.is_empty() {
            text = field("content");
        }
        let mut date = field("pubDate");
        if date.is_empty() {
            date = field("published");
        }
        if date.is_empty() {
            date = field("updated");
        }
        sources.push(citation_source(
            title,
            url,
            truncate_chars(
                &format!(
                    "{}{}",
                    if date.is_empty() {
                        String::new()
                    } else {
                        format!("Published: {date}\n")
                    },
                    visible_fragment(&text)
                ),
                1600,
            ),
        ));
        if sources.len() >= limit {
            break;
        }
    }
    sources
}

pub fn collect_allowed_sources(result: &WebSearchResult) -> HashMap<String, CitationSource> {
    result
        .sources
        .iter()
        .filter_map(|s| canonicalize_url(&s.url).ok().map(|u| (u, s.clone())))
        .collect()
}

pub async fn fetch_url_from_allowed(
    url: &str,
    allowed: &HashMap<String, CitationSource>,
) -> Result<WebSearchResult, String> {
    fetch_url_from_allowed_with_progress(url, allowed, |_| {}).await
}
pub async fn fetch_url_from_allowed_with_progress<F: FnMut(String) + Send>(
    url: &str,
    allowed: &HashMap<String, CitationSource>,
    mut progress: F,
) -> Result<WebSearchResult, String> {
    let canonical = canonicalize_url(url).map_err(|e| e.public_message())?;
    let source = allowed.get(&canonical).ok_or_else(|| {
        "Blocked URL fetch: URL must come from a previous search result in this turn".to_string()
    })?;
    let clients = TransportClients::build().map_err(|e| e.public_message())?;
    progress(format!("Reading {canonical}"));
    let (mut sources, context, message) = match fetch_page(&canonical, &clients).await {
        Ok(page) => (
            vec![page.source],
            format!("[Retrieved via {}]\n{}", page.via, page.text),
            None,
        ),
        Err(error) => (
            vec![source.clone()],
            format!(
                "[Full page unavailable; prior search snippet only]\n{}",
                source.summary
            ),
            Some(error.public_message()),
        ),
    };
    hydrate_favicons_for_sources(&mut sources).await;
    Ok(WebSearchResult {
        mode: "url".into(),
        query: None,
        requested_url: Some(canonical),
        context_markdown: context,
        sources,
        success: true,
        message,
    })
}
