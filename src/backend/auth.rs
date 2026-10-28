use base64::Engine;
use keyring::Entry;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const WEB_API_ENTRY: &str = "spotify-oauth";
pub(super) const PLAYBACK_ENTRY: &str = "spotify-librespot-oauth";
const AUTH_ENTRIES: [&str; 2] = [WEB_API_ENTRY, PLAYBACK_ENTRY];

const SERVICE: &str = "dev.scytify.Scytify";
/// Keychain service used before the app was renamed; read once to migrate credentials.
const LEGACY_SERVICE: &str = "dev.customspotify.CustomSpotify";
const REDIRECT_PORT: u16 = 43817;
/// The redirect users register in their developer app; it must match the port above.
pub const REDIRECT_URI: &str = "http://127.0.0.1:43817/callback";
/// Reading permissions, plus saving to Your Library (`user-library-modify`, and
/// `playlist-modify-public` for following playlists). Adding one requires existing users to
/// sign in again; library endpoints report that as a Permission error instead of failing
/// silently.
const SCOPES: &str = "playlist-read-private playlist-read-collaborative user-read-private user-library-read user-library-modify playlist-modify-public user-follow-read user-read-recently-played streaming user-read-playback-state user-modify-playback-state user-read-currently-playing";

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Tokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: u64,
}

/// Secret-Service-only token vault. Errors remain actionable and never include secret values.
pub(crate) struct Vault;
impl Vault {
    /// Loads the Web API record, independent of playback authorization.
    pub(crate) fn load() -> Result<Option<Tokens>, String> {
        Self::load_entry(WEB_API_ENTRY)
    }
    pub(crate) fn store(t: &Tokens) -> Result<(), String> {
        Self::store_entry(WEB_API_ENTRY, t)
    }
    pub(super) fn load_entry<T: serde::de::DeserializeOwned>(
        name: &str,
    ) -> Result<Option<T>, String> {
        secret_service(|| {
            let e = Entry::new(SERVICE, name).map_err(safe_keyring)?;
            let raw = match e.get_password() {
                Ok(v) => v,
                Err(keyring::Error::NoEntry) => match migrate_legacy_entry(&e, name)? {
                    Some(v) => v,
                    None => return Ok(None),
                },
                Err(e) => return Err(safe_keyring(e)),
            };
            serde_json::from_str(&raw)
                .map(Some)
                .map_err(|_| "credential record is corrupt".into())
        })
    }
    pub(super) fn store_entry<T: Serialize>(name: &str, value: &T) -> Result<(), String> {
        let raw = serde_json::to_string(value).map_err(|_| "could not encode credentials")?;
        secret_service(|| {
            Entry::new(SERVICE, name)
                .map_err(safe_keyring)?
                .set_password(&raw)
                .map_err(safe_keyring)
        })
    }
    pub(super) fn delete_entry(name: &str) -> Result<(), String> {
        secret_service(|| {
            let entry = Entry::new(SERVICE, name).map_err(safe_keyring)?;
            match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(e) => Err(safe_keyring(e)),
            }
        })
    }
    /// Attempts every entry even if one deletion fails. No network revocation is claimed.
    pub(crate) fn clear() -> Result<(), String> {
        secret_service(|| {
            clear_entries(|name| {
                let e = Entry::new(SERVICE, name).map_err(safe_keyring)?;
                match e.delete_credential() {
                    Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                    Err(e) => Err(safe_keyring(e)),
                }
            })
        })
    }
}

/// Copies a credential saved under the app's former keychain service into `entry`, then
/// removes the old one. Returns the record, or `None` if there was nothing to migrate.
fn migrate_legacy_entry(entry: &Entry, name: &str) -> Result<Option<String>, String> {
    let legacy = Entry::new(LEGACY_SERVICE, name).map_err(safe_keyring)?;
    let raw = match legacy.get_password() {
        Ok(v) => v,
        Err(keyring::Error::NoEntry) => return Ok(None),
        Err(e) => return Err(safe_keyring(e)),
    };
    entry.set_password(&raw).map_err(safe_keyring)?;
    // The copy succeeded; a failed cleanup only leaves an unused old item behind.
    let _ = legacy.delete_credential();
    Ok(Some(raw))
}

fn clear_entries(mut delete: impl FnMut(&str) -> Result<(), String>) -> Result<(), String> {
    let mut result = Ok(());
    for name in AUTH_ENTRIES {
        if let Err(error) = delete(name) {
            result = Err(error);
        }
    }
    result
}

/// Let zbus's synchronous keyring API drive its runtime without nesting an async worker.
fn secret_service<T>(operation: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    // Serialized credential changes prevent a late login write from undoing logout.
    // ponytail: a locked OS vault can delay this actor; use an ordered vault worker if needed.
    tokio::task::block_in_place(operation)
}

/// Redacts backend-specific keyring failure details.
fn safe_keyring(_: keyring::Error) -> String {
    if cfg!(target_os = "macos") {
        "macOS Keychain is unavailable or locked; unlock the login keychain and retry".into()
    } else {
        "Linux Secret Service is unavailable or locked; unlock a Secret Service collection and retry"
            .into()
    }
}

/// Runs Spotify authorization-code PKCE against an already configured public developer client ID.
pub(crate) async fn authorize(client_id: &str) -> Result<Tokens, String> {
    if client_id.is_empty() || !client_id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err("invalid Spotify client ID".into());
    }
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, REDIRECT_PORT))
        .await
        .map_err(|_| format!("cannot bind loopback callback port {REDIRECT_PORT}"))?;
    let mut random = [0u8; 32];
    rand::rng().fill_bytes(&mut random);
    let verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random);
    rand::rng().fill_bytes(&mut random);
    let state = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random);
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(verifier.as_bytes()));
    let mut u = url::Url::parse("https://accounts.spotify.com/authorize")
        .map_err(|_| "authorization endpoint is invalid")?;
    u.query_pairs_mut()
        .append_pair("client_id", client_id)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", REDIRECT_URI)
        .append_pair("code_challenge_method", "S256")
        .append_pair("code_challenge", &challenge)
        .append_pair("state", &state)
        .append_pair("scope", SCOPES);
    open::that(u.as_str()).map_err(|_| "could not open the system browser")?;
    let (mut stream, _) =
        tokio::time::timeout(std::time::Duration::from_secs(180), listener.accept())
            .await
            .map_err(|_| "authorization timed out")?
            .map_err(|_| "callback failed")?;
    let mut buf = vec![0; 8192];
    let n = tokio::time::timeout(std::time::Duration::from_secs(5), stream.read(&mut buf))
        .await
        .map_err(|_| "callback timed out")?
        .map_err(|_| "invalid callback")?;
    let request = std::str::from_utf8(&buf[..n]).map_err(|_| "invalid callback")?;
    let target = request
        .lines()
        .next()
        .and_then(|x| x.split_whitespace().nth(1))
        .ok_or("invalid callback")?;
    let cb =
        url::Url::parse(&format!("http://127.0.0.1{target}")).map_err(|_| "invalid callback")?;
    let ok = cb.path() == "/callback" && cb.query_pairs().any(|(k, v)| k == "state" && v == state);
    let code = cb
        .query_pairs()
        .find(|(k, _)| k == "code")
        .map(|(_, v)| v.into_owned());
    let response = if ok && code.is_some() {
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 42\r\nConnection: close\r\n\r\nAuthorization complete. Return to the app."
    } else {
        "HTTP/1.1 400 Bad Request\r\nContent-Type: text/plain\r\nContent-Length: 21\r\nConnection: close\r\n\r\nAuthorization failed."
    };
    let _ = stream.write_all(response.as_bytes()).await;
    if !ok {
        return Err("authorization state mismatch".into());
    }
    let code = code.ok_or("authorization was denied")?;
    #[derive(Deserialize)]
    struct Reply {
        access_token: String,
        refresh_token: Option<String>,
        expires_in: u64,
    }
    let body = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("client_id", client_id)
        .append_pair("grant_type", "authorization_code")
        .append_pair("code", &code)
        .append_pair("redirect_uri", REDIRECT_URI)
        .append_pair("code_verifier", &verifier)
        .finish();
    let r = token_client()?
        .post("https://accounts.spotify.com/api/token")
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .body(body)
        .send()
        .await
        .map_err(|e| super::network_failure("token exchange network failure", e))?;
    if !r.status().is_success() {
        return Err("Spotify rejected the authorization exchange".into());
    }
    let t: Reply = r.json().await.map_err(|_| "invalid token response")?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    Ok(Tokens {
        access_token: t.access_token,
        refresh_token: t.refresh_token,
        expires_at: now + t.expires_in,
    })
}

/// Refreshes an expiring access token with PKCE's public-client flow and retains a rotated token.
pub(crate) async fn refresh(client_id: &str, old: &Tokens) -> Result<Tokens, String> {
    let refresh = old
        .refresh_token
        .as_deref()
        .ok_or("authorization expired; sign in again")?;
    let body = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("client_id", client_id)
        .append_pair("grant_type", "refresh_token")
        .append_pair("refresh_token", refresh)
        .finish();
    let r = token_client()?
        .post("https://accounts.spotify.com/api/token")
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .body(body)
        .send()
        .await
        .map_err(|e| super::network_failure("token refresh network failure", e))?;
    if !r.status().is_success() {
        return Err("Spotify rejected token refresh; sign in again".into());
    }
    #[derive(Deserialize)]
    struct Reply {
        access_token: String,
        refresh_token: Option<String>,
        expires_in: u64,
    }
    let t: Reply = r
        .json()
        .await
        .map_err(|_| "invalid token refresh response")?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    Ok(Tokens {
        access_token: t.access_token,
        refresh_token: t.refresh_token.or_else(|| old.refresh_token.clone()),
        expires_at: now + t.expires_in,
    })
}

/// Returns whether a token should be refreshed before beginning new network work.
pub(crate) fn expiring(t: &Tokens) -> bool {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    t.expires_at <= now.saturating_add(60)
}

/// Builds a bounded no-redirect OAuth token client.
fn token_client() -> Result<reqwest::Client, String> {
    super::http_client()
        .timeout(std::time::Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "HTTP client unavailable".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn displayed_redirect_matches_callback_listener() {
        assert_eq!(
            REDIRECT_URI,
            format!("http://127.0.0.1:{REDIRECT_PORT}/callback")
        );
    }

    #[test]
    fn sign_out_clears_separate_web_and_playback_entries_even_after_failure() {
        let mut names = Vec::new();
        let result = clear_entries(|name| {
            names.push(name.to_owned());
            if name == WEB_API_ENTRY {
                Err("locked".into())
            } else {
                Ok(())
            }
        });
        assert!(result.is_err());
        assert_eq!(names, ["spotify-oauth", "spotify-librespot-oauth"]);
        assert_ne!(WEB_API_ENTRY, PLAYBACK_ENTRY);
        assert!(clear_entries(|_| Ok(())).is_ok());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn blocking_keyring_runtime_does_not_nest_in_async_worker() {
        let result = secret_service(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap();
            Ok(runtime.block_on(async { 42 }))
        });
        assert_eq!(result.unwrap(), 42);
    }
}
