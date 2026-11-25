//! Size caps shared by the sync transport adapters. A storage endpoint is
//! untrusted, so no adapter reads a response body or file without a bound.

use threestrands_sync_transport::TransportError;

/// Largest object any adapter will store or fetch: far above any real
/// envelope chunk, only here to bound a corrupted or hostile response.
pub(crate) const MAX_OBJECT_BYTES: u64 = 8 * 1024 * 1024;

/// Most of an error response body that is read. Error messages are short;
/// anything past this is dropped rather than buffered.
pub(crate) const MAX_ERROR_BODY_BYTES: u64 = 16 * 1024;

/// Reads a response body, failing with `Corruption` once it exceeds `max`
/// bytes (checked up front against `Content-Length`, then while streaming).
pub(crate) async fn read_capped(
    response: &mut reqwest::Response,
    max: u64,
    what: &str,
) -> Result<Vec<u8>, TransportError> {
    let too_large = || TransportError::Corruption(format!("{what} exceeds the size limit"));
    if response.content_length().is_some_and(|length| length > max) {
        return Err(too_large());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| TransportError::Transient(error.to_string()))?
    {
        if (bytes.len() + chunk.len()) as u64 > max {
            return Err(too_large());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

/// Reads at most [`MAX_ERROR_BODY_BYTES`] of an error response as text,
/// dropping the rest. Read failures yield what was read so far.
pub(crate) async fn read_error_body(mut response: reqwest::Response) -> String {
    let mut bytes: Vec<u8> = Vec::new();
    while let Ok(Some(chunk)) = response.chunk().await {
        let room = (MAX_ERROR_BODY_BYTES as usize).saturating_sub(bytes.len());
        bytes.extend_from_slice(&chunk[..chunk.len().min(room)]);
        if bytes.len() >= MAX_ERROR_BODY_BYTES as usize {
            break;
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::get;

    async fn serve(length: usize, status: u16) -> String {
        let app = axum::Router::new().route(
            "/",
            get(move || async move {
                (
                    axum::http::StatusCode::from_u16(status).unwrap(),
                    "a".repeat(length),
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        url
    }

    #[tokio::test]
    async fn a_body_is_read_below_and_at_the_cap_and_refused_above_it() {
        for (length, ok) in [(99, true), (100, true), (101, false)] {
            let url = serve(length, 200).await;
            let mut response = reqwest::get(&url).await.unwrap();
            let result = read_capped(&mut response, 100, "body").await;
            match result {
                Ok(bytes) => assert!(ok && bytes.len() == length),
                Err(error) => assert!(!ok && matches!(error, TransportError::Corruption(_))),
            }
        }
    }

    #[tokio::test]
    async fn an_error_body_is_truncated_to_the_cap() {
        for (length, expected) in [
            (10, 10),
            (MAX_ERROR_BODY_BYTES as usize, MAX_ERROR_BODY_BYTES as usize),
            (
                MAX_ERROR_BODY_BYTES as usize + 1,
                MAX_ERROR_BODY_BYTES as usize,
            ),
            (5 * 1024 * 1024, MAX_ERROR_BODY_BYTES as usize),
        ] {
            let response = reqwest::get(serve(length, 500).await).await.unwrap();
            assert_eq!(read_error_body(response).await.len(), expected);
        }
    }
}
