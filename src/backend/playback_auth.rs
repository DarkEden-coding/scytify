//! Librespot authorization and reusable AP credentials, isolated from Web API PKCE.
use super::{auth, playback::Playback};
use crate::domain::PlaybackAuth;
use librespot_core::authentication::Credentials;
use librespot_oauth::{OAuthClientBuilder, OAuthError};
use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// KEYMASTER from librespot-core 0.8.0 config.rs. Login5 stored credentials must
// originate from OAuth for this client, as in librespot's own binary.
pub(super) const CLIENT_ID: &str = "65b708073fc0480ea92a077233ca87bd";
// librespot v0.8.0 main.rs: default oauth_port=5588 and /login redirect.
const REDIRECT: &str = "http://127.0.0.1:5588/login";
// The Connect/streaming subset of main.rs OAUTH_SCOPES. No library write access.
const SCOPES: &[&str] = &[
    "streaming",
    "app-remote-control",
    "user-read-playback-state",
    "user-modify-playback-state",
    "user-read-currently-playing",
];

#[derive(Clone, Default, Serialize, Deserialize)]
pub(super) struct Record {
    user_id: String,
    tokens: Option<auth::Tokens>,
    credentials: Option<Credentials>,
}

/// The actor saves this record only after checking its authentication epoch.
/// Even a failed audio/Connect setup can retain a completed browser consent.
pub(super) struct Setup {
    pub playback: Result<Playback, PlaybackAuth>,
    pub record: Option<Record>,
}

#[derive(Debug, PartialEq, Eq)]
enum Selection {
    Stored,
    Refresh,
    NeedsAuthorization,
}

fn select(stored: bool, refresh: bool) -> Selection {
    if stored {
        Selection::Stored
    } else if refresh {
        Selection::Refresh
    } else {
        Selection::NeedsAuthorization
    }
}

fn client() -> Result<librespot_oauth::OAuthClient, String> {
    OAuthClientBuilder::new(CLIENT_ID, REDIRECT, SCOPES.to_vec())
        .open_in_browser()
        .with_custom_message("Playback authorization complete. Return to Scytify.")
        .build()
        .map_err(|_| "could not create playback authorization client".into())
}

async fn authorize() -> Result<auth::Tokens, String> {
    let client = client()?;
    let token = tokio::time::timeout(Duration::from_secs(180), client.get_access_token_async())
        .await
        .map_err(|_| "Playback authorization timed out after 180 seconds; try again".to_owned())?
        .map_err(|error| -> String {
            match error {
                OAuthError::AuthCodeListenerBind { .. } => {
                    "Playback callback port 5588 is busy; close the other listener and retry".into()
                }
                OAuthError::AuthorizationDenied => {
                    "Spotify playback authorization was denied; try again".into()
                }
                OAuthError::BrowserOpen => {
                    "Could not open the system browser for playback authorization".into()
                }
                // OAuth errors can contain callback codes or token response bodies.
                _ => "Playback authorization failed; retry and complete Spotify's consent page"
                    .into(),
            }
        })?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    Ok(auth::Tokens {
        access_token: token.access_token,
        refresh_token: (!token.refresh_token.is_empty()).then_some(token.refresh_token),
        expires_at: now
            + token
                .expires_at
                .saturating_duration_since(std::time::Instant::now())
                .as_secs(),
    })
}

/// Startup never opens a browser. Only the explicit AuthorizePlayback action opts in.
pub(super) async fn connect(
    client_id: String,
    user_id: String,
    volume: f32,
    interactive: bool,
) -> Setup {
    let mut record: Record = match auth::Vault::load_entry(auth::PLAYBACK_ENTRY) {
        Ok(record) => record.unwrap_or_default(),
        Err(error) => {
            return Setup {
                playback: Err(PlaybackAuth::Failed(error)),
                record: None,
            };
        }
    };
    if record.user_id != user_id {
        record = Record {
            user_id: user_id.clone(),
            ..Record::default()
        };
    }
    let mut last_error = None;
    if select(record.credentials.is_some(), record.tokens.is_some()) == Selection::Stored {
        let credentials = record
            .credentials
            .clone()
            .expect("selected stored credentials");
        match Playback::connect(client_id.clone(), credentials, volume).await {
            Ok(player) => return connected(player, record, &user_id),
            Err(error) => last_error = Some(error),
        }
    }
    if select(false, record.tokens.is_some()) == Selection::Refresh {
        let old = record.tokens.clone().expect("selected OAuth tokens");
        // Refresh after a stored-credential failure even if the old token looks fresh.
        let refreshed = if auth::expiring(&old) || last_error.is_some() {
            auth::refresh(CLIENT_ID, &old).await
        } else {
            Ok(old)
        };
        match refreshed {
            Ok(tokens) => {
                let credentials = Credentials::with_access_token(&tokens.access_token);
                record.tokens = Some(tokens);
                match Playback::connect(client_id.clone(), credentials, volume).await {
                    Ok(player) => return connected(player, record, &user_id),
                    Err(error) => last_error = Some(error),
                }
            }
            Err(error) => last_error = Some(error),
        }
    }
    if !interactive {
        return Setup {
            playback: Err(last_error.map_or(PlaybackAuth::NeedsAuthorization, PlaybackAuth::Failed)),
            record: (record.tokens.is_some() || record.credentials.is_some()).then_some(record),
        };
    }
    match authorize().await {
        Ok(tokens) => {
            let credentials = Credentials::with_access_token(&tokens.access_token);
            record.tokens = Some(tokens);
            match Playback::connect(client_id, credentials, volume).await {
                Ok(player) => connected(player, record, &user_id),
                Err(error) => Setup {
                    playback: Err(PlaybackAuth::Failed(error)),
                    record: Some(record),
                },
            }
        }
        Err(error) => Setup {
            playback: Err(PlaybackAuth::Failed(error)),
            record: (record.tokens.is_some() || record.credentials.is_some()).then_some(record),
        },
    }
}

fn connected(player: Playback, mut record: Record, user_id: &str) -> Setup {
    if player.credentials.username.as_deref() != Some(user_id) {
        return Setup {
            playback: Err(PlaybackAuth::Failed("Playback was authorized for a different Spotify account. Use the account signed in here and retry.".into())),
            record: None,
        };
    }
    record.credentials = Some(player.credentials.clone());
    Setup {
        playback: Ok(player),
        record: Some(record),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playback_record_round_trips_reusable_credentials_and_rotated_tokens() {
        use librespot_protocol::authentication::AuthenticationType;
        let record = Record {
            user_id: "user".into(),
            tokens: Some(auth::Tokens {
                access_token: "access".into(),
                refresh_token: Some("rotated-refresh".into()),
                expires_at: 1234,
            }),
            credentials: Some(Credentials {
                username: Some("user".into()),
                auth_type: AuthenticationType::AUTHENTICATION_STORED_SPOTIFY_CREDENTIALS,
                auth_data: vec![0, 1, 255],
            }),
        };
        let decoded: Record =
            serde_json::from_str(&serde_json::to_string(&record).unwrap()).unwrap();
        assert_eq!(decoded.credentials, record.credentials);
        assert_eq!(decoded.user_id, "user");
        let tokens = decoded.tokens.unwrap();
        assert_eq!(tokens.access_token, "access");
        assert_eq!(tokens.refresh_token.as_deref(), Some("rotated-refresh"));
        assert_eq!(tokens.expires_at, 1234);
    }

    #[test]
    fn credentials_prefer_stored_then_oauth_then_explicit_consent() {
        assert_eq!(select(true, true), Selection::Stored);
        assert_eq!(select(true, false), Selection::Stored);
        assert_eq!(select(false, true), Selection::Refresh);
        assert_eq!(select(false, false), Selection::NeedsAuthorization);
    }
}
