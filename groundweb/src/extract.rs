//! Native content extraction. Downloaded text is evidence, never instructions.
use dom_smoothie::{Config, Readability, TextMode};
use scraper::{Html, Selector};

use crate::constants::MAX_FETCH_CHARS;
use crate::html::{clean_page_text, extract_title};

pub(crate) fn truncate_chars(text: &str, limit: usize) -> String {
    let mut chars = text.chars();
    let out: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        format!("{out}\n[Content truncated]")
    } else {
        out
    }
}

pub(crate) fn readable_html(body: &str, url: &str) -> (String, String) {
    let cfg = Config {
        text_mode: TextMode::Markdown,
        max_elements_to_parse: 30_000,
        ..Default::default()
    };
    if let Ok(mut reader) = Readability::new(body, Some(url), Some(cfg)) {
        if let Ok(article) = reader.parse() {
            if article.text_content.trim().chars().count() >= 100 {
                return (
                    article.title.to_string(),
                    truncate_chars(article.text_content.trim(), MAX_FETCH_CHARS),
                );
            }
        }
    }
    let doc = Html::parse_document(body);
    for selector in ["article", "main", "[role=main]", "body"] {
        let selector = Selector::parse(selector).expect("static selector");
        if let Some(node) = doc.select(&selector).next() {
            let text = clean_page_text(&node.html());
            if text.chars().count() >= 80 {
                return (extract_title(body, url), text);
            }
        }
    }
    (extract_title(body, url), clean_page_text(body))
}

pub(crate) fn visible_fragment(body: &str) -> String {
    // Decode entities with the DOM before normalizing whitespace.
    Html::parse_fragment(body)
        .root_element()
        .text()
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn looks_blocked(body: &str) -> bool {
    let doc = Html::parse_document(body);
    let selector = Selector::parse("title, h1").expect("static selector");
    let heading = doc
        .select(&selector)
        .flat_map(|n| n.text())
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    [
        "just a moment",
        "access denied",
        "verify you are human",
        "robot check",
        "captcha",
        "attention required",
        "403 forbidden",
    ]
    .iter()
    .any(|s| heading.contains(s))
        || body.contains("id=\"challenge-form\"")
        || body.contains("id=\"cf-chl-widget")
}
