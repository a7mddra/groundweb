// Copyright 2026 a7mddra
// SPDX-License-Identifier: Apache-2.0

//! URL helpers shared by every search branch.
//!
//! NOTE: the DuckDuckGo `/l/?uddg=` redirect unwrapping from the donor is
//! intentionally gone — DDG is out of scope. Only plain `http(s)` URLs plus
//! the `//host/path` shorthand are canonicalized.

use std::net::{IpAddr, Ipv6Addr};
use url::Url;

use super::types::{SearchError, SearchFailureClass};

pub(crate) fn encode_query(query: &str) -> String {
    url::form_urlencoded::byte_serialize(query.as_bytes()).collect::<String>()
}

fn looks_like_loopback_host(host: &str) -> bool {
    let h = host.trim().to_ascii_lowercase();
    h == "localhost" || h.ends_with(".localhost") || h.ends_with(".local")
}

fn is_documentation_v6(v6: &Ipv6Addr) -> bool {
    let segments = v6.segments();
    segments[0] == 0x2001 && segments[1] == 0x0db8
}

fn is_unique_local_v6(v6: &Ipv6Addr) -> bool {
    (v6.segments()[0] & 0xfe00) == 0xfc00
}

fn is_unicast_link_local_v6(v6: &Ipv6Addr) -> bool {
    (v6.segments()[0] & 0xffc0) == 0xfe80
}

pub(crate) fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, _, _] = v4.octets();
            a != 0
                && a < 224
                && !(a == 100 && (64..=127).contains(&b))
                && !(a == 198 && (b == 18 || b == 19))
                && !v4.is_private()
                && !v4.is_loopback()
                && !v4.is_link_local()
                && !v4.is_multicast()
                && !v4.is_broadcast()
                && !v4.is_documentation()
                && !v4.is_unspecified()
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_ip(IpAddr::V4(v4));
            }
            !v6.is_loopback()
                && !v6.is_unspecified()
                && !v6.is_multicast()
                && !is_unique_local_v6(&v6)
                && !is_unicast_link_local_v6(&v6)
                && !is_documentation_v6(&v6)
        }
    }
}

pub(crate) async fn ensure_public_target(url: &Url) -> Result<(), SearchError> {
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(SearchError::fatal(
            SearchFailureClass::BlockedTarget,
            "Only public HTTP(S) URLs without credentials are allowed",
        ));
    }
    let host = url.host_str().ok_or_else(|| {
        SearchError::fatal(
            SearchFailureClass::BlockedTarget,
            "Blocked URL: missing host",
        )
    })?;
    if looks_like_loopback_host(host) {
        return Err(SearchError::fatal(
            SearchFailureClass::BlockedTarget,
            "Blocked URL: local host is not allowed",
        ));
    }

    let port = url.port_or_known_default().ok_or_else(|| {
        SearchError::fatal(
            SearchFailureClass::BlockedTarget,
            "Blocked URL: unknown port",
        )
    })?;
    if let Ok(ip) = host.trim_matches(['[', ']']).parse::<IpAddr>() {
        return if is_public_ip(ip) {
            Ok(())
        } else {
            Err(SearchError::fatal(
                SearchFailureClass::BlockedTarget,
                "Blocked URL: non-public IP target",
            ))
        };
    }
    let lookup = tokio::time::timeout(
        std::time::Duration::from_secs(4),
        tokio::net::lookup_host((host, port)),
    )
    .await
    .map_err(|_| SearchError::retriable(SearchFailureClass::Dns, "DNS lookup timed out"))?
    .map_err(|e| {
        SearchError::retriable(SearchFailureClass::Dns, format!("DNS lookup failed: {}", e))
    })?;

    let mut has_ip = false;
    for socket_addr in lookup {
        has_ip = true;
        if !is_public_ip(socket_addr.ip()) {
            return Err(SearchError::fatal(
                SearchFailureClass::BlockedTarget,
                format!("Blocked URL: non-public IP target ({})", socket_addr.ip()),
            ));
        }
    }
    if !has_ip {
        return Err(SearchError::retriable(
            SearchFailureClass::Dns,
            "DNS lookup returned no IP addresses".to_string(),
        ));
    }
    Ok(())
}

pub(crate) fn normalize_domain(domain: &str) -> String {
    domain.trim().trim_start_matches('.').to_ascii_lowercase()
}

pub fn domain_from_url(url: &str) -> Option<String> {
    Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(normalize_domain))
        .map(|d| d.trim_start_matches("www.").to_string())
}

pub(crate) fn canonicalize_url(raw: &str) -> Result<String, SearchError> {
    let mut input = raw.trim().to_string();
    if input.is_empty() {
        return Err(SearchError::fatal(
            SearchFailureClass::InvalidUrl,
            "Invalid URL: empty",
        ));
    }

    if input.starts_with("//") {
        input = format!("https:{}", input);
    }

    let mut parsed = Url::parse(&input).map_err(|_| {
        SearchError::fatal(
            SearchFailureClass::InvalidUrl,
            format!("Invalid URL: {}", raw.trim()),
        )
    })?;

    let scheme = parsed.scheme().to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err(SearchError::fatal(
            SearchFailureClass::InvalidUrl,
            format!("Blocked URL scheme: {}", parsed.scheme()),
        ));
    }
    if parsed.host_str().is_none() {
        return Err(SearchError::fatal(
            SearchFailureClass::InvalidUrl,
            "Blocked URL: host is required",
        ));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(SearchError::fatal(
            SearchFailureClass::InvalidUrl,
            "URL credentials are not allowed",
        ));
    }

    parsed.set_fragment(None);
    let pairs: Vec<(String, String)> = parsed
        .query_pairs()
        .filter(|(key, _)| {
            let key = key.to_ascii_lowercase();
            !key.starts_with("utm_")
                && !matches!(key.as_str(), "gclid" | "fbclid" | "mc_cid" | "mc_eid")
        })
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    if parsed.query().is_some() {
        parsed.set_query(None);
        if !pairs.is_empty() {
            parsed.query_pairs_mut().extend_pairs(pairs);
        }
    }
    if (parsed.scheme() == "https" && parsed.port() == Some(443))
        || (parsed.scheme() == "http" && parsed.port() == Some(80))
    {
        let _ = parsed.set_port(None);
    }

    Ok(parsed.to_string())
}
