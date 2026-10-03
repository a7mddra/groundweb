//! All discovery providers dispatch here; retrieval and UI hydration are shared.
use futures_util::{future::BoxFuture, stream, StreamExt};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

use crate::{
    constants::DEFAULT_MAX_RESULTS,
    execution::{Observer, ProgressEvent, RetrievalFailure},
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
    timeout_secs: u64,
    progress: Observer<'a>,
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
                        run_branch(
                            provider,
                            query,
                            Some(limit),
                            clients,
                            timeout_secs,
                            progress,
                        )
                        .await,
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
        progress(ProgressEvent::Discovering { branch });
        if COOLDOWNS
            .lock()
            .unwrap()
            .get(&branch)
            .is_some_and(|until| *until > Instant::now())
        {
            let failure = RetrievalFailure {
                target: branch.as_str().into(),
                stage: "discovery".into(),
                kind: "cooldown".into(),
                message: "temporarily cooling down after a block/rate limit".into(),
                retryable: true,
                retry_after_secs: COOLDOWNS
                    .lock()
                    .unwrap()
                    .get(&branch)
                    .map(|until| until.saturating_duration_since(Instant::now()).as_secs()),
            };
            progress(ProgressEvent::Failed {
                failure: failure.clone(),
            });
            return Err(failure.message);
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
        let result = tokio::time::timeout(Duration::from_secs(timeout_secs), task)
            .await
            .map_err(|_| {
                let failure = RetrievalFailure::deadline(branch.as_str().into(), "discovery");
                progress(ProgressEvent::Failed {
                    failure: failure.clone(),
                });
                failure.message
            })?
            .map_err(|e| {
                if matches!(e.kind, crate::types::SearchFailureClass::Challenge)
                    || e.message.contains("429")
                {
                    COOLDOWNS.lock().unwrap().insert(
                        branch,
                        Instant::now() + Duration::from_secs(e.retry_after_secs.unwrap_or(60)),
                    );
                }
                progress(ProgressEvent::Failed {
                    failure: RetrievalFailure {
                        target: branch.as_str().into(),
                        stage: "discovery".into(),
                        kind: e.kind.as_str().into(),
                        message: e.message.clone(),
                        retryable: e.retriable,
                        retry_after_secs: e.retry_after_secs,
                    },
                });
                e.public_message()
            });
        result.map(|mut result| {
            progress(ProgressEvent::Discovered {
                branch,
                sources: result.sources.len(),
            });
            result.mode = branch.as_str().into();
            result
        })
    })
}
