// Copyright 2026 a7mddra
// SPDX-License-Identifier: MIT

//! Keyless Mojeek HTML discovery. New providers dispatch beside it in branches.
//! Blocks fail promptly so shared discovery can preserve other providers.

use reqwest::{header, StatusCode};

use super::constants::{DEFAULT_MAX_RESULTS, MAX_FETCH_BYTES, MOJEEK_SEARCH_URL};
use super::favicon::hydrate_favicons_for_sources;
use super::html::{
    build_query_result, looks_like_mojeek_block_page, parse_mojeek_results, rerank_sources,
};
use super::retry::{emit_progress, with_retries_with_progress};
use super::transport::{read_capped_response_body, send_with_transport, TransportClients};
use super::types::{CitationSource, SearchError, SearchFailureClass, WebSearchResult};
use super::url_utils::encode_query;

pub(crate) async fn run_mojeek_query_once(
    query: &str,
    max_results: usize,
    clients: &TransportClients,
) -> Result<Vec<CitationSource>, SearchError> {
    let encoded = encode_query(query);
    let search_url = format!("{}{}", MOJEEK_SEARCH_URL, encoded);

    let (response, route) = send_with_transport(clients, move |client| {
        client
            .get(search_url.clone())
            .header(header::ACCEPT, "text/html")
    })
    .await?;

    if !response.status().is_success() {
        let status = response.status();
        let status_message = format!(
            "mojeek returned HTTP {} via {}",
            status.as_u16(),
            route.as_str()
        );

        if status == StatusCode::FORBIDDEN {
            return Err(SearchError::fatal(
                SearchFailureClass::Challenge,
                "Mojeek blocked automated requests (403)",
            ));
        }

        return if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
            Err(SearchError::retriable(
                SearchFailureClass::HttpStatus,
                status_message,
            ))
        } else {
            Err(SearchError::fatal(
                SearchFailureClass::HttpStatus,
                status_message,
            ))
        };
    }

    let html = read_capped_response_body(response, MAX_FETCH_BYTES).await?;

    if looks_like_mojeek_block_page(&html) {
        return Err(SearchError::fatal(
            SearchFailureClass::Challenge,
            "Mojeek temporarily blocked automated requests",
        ));
    }

    let parsed = parse_mojeek_results(&html, max_results);

    if parsed.is_empty() {
        return Err(SearchError::retriable(
            SearchFailureClass::NoResults,
            "mojeek returned no usable results".to_string(),
        ));
    }

    Ok(rerank_sources(query, parsed, max_results))
}

pub async fn search_query(
    query: &str,
    max_results: Option<usize>,
) -> Result<WebSearchResult, String> {
    search_query_with_progress(query, max_results, |_| {}).await
}

pub async fn search_query_with_progress<F>(
    query: &str,
    max_results: Option<usize>,
    mut progress: F,
) -> Result<WebSearchResult, String>
where
    F: FnMut(String) + Send,
{
    let q = query.trim();
    if q.is_empty() {
        return Err("No query provided".to_string());
    }

    let limit = max_results
        .unwrap_or(DEFAULT_MAX_RESULTS)
        .clamp(1, crate::constants::MAX_RESULTS);

    let clients = TransportClients::build().map_err(|e| e.public_message())?;
    let mut progress_ref: Option<&mut (dyn FnMut(String) + Send)> = Some(&mut progress);

    emit_progress(
        &mut progress_ref,
        "Searching for relevant sources".to_string(),
    );

    match with_retries_with_progress(
        "Mojeek",
        || run_mojeek_query_once(q, limit, &clients),
        &mut progress_ref,
    )
    .await
    {
        Ok(mut sources) => {
            hydrate_favicons_for_sources(&mut sources).await;
            eprintln!(
                "[WebSearch] backend=mojeek success results={}",
                sources.len()
            );
            emit_progress(
                &mut progress_ref,
                format!("Mojeek: found {} results", sources.len()),
            );
            Ok(build_query_result(q, sources, None))
        }
        Err(error) => {
            eprintln!(
                "[WebSearch] backend=mojeek failed [{}]: {}",
                error.kind.as_str(),
                error.message
            );
            emit_progress(
                &mut progress_ref,
                "Search is unavailable right now.".to_string(),
            );
            Err(error.public_message())
        }
    }
}
