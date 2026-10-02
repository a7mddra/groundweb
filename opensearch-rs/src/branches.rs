//! All discovery providers dispatch here; retrieval and UI hydration are shared.
use futures_util::{future::BoxFuture, stream, StreamExt};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

use crate::{
    constants::{DEFAULT_MAX_RESULTS, DISCOVERY_TIMEOUT_SECS},
    html::{build_query_result, rerank_sources},
    transport::TransportClients,
    types::WebSearchResult,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchBranch {
    /// Merge all healthy free/keyless branches.
    #[default]
    Auto,
    Mojeek,
    Bing,
    PublicSources,
    Exa,
    Parallel,
}
impl SearchBranch {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Mojeek => "mojeek",
            Self::Bing => "bing",
            Self::PublicSources => "public_sources",
            Self::Exa => "exa",
            Self::Parallel => "parallel",
        }
    }
}

lazy_static::lazy_static! {
    static ref COOLDOWNS: Mutex<HashMap<SearchBranch, Instant>> = Mutex::new(HashMap::new());
}

pub(crate) fn run_branch<'a>(
    branch: SearchBranch,
    query: &'a str,
    max_results: Option<usize>,
    clients: &'a TransportClients,
) -> BoxFuture<'a, Result<WebSearchResult, String>> {
    Box::pin(async move {
        let limit = max_results.unwrap_or(DEFAULT_MAX_RESULTS);
        if branch == SearchBranch::Auto {
            let providers = [
                SearchBranch::Mojeek,
                SearchBranch::Bing,
                SearchBranch::PublicSources,
                SearchBranch::Exa,
                SearchBranch::Parallel,
            ];
            let mut jobs = stream::iter(providers)
                .map(|provider| async move {
                    (
                        provider,
                        run_branch(provider, query, Some(limit), clients).await,
                    )
                })
                .buffer_unordered(5);
            let mut sources = Vec::new();
            let mut notes = Vec::new();
            while let Some((provider, result)) = jobs.next().await {
                match result {
                    Ok(result) => {
                        notes.push(format!(
                            "{}: {} results{}",
                            provider.as_str(),
                            result.sources.len(),
                            result.message.map(|m| format!("; {m}")).unwrap_or_default()
                        ));
                        sources.extend(result.sources);
                    }
                    Err(error) => notes.push(format!("{}: {error}", provider.as_str())),
                }
            }
            if sources.is_empty() {
                return Err(notes.join("; "));
            }
            // Arrival order must not decide ranking.
            sources.sort_by(|a, b| a.url.cmp(&b.url));
            let mut result = build_query_result(
                query,
                rerank_sources(query, sources, limit),
                Some(notes.join("; ")),
            );
            result.mode = "auto".into();
            return Ok(result);
        }
        if COOLDOWNS
            .lock()
            .unwrap()
            .get(&branch)
            .is_some_and(|until| *until > Instant::now())
        {
            return Err("temporarily cooling down after a block/rate limit".into());
        }
        let task = async {
            match branch {
                SearchBranch::Mojeek => crate::mojeek::run_mojeek_query_once(query, limit, clients)
                    .await
                    .map(|sources| build_query_result(query, sources, None)),
                SearchBranch::Bing => crate::bing::search(query, limit, clients).await,
                SearchBranch::PublicSources => {
                    crate::public_sources::search(query, limit, clients).await
                }
                SearchBranch::Exa => crate::mcp_search::search(branch, query, limit, clients).await,
                SearchBranch::Parallel => {
                    crate::mcp_search::search(branch, query, limit, clients).await
                }
                SearchBranch::Auto => unreachable!(),
            }
        };
        let started = Instant::now();
        let result = tokio::time::timeout(Duration::from_secs(DISCOVERY_TIMEOUT_SECS), task)
            .await
            .map_err(|_| "discovery deadline exceeded".to_string())?
            .map_err(|e| {
                if matches!(e.kind, crate::types::SearchFailureClass::Challenge)
                    || e.message.contains("429")
                {
                    COOLDOWNS.lock().unwrap().insert(
                        branch,
                        Instant::now() + Duration::from_secs(e.retry_after_secs.unwrap_or(60)),
                    );
                }
                e.public_message()
            });
        eprintln!(
            "[WebSearch] provider={} elapsed_ms={} results={}",
            branch.as_str(),
            started.elapsed().as_millis(),
            result.as_ref().map(|r| r.sources.len()).unwrap_or(0)
        );
        result.map(|mut result| {
            result.mode = branch.as_str().into();
            result
        })
    })
}
