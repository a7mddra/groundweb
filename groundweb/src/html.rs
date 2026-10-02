// Copyright 2026 a7mddra
// SPDX-License-Identifier: MIT

//! Mojeek DOM parsing, shared normalization, relevance and host diversity.

use regex::Regex;
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use url::Url;

use super::constants::{MAX_FETCH_CHARS, MAX_SUMMARY_WORDS};
use super::favicon::citation_source;
use super::types::{CitationSource, WebSearchResult};
use super::url_utils::{canonicalize_url, domain_from_url};

lazy_static::lazy_static! {
    static ref TITLE_RE: Regex = Regex::new(r#"(?is)<title[^>]*>(.*?)</title>"#).expect("valid title regex");
    static ref SKIP_BLOCK_RE: Regex = Regex::new(
        r#"(?is)<script[^>]*>.*?</script>|<style[^>]*>.*?</style>|<head[^>]*>.*?</head>|<noscript[^>]*>.*?</noscript>|<svg[^>]*>.*?</svg>|<math[^>]*>.*?</math>|<nav[^>]*>.*?</nav>|<footer[^>]*>.*?</footer>"#
    ).expect("valid skip block regex");
    static ref TAG_RE: Regex = Regex::new(r#"(?is)<[^>]+>"#).expect("valid tag regex");
    static ref WS_RE: Regex = Regex::new(r#"\s+"#).expect("valid whitespace regex");

}

fn clean_html_fragment(input: &str) -> String {
    let mut s = input.replace("&nbsp;", " ");
    s = s.replace("&amp;", "&");
    s = s.replace("&quot;", "\"");
    s = s.replace("&#39;", "'");
    s = s.replace("&lt;", "<");
    s = s.replace("&gt;", ">");

    let without_tags = TAG_RE.replace_all(&s, " ");
    let collapsed = WS_RE.replace_all(&without_tags, " ");
    collapsed.trim().to_string()
}

pub(crate) fn looks_like_mojeek_block_page(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    (lower.contains("403 - forbidden") && lower.contains("automated queries"))
        || crate::extract::looks_blocked(html)
}

pub(crate) fn clean_page_text(raw_html: &str) -> String {
    let stripped = SKIP_BLOCK_RE.replace_all(raw_html, " ");
    let no_tags = TAG_RE.replace_all(&stripped, " ");
    let decoded = clean_html_fragment(&no_tags);
    crate::extract::truncate_chars(&decoded, MAX_FETCH_CHARS)
}

pub(crate) fn compact_summary(text: &str, max_words: usize) -> String {
    if text.trim().is_empty() {
        return String::new();
    }
    let words: Vec<&str> = text.split_whitespace().take(max_words + 1).collect();
    if words.len() <= max_words {
        words.join(" ")
    } else {
        format!("{}...", words[..max_words].join(" "))
    }
}

pub(crate) fn extract_title(raw_html: &str, fallback_url: &str) -> String {
    if let Some(cap) = TITLE_RE.captures(raw_html) {
        if let Some(m) = cap.get(1) {
            let title = clean_html_fragment(m.as_str());
            if !title.is_empty() {
                return title;
            }
        }
    }
    Url::parse(fallback_url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_string()))
        .unwrap_or_else(|| fallback_url.to_string())
}

pub(crate) fn parse_mojeek_results(html: &str, max_results: usize) -> Vec<CitationSource> {
    let doc = scraper::Html::parse_document(html);
    let blocks =
        scraper::Selector::parse("li.results-standard, li.result, li.serp-result").unwrap();
    let links = scraper::Selector::parse("h2 a[href], h3 a[href]").unwrap();
    let snippets = scraper::Selector::parse("p.s, p.snippet, p.desc, p.description").unwrap();
    let mut sources = Vec::new();
    let mut seen = HashSet::new();
    for block in doc.select(&blocks) {
        let Some(link) = block.select(&links).next() else {
            continue;
        };
        let Some(raw) = link.value().attr("href") else {
            continue;
        };
        let Ok(url) = canonicalize_url(raw) else {
            continue;
        };
        if domain_from_url(&url).is_some_and(|d| d == "mojeek.com" || d.ends_with(".mojeek.com"))
            || !seen.insert(url.clone())
        {
            continue;
        }
        let title = link.text().collect::<Vec<_>>().join(" ");
        if title.trim().is_empty() {
            continue;
        }
        let snippet = block
            .select(&snippets)
            .next()
            .map(|n| n.text().collect::<Vec<_>>().join(" "))
            .unwrap_or_default();
        sources.push(citation_source(
            title.trim().to_string(),
            url,
            compact_summary(&snippet, MAX_SUMMARY_WORDS),
        ));
        if sources.len() >= max_results {
            break;
        }
    }
    sources
}

pub(crate) fn query_terms(query: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    query
        .split(|c: char| !c.is_alphanumeric() && !matches!(c, '-' | '_'))
        .map(|s| s.to_lowercase())
        .filter(|s| {
            s.chars().count() >= 2
                && !matches!(
                    s.as_str(),
                    "the"
                        | "and"
                        | "for"
                        | "with"
                        | "from"
                        | "this"
                        | "that"
                        | "what"
                        | "which"
                        | "how"
                        | "does"
                        | "are"
                        | "can"
                        | "you"
                        | "please"
                        | "search"
                        | "find"
                        | "compare"
                        | "explain"
                        | "summarize"
                        | "using"
                        | "use"
                        | "about"
                        | "give"
                        | "me"
                        | "of"
                        | "to"
                        | "in"
                        | "on"
                        | "is"
                        | "it"
                        | "as"
                        | "an"
                        | "be"
                        | "https"
                        | "http"
                        | "www"
                        | "com"
                )
        })
        .filter(|s| seen.insert(s.clone()))
        .collect()
}

pub(crate) fn term_overlap(query: &str, text: &str) -> usize {
    let text = text.to_lowercase();
    let tokens: HashSet<_> = text
        .split(|c: char| !c.is_alphanumeric() && !matches!(c, '-' | '_'))
        .collect();
    query_terms(query)
        .iter()
        .filter(|term| {
            if term.is_ascii() {
                tokens.contains(term.as_str())
            } else {
                text.contains(term.as_str())
            }
        })
        .count()
}

fn source_score(query: &str, source: &CitationSource) -> i32 {
    let title = term_overlap(query, &source.title) as i32;
    let snippet = term_overlap(query, &source.summary) as i32;
    let path = term_overlap(query, &source.url) as i32;
    let trusted =
        domain_from_url(&source.url).is_some_and(|d| crate::safe_sources::is_safe_domain(&d));
    let mut freshness = 0;
    if let Ok(url) = Url::parse(&source.url) {
        if url.host_str() == Some("docs.rs") {
            let parts: Vec<_> = url.path_segments().into_iter().flatten().collect();
            if parts.get(1) == Some(&"latest") {
                freshness += 18;
            } else if let Some(version) = parts
                .get(1)
                .filter(|v| v.chars().next().is_some_and(|c| c.is_ascii_digit()))
            {
                if !query.contains(version) {
                    freshness -= 20;
                }
            }
        }
        if crate::html::query_terms(query)
            .iter()
            .any(|t| matches!(t.as_str(), "latest" | "current" | "today"))
        {
            if url.path().contains("/nightly/") {
                freshness -= 15;
            }
            if url.host_str() == Some("blog.rust-lang.org") {
                freshness += 12;
            }
        }
    }
    title * 9
        + freshness
        + snippet * 3
        + path * 2
        + if trusted { 2 } else { 0 }
        + if source.summary.is_empty() { -3 } else { 2 }
}

pub(crate) fn rerank_sources(
    query: &str,
    sources: Vec<CitationSource>,
    max_results: usize,
) -> Vec<CitationSource> {
    let mut merged: HashMap<String, (usize, CitationSource, i32)> = HashMap::new();
    // Repeated discovery hits add a small agreement boost.
    for (index, mut source) in sources.into_iter().enumerate() {
        let Ok(url) = canonicalize_url(&source.url) else {
            continue;
        };
        source.url = url.clone();
        if let Some((_, existing, votes)) = merged.get_mut(&url) {
            *votes += 1;
            if source.summary.len() > existing.summary.len() {
                existing.summary = source.summary;
            }
        } else {
            merged.insert(url, (index, source, 0));
        }
    }
    let site = query
        .split_whitespace()
        .find_map(|s| s.strip_prefix("site:"))
        .map(|s| s.trim_end_matches('/').to_lowercase());
    let mut indexed: Vec<_> = merged
        .into_values()
        .filter(|(_, source, _)| {
            site.as_ref().is_none_or(|target| {
                domain_from_url(&source.url)
                    .is_some_and(|d| d == *target || d.ends_with(&format!(".{target}")))
            })
        })
        .map(|(idx, source, votes)| {
            let score = source_score(query, &source) + votes * 4;
            (idx, source, score)
        })
        .collect();
    indexed.sort_by_key(|(idx, _, score)| (Reverse(*score), *idx));
    let mut counts = HashMap::<String, usize>::new();
    let mut chosen = Vec::new();
    let mut overflow = Vec::new();
    for (_, source, _) in indexed {
        let host = domain_from_url(&source.url).unwrap_or_default();
        let count = counts.entry(host).or_default();
        if *count < 2 {
            *count += 1;
            chosen.push(source);
        } else {
            overflow.push(source);
        }
    }
    chosen.extend(overflow);
    chosen.truncate(max_results);
    chosen
}

fn build_query_context(query: &str, sources: &[CitationSource]) -> String {
    let mut context = format!("[Search results for \"{}\"]\n", query.trim());
    for source in sources {
        context.push_str(&format!(
            "- {} — {}\n  {}\n",
            source.title,
            source.url,
            if source.summary.is_empty() {
                "(No snippet available)"
            } else {
                &source.summary
            }
        ));
    }
    context.trim().to_string()
}

pub fn build_query_result(
    query: &str,
    sources: Vec<CitationSource>,
    message: Option<String>,
) -> WebSearchResult {
    WebSearchResult {
        mode: "query".to_string(),
        query: Some(query.trim().to_string()),
        requested_url: None,
        context_markdown: build_query_context(query, &sources),
        sources,
        success: true,
        message,
    }
}
