//! Backend owner: secure authorization, official API access, queue reduction and persistence.
mod api;
mod auth;
mod commands;
mod connect;
mod discover;
mod lyrics;
mod playback;
mod playback_auth;
mod queue;
mod remote;
mod storage;
mod transfer;

use crate::{contracts::*, domain::*};

/// Builds conservative startup state without disk, network, D-Bus, or credential access.
pub fn initial_state() -> AppState {
    AppState {
        settings: storage::default_settings(),
        auth: AuthState::NeedsClientId,
        player: PlayerState {
            status: PlaybackStatus::Stopped,
            current: None,
            current_entry_id: None,
            position_ms: 0,
            observed_at: std::time::Instant::now(),
            volume: 1.0,
            shuffle: false,
            repeat: RepeatMode::Off,
            queue_revision: 0,
            seek_revision: 0,
        },
        connect: ConnectState::default(),
        cache_used_bytes: 0,
        cache_limit_bytes: 1_000_000_000,
        lyrics_key_set: false,
        discover_active: false,
    }
}

mod actor;

/// Starts an HTTPS client builder after installing the process TLS provider (ring, shared with librespot).
pub(crate) fn http_client() -> reqwest::ClientBuilder {
    // Fails only when a provider is already installed, which is equally usable.
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::Client::builder()
}

/// Describes a request failure by its innermost cause, without the URL.
pub(crate) fn network_failure(context: &str, error: reqwest::Error) -> String {
    let error = error.without_url();
    let mut cause: &dyn std::error::Error = &error;
    while let Some(next) = cause.source() {
        cause = next;
    }
    format!("{context}: {cause}")
}

/// Runs the responsive backend owner until Quit or channel closure.
pub async fn run(port: BackendPort) -> Result<(), AppError> {
    actor::run(port).await
}

#[cfg(test)]
mod tests {
    /// Regression: reqwest 0.13 built without a TLS backend rejects every https URL before connecting.
    #[tokio::test]
    async fn http_client_supports_https() {
        let client = super::http_client().build().unwrap();
        let error = client.get("https://127.0.0.1:9/").send().await.unwrap_err();
        let message = super::network_failure("probe", error);
        assert!(!message.contains("scheme is not http"), "{message}");
    }
}
