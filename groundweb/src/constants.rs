// Copyright 2026 a7mddra
// SPDX-License-Identifier: Apache-2.0

//! Shared leaf constants for the search branches.
//!
pub const MOJEEK_SEARCH_URL: &str = "https://www.mojeek.com/search?q=";
pub const DEFAULT_MAX_RESULTS: usize = 8;
pub const MAX_RESULTS: usize = 20;
pub const MAX_URLS: usize = 8;
pub const FETCH_CONCURRENCY: usize = 3;
pub const DISCOVERY_TIMEOUT_SECS: u64 = 10;
pub const PAGE_TIMEOUT_SECS: u64 = 10;
pub const MAX_REDIRECTS: usize = 5;
pub const MAX_FETCH_BYTES: usize = 1024 * 1024;
pub const MAX_FETCH_CHARS: usize = 12_000;
pub const MAX_SUMMARY_WORDS: usize = 50;
pub const MAX_RETRIES: usize = 2;
pub const REQUEST_TIMEOUT_SECS: u64 = 8;
pub const CONNECT_TIMEOUT_SECS: u64 = 4;
pub const FAVICON_TIMEOUT_SECS: u64 = 5;
pub const MAX_FAVICON_BYTES: usize = 128 * 1024;
