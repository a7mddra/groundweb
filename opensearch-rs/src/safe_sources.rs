// Copyright 2026 a7mddra
// SPDX-License-Identifier: Apache-2.0

//! Trusted-source layer: a curated `safe_sources.json` catalog plus the two
//! helpers every branch (and the `execute` fallback) uses.
//!
//! - `local_safe_source_candidates`: keyless, network-free candidates ranked
//!   by query-term overlap — the fallback when Mojeek is blocked.
//! - `filter_suggested_urls_to_safe_sources`: keeps only safe domains out of
//!   LLM-suggested URLs (see `suggester`, now OpenRouter-backed).

use serde::Deserialize;
use std::cmp::Reverse;
use std::collections::HashSet;

use super::favicon::citation_source;
use super::types::CitationSource;
use super::url_utils::{canonicalize_url, domain_from_url, normalize_domain};

#[derive(Debug, Clone, Deserialize)]
struct SafeSourcesCatalog {
    safe_sources: Vec<SafeSourcesCategory>,
}

#[derive(Debug, Clone, Deserialize)]
struct SafeSourcesCategory {
    category: String,
    sites: Vec<SafeSourceSite>,
}

#[derive(Debug, Clone, Deserialize)]
struct SafeSourceSite {
    name: String,
    domain: String,
    #[serde(default)]
    rss: Option<String>,
}

lazy_static::lazy_static! {
    static ref SAFE_SOURCES_DATA: Vec<SafeSourcesCategory> = {
        let json_content = include_str!("./assets/safe_sources.json");
        serde_json::from_str::<SafeSourcesCatalog>(json_content)
            .map(|catalog| catalog.safe_sources)
            .unwrap_or_default()
    };
    static ref SAFE_DOMAINS: HashSet<String> = {
        let mut out = HashSet::new();
        for category in SAFE_SOURCES_DATA.iter() {
            for site in &category.sites {
                out.insert(normalize_domain(&site.domain));
            }
        }
        out
    };
}

pub(crate) fn is_safe_domain(domain: &str) -> bool {
    let normalized = normalize_domain(domain)
        .trim_start_matches("www.")
        .to_string();
    SAFE_DOMAINS
        .iter()
        .any(|allowed| normalized == *allowed || normalized.ends_with(&format!(".{}", allowed)))
}

fn make_safe_source_url(site: &SafeSourceSite) -> Option<String> {
    let preferred = site
        .rss
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(|v| v.to_string())
        .unwrap_or_else(|| format!("https://{}/", site.domain.trim()));

    canonicalize_url(&preferred).ok()
}

fn query_terms(query: &str) -> HashSet<String> {
    query
        .split(|c: char| !c.is_alphanumeric())
        .map(|part| part.trim().to_ascii_lowercase())
        .filter(|part| part.len() >= 2)
        .collect()
}

fn safe_source_relevance_score(query: &str, category: &str, site: &SafeSourceSite) -> i32 {
    let terms = query_terms(query);
    if terms.is_empty() {
        return 0;
    }

    let haystack = format!(
        "{} {} {}",
        category.to_ascii_lowercase(),
        site.name.to_ascii_lowercase(),
        site.domain.to_ascii_lowercase()
    );

    terms
        .iter()
        .filter(|term| haystack.contains(term.as_str()))
        .count() as i32
}

pub fn local_safe_source_candidates(
    query: &str,
    attempted_domains: &HashSet<String>,
    max_candidates: usize,
) -> Vec<CitationSource> {
    if max_candidates == 0 {
        return Vec::new();
    }

    let mut candidates = Vec::<(usize, i32, String, SafeSourceSite)>::new();
    let mut seen_domains = HashSet::<String>::new();
    let mut ordinal = 0usize;

    for category in SAFE_SOURCES_DATA.iter() {
        let category_name = category.category.to_ascii_lowercase();
        for site in &category.sites {
            let domain = normalize_domain(&site.domain)
                .trim_start_matches("www.")
                .to_string();

            if attempted_domains.contains(&domain) || !seen_domains.insert(domain.clone()) {
                continue;
            }

            let score = safe_source_relevance_score(query, &category_name, site);
            candidates.push((ordinal, score, category_name.clone(), site.clone()));
            ordinal += 1;
        }
    }

    candidates.sort_by_key(|(idx, score, _, _)| (Reverse(*score), *idx));

    let mut out = Vec::<CitationSource>::new();
    for (_, _, category, site) in candidates.into_iter().take(max_candidates) {
        let Some(url) = make_safe_source_url(&site) else {
            continue;
        };
        out.push(citation_source(
            site.name,
            url,
            format!("Trusted {} source candidate.", category),
        ));
    }

    out
}

pub(crate) fn relevant_catalog_candidates(query: &str, limit: usize) -> Vec<CitationSource> {
    let mut sites: Vec<_> = SAFE_SOURCES_DATA
        .iter()
        .flat_map(|category| {
            category.sites.iter().filter_map(|site| {
                let score = safe_source_relevance_score(query, &category.category, site);
                if score <= 0 {
                    return None;
                }
                Some((
                    score,
                    citation_source(
                        site.name.clone(),
                        make_safe_source_url(site)?,
                        "Catalog seed; not yet fetched".into(),
                    ),
                ))
            })
        })
        .collect();
    sites.sort_by_key(|(score, _)| Reverse(*score));
    sites
        .into_iter()
        .take(limit)
        .map(|(_, source)| source)
        .collect()
}

/// Pick only relevant feeds; a catalog homepage is never evidence of a fact.
pub(crate) fn relevant_feed_candidates(query: &str, limit: usize) -> Vec<CitationSource> {
    let terms = query_terms(query);
    let lower = query.to_lowercase();
    let mut feeds = Vec::new();
    for category in SAFE_SOURCES_DATA.iter() {
        for site in &category.sites {
            let Some(rss) = site.rss.as_deref() else {
                continue;
            };
            let mut score = safe_source_relevance_score(query, &category.category, site);
            let keywords = match category.category.as_str() {
                "official_technology" => "rust software programming release linux api",
                "reported_news" => "news latest today world politics technology ai",
                "science_reporting" | "official_science_health" => {
                    "science nasa space research discovery health"
                }
                "community_reference" => "rust programming technology ai llm software",
                _ => "",
            };
            score += terms
                .iter()
                .filter(|t| keywords.split_whitespace().any(|k| k == t.as_str()))
                .count() as i32;
            // Prefer a named source, e.g. Rust, over unrelated technology feeds.
            if lower.contains(&site.name.to_lowercase()) {
                score += 5;
            }
            if score > 0 {
                feeds.push((score, site.name.clone(), rss.to_string()));
            }
        }
    }
    feeds.sort_by_key(|(score, name, _)| (Reverse(*score), name.clone()));
    feeds
        .into_iter()
        .take(limit)
        .map(|(_, name, url)| citation_source(name, url, "Catalog feed; not yet fetched".into()))
        .collect()
}

pub fn filter_suggested_urls_to_safe_sources(
    urls: &[String],
    attempted_domains: &HashSet<String>,
    max_candidates: usize,
) -> Vec<CitationSource> {
    if max_candidates == 0 {
        return Vec::new();
    }

    let mut out = Vec::<CitationSource>::new();
    let mut seen_domains = HashSet::<String>::new();
    let mut seen_urls = HashSet::<String>::new();

    for url in urls {
        if out.len() >= max_candidates {
            break;
        }

        let Ok(canonical) = canonicalize_url(url) else {
            continue;
        };
        let Some(domain) = domain_from_url(&canonical) else {
            continue;
        };

        if !is_safe_domain(&domain)
            || attempted_domains.contains(&domain)
            || !seen_domains.insert(domain.clone())
            || !seen_urls.insert(canonical.clone())
        {
            continue;
        }

        out.push(citation_source(
            domain.clone(),
            canonical,
            "OpenRouter-suggested trusted source candidate.".to_string(),
        ));
    }

    out
}
