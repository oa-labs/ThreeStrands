//! The one place outbound HTTP clients start. reqwest is built with
//! `rustls-no-provider`, so TLS needs a process-wide rustls crypto provider
//! before any client is built; installing it here (rather than at app
//! startup) also covers clients built in tests and before the updater's own
//! lazy install. ring matches the provider tauri-plugin-updater installs.
//!
//! reqwest's gzip/brotli/deflate features exist for the image proxy, which
//! must advertise browser-like `Accept-Encoding` support. Feature flags are
//! crate-wide, so the shared builder switches automatic decompression back
//! off: every other client keeps sending no `Accept-Encoding` and sees the
//! response bytes and `Content-Length` exactly as the server sent them.

use std::sync::Once;

static INSTALL_PROVIDER: Once = Once::new();

/// A `reqwest::ClientBuilder` whose TLS backend is ready to use, with
/// automatic response decompression off unless a caller opts back in.
pub(crate) fn builder() -> reqwest::ClientBuilder {
    INSTALL_PROVIDER.call_once(|| {
        // Another component (such as the updater) may have installed ring
        // first; either way a provider is in place.
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
    reqwest::Client::builder()
        .gzip(false)
        .brotli(false)
        .deflate(false)
}

#[cfg(test)]
mod tests {
    #[test]
    fn builds_a_tls_capable_client_after_installing_a_provider() {
        let client = super::builder().https_only(true).build();
        assert!(client.is_ok(), "{client:?}");
        assert!(rustls::crypto::CryptoProvider::get_default().is_some());
    }

    #[tokio::test]
    async fn shared_clients_do_not_advertise_compressed_responses() {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let read = stream.read(&mut buffer).unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
            }
            stream
                .write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
                .unwrap();
            String::from_utf8(request).unwrap().to_ascii_lowercase()
        });

        let client = super::builder().build().unwrap();
        client
            .get(format!("http://127.0.0.1:{port}/"))
            .send()
            .await
            .unwrap();
        let request = server.join().unwrap();
        assert!(!request.contains("accept-encoding:"), "{request}");
    }
}
