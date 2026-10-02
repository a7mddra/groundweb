// Copyright 2026 a7mddra
// SPDX-License-Identifier: Apache-2.0

//! Shared leaf constants for the search branches.
//!
//! NOTE: there is intentionally no DuckDuckGo URL here — DDG is out of scope
//! (bot blocking / paid tier). The only query branch is Mojeek.

pub const MOJEEK_SEARCH_URL: &str = "https://www.mojeek.com/search?q=";
pub const DEFAULT_MAX_RESULTS: usize = 6;
pub const MAX_REDIRECTS: usize = 5;
pub const MAX_FETCH_BYTES: usize = 320 * 1024;
pub const MAX_FETCH_CHARS: usize = 12_000;
pub const MAX_SUMMARY_WORDS: usize = 50;
pub const MAX_RETRIES: usize = 2;
pub const REQUEST_TIMEOUT_SECS: u64 = 15;
pub const CONNECT_TIMEOUT_SECS: u64 = 8;
pub const FAVICON_TIMEOUT_SECS: u64 = 5;
pub const FAVICON_CONNECT_TIMEOUT_SECS: u64 = 4;
pub const MAX_FAVICON_BYTES: usize = 128 * 1024;
