// Copyright 2026 a7mddra
// SPDX-License-Identifier: Apache-2.0

//! LLM fallback URL suggester, OpenRouter/OpenAI-syntax edition.
//!
//! The donor called Google's Generative Language endpoint (`x-goog-api-key`);
//! this version is provider-agnostic: `POST {base_url}/chat/completions` with
//! a Bearer key, any OpenRouter model. Same contract in and out: prompt in,
//! JSON-array-of-URLs out (parsed by the unchanged `parse_url_array_from_text`).
//!
//! Pair with `safe_sources::filter_suggested_urls_to_safe_sources` before
//! fetching anything it returns.

use serde::Deserialize;
use std::collections::HashSet;
use std::time::Duration;

#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    choices: Option<Vec<ChatChoice>>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: Option<ChatMessage>,
}

#[derive(Debug, Deserialize)]
struct ChatMessage {
    content: Option<serde_json::Value>,
}

fn extract_text_from_message(message: &ChatMessage) -> Option<String> {
    match &message.content {
        Some(serde_json::Value::String(s)) => {
            let trimmed = s.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        }
        Some(serde_json::Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| match part {
                serde_json::Value::String(s) => Some(s.as_str()),
                serde_json::Value::Object(map) => map.get("text").and_then(|v| v.as_str()),
                _ => None,
            })
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string()
            .into(),
        _ => None,
    }
    .filter(|s| !s.is_empty())
}

fn parse_url_array_from_text(raw: &str, max_urls: usize) -> Vec<String> {
    fn normalize_urls(values: Vec<String>, max_urls: usize) -> Vec<String> {
        let mut out = Vec::<String>::new();
        let mut seen = HashSet::<String>::new();
        for value in values {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Ok(parsed) = url::Url::parse(trimmed) else {
                continue;
            };
            let scheme = parsed.scheme().to_ascii_lowercase();
            if scheme != "http" && scheme != "https" {
                continue;
            }
            let normalized = parsed.to_string();
            if seen.insert(normalized.clone()) {
                out.push(normalized);
                if out.len() >= max_urls {
                    break;
                }
            }
        }
        out
    }

    let text = raw.trim();
    if text.is_empty() {
        return Vec::new();
    }

    if let Ok(values) = serde_json::from_str::<Vec<String>>(text) {
        return normalize_urls(values, max_urls);
    }

    let start = text.find('[');
    let end = text.rfind(']');
    if let (Some(s), Some(e)) = (start, end) {
        if s < e {
            let slice = &text[s..=e];
            if let Ok(values) = serde_json::from_str::<Vec<String>>(slice) {
                return normalize_urls(values, max_urls);
            }
        }
    }

    Vec::new()
}

/// Ask any OpenRouter model for up to `max_urls` direct public URLs for
/// `query`. Returns `[]` on any failure — callers treat this as best-effort.
///
/// `client` is caller-owned so apps can reuse their pool; timeouts are
/// intentionally short (8s) since this is a fallback path.
pub async fn suggest_fallback_urls(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    model: &str,
    query: &str,
    max_urls: usize,
) -> Vec<String> {
    if max_urls == 0 || api_key.trim().is_empty() || model.trim().is_empty() {
        return Vec::new();
    }

    let endpoint = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let prompt = format!(
        "Suggest up to {max_urls} direct, publicly accessible URLs for this query: \"{query}\".\n\
         Prefer captcha-free sources like Wikipedia, official docs, blogs, and trusted public pages.\n\
         Return ONLY a JSON array of URLs. No markdown, no explanation."
    );

    let request_body = serde_json::json!({
        "model": model,
        "messages": [
            { "role": "system", "content": "You suggest source URLs. Reply with ONLY a JSON array of URLs, no other text." },
            { "role": "user", "content": prompt }
        ],
        "temperature": 0.2,
        "max_tokens": 512,
    });

    let response = match tokio::time::timeout(
        Duration::from_secs(8),
        client
            .post(&endpoint)
            .header("Authorization", format!("Bearer {api_key}"))
            .header("HTTP-Referer", "https://github.com/a7mddra/groundweb")
            .header("X-Title", "groundweb-suggester")
            .json(&request_body)
            .send(),
    )
    .await
    {
        Ok(Ok(resp)) => resp,
        Ok(Err(_)) => return Vec::new(),
        Err(_) => return Vec::new(),
    };

    if !response.status().is_success() {
        return Vec::new();
    }

    let body = match tokio::time::timeout(Duration::from_secs(8), response.text()).await {
        Ok(Ok(v)) => v,
        Ok(Err(_)) => return Vec::new(),
        Err(_) => return Vec::new(),
    };

    let parsed: ChatCompletionResponse = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    let Some(text) = parsed
        .choices
        .as_ref()
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.message.as_ref())
        .and_then(extract_text_from_message)
    else {
        return Vec::new();
    };

    parse_url_array_from_text(&text, max_urls)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bare_json_array() {
        let out =
            parse_url_array_from_text(r#"["https://example.com/a", "https://example.org/b"]"#, 5);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn extracts_array_embedded_in_prose() {
        let out = parse_url_array_from_text("here you go:\n[\"https://example.com/a\"]\ncheers", 5);
        assert_eq!(out, vec!["https://example.com/a".to_string()]);
    }

    #[test]
    fn drops_non_http_schemes_and_dupes() {
        let out = parse_url_array_from_text(
            r#"["https://example.com/a", "file:///etc/passwd", "https://example.com/a"]"#,
            5,
        );
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn extracts_openai_string_content() {
        let msg = ChatMessage {
            content: Some(serde_json::Value::String(
                "[\"https://example.com\"]".to_string(),
            )),
        };
        assert!(extract_text_from_message(&msg)
            .unwrap()
            .contains("example.com"));
    }

    #[test]
    fn extracts_openai_part_array_content() {
        let msg = ChatMessage {
            content: Some(
                serde_json::json!([{"type": "text", "text": "[\"https://example.com\"]"}]),
            ),
        };
        assert!(extract_text_from_message(&msg)
            .unwrap()
            .contains("example.com"));
    }
}
