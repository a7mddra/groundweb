// Copyright 2026 a7mddra
// SPDX-License-Identifier: MIT

//! Caller-owned retrieval limits and live, factual retrieval events.
use crate::{CitationSource, SearchBranch};
use serde::{Deserialize, Serialize};

/// Resource ceilings remain enforced even when a caller raises these limits.
#[derive(Debug, Clone)]
pub struct ExecutionOptions {
    pub pages_to_read: usize,
    pub page_attempts: usize,
    pub context_chars: usize,
    pub discovery_timeout_secs: u64,
    pub reading_timeout_secs: u64,
}

impl Default for ExecutionOptions {
    fn default() -> Self {
        Self {
            pages_to_read: 3,
            page_attempts: 6,
            context_chars: 64_000,
            discovery_timeout_secs: crate::constants::DISCOVERY_TIMEOUT_SECS,
            reading_timeout_secs: 20,
        }
    }
}

impl ExecutionOptions {
    pub(crate) fn bounded(mut self) -> Self {
        self.pages_to_read = self.pages_to_read.clamp(1, 8);
        self.page_attempts = self.page_attempts.clamp(self.pages_to_read, 20);
        self.context_chars = self.context_chars.clamp(1_000, 128_000);
        self.discovery_timeout_secs = self.discovery_timeout_secs.clamp(1, 30);
        self.reading_timeout_secs = self.reading_timeout_secs.clamp(1, 30);
        self
    }
}

/// A failed provider or page does not invalidate other retrieved evidence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalFailure {
    pub target: String,
    pub stage: String,
    pub kind: String,
    pub message: String,
    pub retryable: bool,
    pub retry_after_secs: Option<u64>,
}

impl RetrievalFailure {
    pub(crate) fn page(target: String, error: &crate::types::SearchError) -> Self {
        Self {
            target,
            stage: "reading".into(),
            kind: error.kind.as_str().into(),
            message: error.message.clone(),
            retryable: error.retriable,
            retry_after_secs: error.retry_after_secs,
        }
    }
    pub(crate) fn deadline(target: String, stage: &str) -> Self {
        Self {
            target,
            stage: stage.into(),
            kind: "timeout".into(),
            message: format!("{stage} deadline exceeded"),
            retryable: true,
            retry_after_secs: None,
        }
    }
}

/// These are retrieval observations, never model reasoning. Callbacks run
/// synchronously and should return quickly. Dropping the execution future
/// cancels its in-flight requests; no background retrieval task is spawned.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProgressEvent {
    Discovering {
        branch: SearchBranch,
    },
    Discovered {
        branch: SearchBranch,
        sources: usize,
    },
    Reading {
        url: String,
    },
    Read {
        url: String,
        source: CitationSource,
    },
    SourceReady {
        source: CitationSource,
        fetched: bool,
    },
    Failed {
        failure: RetrievalFailure,
    },
    Finished {
        sources: usize,
        pages_read: usize,
    },
}

pub(crate) type Observer<'a> = &'a (dyn Fn(ProgressEvent) + Send + Sync);
