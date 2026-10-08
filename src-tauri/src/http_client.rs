//! The one place outbound HTTP clients start. reqwest is built with
//! `rustls-no-provider`, so TLS needs a process-wide rustls crypto provider
//! before any client is built; installing it here (rather than at app
//! startup) also covers clients built in tests and before the updater's own
//! lazy install. ring matches the provider tauri-plugin-updater installs.

use std::sync::Once;

static INSTALL_PROVIDER: Once = Once::new();

/// A `reqwest::ClientBuilder` whose TLS backend is ready to use.
pub(crate) fn builder() -> reqwest::ClientBuilder {
    INSTALL_PROVIDER.call_once(|| {
        // Another component (such as the updater) may have installed ring
        // first; either way a provider is in place.
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
    reqwest::Client::builder()
}

#[cfg(test)]
mod tests {
    #[test]
    fn builds_a_tls_capable_client_after_installing_a_provider() {
        let client = super::builder().https_only(true).build();
        assert!(client.is_ok(), "{client:?}");
        assert!(rustls::crypto::CryptoProvider::get_default().is_some());
    }
}
