// Copyright 2026 a7mddra
// SPDX-License-Identifier: MIT

//! Shared result/error types for all search branches.
//!
//! `CitationSource` (including its favicon fields) is the frontend-facing
//! shape — keep it stable, every current and future branch emits it.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CitationSource {
    pub title: String,
    pub url: String,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub favicon_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub favicon_base64: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WebSearchResult {
    pub mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_url: Option<String>,
    pub context_markdown: String,
    pub sources: Vec<CitationSource>,
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SearchFailureClass {
    ProxyTransport,
    ConnectTimeout,
    ReadTimeout,
    Challenge,
    HttpStatus,
    NoResults,
    Dns,
    InvalidUrl,
    BlockedTarget,
    Parse,
    Other,
}

impl SearchFailureClass {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            SearchFailureClass::ProxyTransport => "proxy_transport",
            SearchFailureClass::ConnectTimeout => "connect_timeout",
            SearchFailureClass::ReadTimeout => "read_timeout",
            SearchFailureClass::Challenge => "challenge",
            SearchFailureClass::HttpStatus => "http_status",
            SearchFailureClass::NoResults => "no_results",
            SearchFailureClass::Dns => "dns",
            SearchFailureClass::InvalidUrl => "invalid_url",
            SearchFailureClass::BlockedTarget => "blocked_target",
            SearchFailureClass::Parse => "parse",
            SearchFailureClass::Other => "other",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SearchError {
    pub(crate) kind: SearchFailureClass,
    pub(crate) message: String,
    pub(crate) retriable: bool,
    pub(crate) retry_after_secs: Option<u64>,
}

impl SearchError {
    pub(crate) fn fatal(kind: SearchFailureClass, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            retriable: false,
            retry_after_secs: None,
        }
    }

    pub(crate) fn retriable(kind: SearchFailureClass, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            retriable: true,
            retry_after_secs: None,
        }
    }

    pub(crate) fn public_message(&self) -> String {
        format!("[{}] {}", self.kind.as_str(), self.message)
    }

    pub(crate) fn with_retry_after(mut self, seconds: u64) -> Self {
        self.retry_after_secs = Some(seconds.clamp(1, 3600));
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransportRoute {
    Direct,
    Proxy,
}

impl TransportRoute {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            TransportRoute::Direct => "direct",
            TransportRoute::Proxy => "proxy",
        }
    }
}
