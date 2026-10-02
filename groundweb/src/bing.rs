//! Lightweight Bing RSS discovery; no API key or browser. Availability varies.
use crate::{
    fetch::{fetch_document, parse_feed},
    html::build_query_result,
    transport::TransportClients,
    types::{SearchError, SearchFailureClass, WebSearchResult},
};
pub(crate) async fn search(
    query: &str,
    limit: usize,
    clients: &TransportClients,
) -> Result<WebSearchResult, SearchError> {
    let mut url = url::Url::parse("https://www.bing.com/search").unwrap();
    url.query_pairs_mut()
        .append_pair("q", query)
        .append_pair("format", "rss")
        .append_pair("setlang", "en-US");
    let doc = fetch_document(
        url.as_str(),
        "application/rss+xml,application/xml,text/xml",
        clients,
    )
    .await?;
    let sources = parse_feed(&doc.body, url.as_str(), limit);
    if sources.is_empty() {
        return Err(SearchError::fatal(
            SearchFailureClass::NoResults,
            "Bing RSS returned no usable items",
        ));
    }
    Ok(build_query_result(query, sources, None))
}
