// Copyright 2026 a7mddra
// SPDX-License-Identifier: Apache-2.0

//! Branch fan-out for web search.
//!
//! `execute()` picks ONE branch per call. Today only `Mojeek` exists — it is
//! deliberately NOT the whole story: the planned real web scraper (and any
//! later provider) lands here as a new variant + one match arm, sharing the
//! global `favicon`, `safe_sources`, `transport` and `fetch` layers.

use serde::{Deserialize, Serialize};

use super::types::WebSearchResult;

/// One branch of the web-search fan-out. Serialized into the tool schema so
/// the model (or the calling app) can pick explicitly; defaults to Mojeek.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchBranch {
    /// Mojeek HTML search + safe-source rerank boost. Keyless, free.
    #[default]
    Mojeek,
}

impl SearchBranch {
    pub fn as_str(self) -> &'static str {
        match self {
            SearchBranch::Mojeek => "mojeek",
        }
    }
}

/// Run one branch. Each arm owns its query path; shared post-processing
/// (favicon hydration etc.) lives inside the branch modules themselves.
pub(crate) async fn run_branch(
    branch: SearchBranch,
    query: &str,
    max_results: Option<usize>,
) -> Result<WebSearchResult, String> {
    match branch {
        SearchBranch::Mojeek => super::mojeek::search_query(query, max_results).await,
    }
}
