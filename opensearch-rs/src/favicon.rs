//! Global origin favicon hydration for every discovery/retrieval branch.
use crate::{
    constants::{FAVICON_TIMEOUT_SECS, MAX_FAVICON_BYTES, MAX_REDIRECTS},
    transport::{send_with_transport, TransportClients},
    types::CitationSource,
    url_utils::ensure_public_target,
};
use base64::{engine::general_purpose, Engine};
use futures_util::{stream, StreamExt};
use reqwest::header;
use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};
use url::Url;

pub fn favicon_for_url(url: &str) -> Option<String> {
    let parsed = Url::parse(url).ok()?;
    matches!(parsed.scheme(), "http" | "https")
        .then(|| format!("{}/favicon.ico", parsed.origin().ascii_serialization()))
}
pub fn citation_source(title: String, url: String, summary: String) -> CitationSource {
    CitationSource {
        title,
        favicon_url: favicon_for_url(&url),
        url,
        summary,
        favicon_base64: None,
    }
}
lazy_static::lazy_static! {
    static ref ICON_CACHE:Mutex<HashMap<String,(Instant,Option<String>)>>=Mutex::new(HashMap::new());
}
async fn fetch_icon(raw: &str, clients: &TransportClients) -> Option<String> {
    let mut url = Url::parse(raw).ok()?;
    for _ in 0..=MAX_REDIRECTS {
        ensure_public_target(&url).await.ok()?;
        let (response, _) = send_with_transport(clients, |client| {
            client.get(url.clone()).header(header::ACCEPT, "image/*")
        })
        .await
        .ok()?;
        if response.status().is_redirection() {
            url = url
                .join(response.headers().get(header::LOCATION)?.to_str().ok()?)
                .ok()?;
            continue;
        }
        if !response.status().is_success() {
            return None;
        }
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)?
            .to_str()
            .ok()?
            .split(';')
            .next()?
            .trim()
            .to_lowercase();
        let mime = match content_type.as_str() {
            "image/png" => "image/png",
            "image/jpeg" => "image/jpeg",
            "image/webp" => "image/webp",
            "image/svg+xml" => "image/svg+xml",
            "image/x-icon" | "image/vnd.microsoft.icon" => "image/x-icon",
            _ => return None,
        };
        let mut bytes = Vec::new();
        let mut chunks = response.bytes_stream();
        while let Some(chunk) = chunks.next().await {
            let chunk = chunk.ok()?;
            if bytes.len() + chunk.len() > MAX_FAVICON_BYTES {
                return None;
            }
            bytes.extend_from_slice(&chunk);
        }
        if bytes.is_empty() {
            return None;
        }
        return Some(format!(
            "data:{mime};base64,{}",
            general_purpose::STANDARD.encode(bytes)
        ));
    }
    None
}

/// Bounded, cached hydration. Icons never delay a source by more than 5s.
pub async fn hydrate_favicons_for_sources(sources: &mut [CitationSource]) {
    let Ok(clients) = TransportClients::build() else {
        return;
    };
    let mut origins = Vec::new();
    for source in sources.iter() {
        if let Ok(url) = Url::parse(&source.url) {
            if matches!(url.scheme(), "http" | "https") {
                let origin = url.origin().ascii_serialization();
                if !origins.contains(&origin) {
                    origins.push(origin);
                }
            }
        }
    }
    let mut jobs = stream::iter(origins)
        .map(|origin| {
            let clients = &clients;
            async move {
                let cached = ICON_CACHE
                    .lock()
                    .unwrap()
                    .get(&origin)
                    .filter(|(time, icon)| {
                        time.elapsed() < Duration::from_secs(if icon.is_some() { 600 } else { 120 })
                    })
                    .cloned();
                if let Some((_, icon)) = cached {
                    return (origin, icon);
                }
                let task = async {
                    for path in ["/favicon.ico", "/favicon.png", "/apple-touch-icon.png"] {
                        if let Some(icon) = fetch_icon(&format!("{origin}{path}"), clients).await {
                            return Some(icon);
                        }
                    }
                    None
                };
                let icon = tokio::time::timeout(Duration::from_secs(FAVICON_TIMEOUT_SECS), task)
                    .await
                    .ok()
                    .flatten();
                let mut cache = ICON_CACHE.lock().unwrap();
                if cache.len() >= 64 {
                    if let Some(key) = cache
                        .iter()
                        .min_by_key(|(_, (time, _))| *time)
                        .map(|(k, _)| k.clone())
                    {
                        cache.remove(&key);
                    }
                }
                cache.insert(origin.clone(), (Instant::now(), icon.clone()));
                (origin, icon)
            }
        })
        .buffer_unordered(4);
    let mut icons = HashMap::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(FAVICON_TIMEOUT_SECS);
    while let Ok(Some((origin, icon))) = tokio::time::timeout_at(deadline, jobs.next()).await {
        icons.insert(origin, icon);
    }
    for source in sources.iter_mut() {
        if let Ok(url) = Url::parse(&source.url) {
            if let Some(Some(icon)) = icons.get(&url.origin().ascii_serialization()) {
                source.favicon_base64 = Some(icon.clone());
            }
        }
    }
}
