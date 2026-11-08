use super::{actor::safe, auth::Vault};
use crate::{
    contracts::{AppError, ErrorKind},
    domain::*,
};
use serde::Deserialize;
use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

const KEY_ENTRY: &str = "spicy-lyrics-key";

pub(super) struct Client {
    http: reqwest::Client,
    key: Option<String>,
}

impl Client {
    pub(super) fn new(key: Option<String>) -> Result<Self, AppError> {
        let http = super::http_client()
            .user_agent(concat!("Scytify/", env!("CARGO_PKG_VERSION")))
            .timeout(std::time::Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| safe(ErrorKind::Unavailable, "Lyrics HTTP client unavailable"))?;
        Ok(Self { http, key })
    }

    pub(super) fn load_key() -> Result<Option<String>, AppError> {
        Vault::load_entry(KEY_ENTRY).map_err(|e| safe(ErrorKind::CredentialStore, &e))
    }

    pub(super) fn has_key(&self) -> bool {
        self.key.is_some()
    }

    pub(super) fn set_key(&mut self, key: Option<String>) -> Result<(), AppError> {
        if let Some(key) = &key {
            validate_key(key)?;
            Vault::store_entry(KEY_ENTRY, key)
        } else {
            Vault::delete_entry(KEY_ENTRY)
        }
        .map_err(|e| safe(ErrorKind::CredentialStore, &e))?;
        self.key = key;
        Ok(())
    }

    pub(super) fn request(
        &self,
        track_id: &SpotifyId,
    ) -> Result<reqwest::RequestBuilder, AppError> {
        let key = self.key.as_deref().ok_or_else(|| {
            safe(
                ErrorKind::Authentication,
                "Add a Spicy Lyrics API key in Settings → Lyrics",
            )
        })?;
        if track_id.0.len() != 22 || !track_id.0.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(safe(ErrorKind::InvalidInput, "Invalid Spotify track ID"));
        }
        Ok(self
            .http
            .get(format!(
                "https://api.spicylyrics.org/v1/lyrics/{}",
                track_id.0
            ))
            .bearer_auth(key))
    }
}

fn validate_key(key: &str) -> Result<(), AppError> {
    if key.is_empty()
        || key.len() > 256
        || !key.is_ascii()
        || key
            .bytes()
            .any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
    {
        return Err(safe(
            ErrorKind::InvalidInput,
            "Invalid Spicy Lyrics API key",
        ));
    }
    Ok(())
}

/// Minimum spacing between requests to the lyrics provider, however many lookups are queued
/// (the lyrics panel, the "only songs with lyrics" filter), so a long list is checked slowly
/// instead of in a burst the provider would throttle.
const REQUEST_SPACING: Duration = Duration::from_millis(500);
/// Pause after a rate limit that names no wait of its own.
const DEFAULT_COOLDOWN: Duration = Duration::from_secs(60);

/// When the next request may go out, and until when the provider has asked us to stay away.
#[derive(Default)]
struct Gate {
    next: Option<Instant>,
    cooldown_until: Option<Instant>,
}

static GATE: Mutex<Gate> = Mutex::new(Gate {
    next: None,
    cooldown_until: None,
});

impl Gate {
    /// Claim the next send slot. Fails with the remaining wait while the provider has us
    /// cooling down, so queued lookups give up at once instead of holding backend jobs.
    fn reserve(&mut self, now: Instant) -> Result<Instant, AppError> {
        if let Some(until) = self.cooldown_until.filter(|until| *until > now) {
            let mut error = safe(
                ErrorKind::RateLimited,
                "Spicy Lyrics rate limit reached; retry later",
            );
            error.retry_after_seconds = Some((until - now).as_secs() + 1);
            return Err(error);
        }
        let slot = self.next.filter(|next| *next > now).unwrap_or(now);
        self.next = Some(slot + REQUEST_SPACING);
        Ok(slot)
    }

    fn rate_limited(&mut self, now: Instant, retry_after: Option<u64>) {
        let wait = retry_after.map_or(DEFAULT_COOLDOWN, Duration::from_secs);
        self.cooldown_until = Some(now + wait);
    }
}

pub(super) async fn fetch(
    request: reqwest::RequestBuilder,
    track_id: &SpotifyId,
) -> Result<Option<Lyrics>, AppError> {
    let slot = GATE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .reserve(Instant::now())?;
    tokio::time::sleep_until(slot.into()).await;
    let result = send(request, track_id).await;
    if let Err(error) = &result
        && error.kind == ErrorKind::RateLimited
    {
        GATE.lock()
            .unwrap_or_else(|e| e.into_inner())
            .rate_limited(Instant::now(), error.retry_after_seconds);
    }
    result
}

async fn send(
    request: reqwest::RequestBuilder,
    track_id: &SpotifyId,
) -> Result<Option<Lyrics>, AppError> {
    let response = request
        .send()
        .await
        .map_err(|_| safe(ErrorKind::Network, "Spicy Lyrics network request failed"))?;
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok());
    let http_status = response.status().as_u16();
    if !(200..300).contains(&http_status) && http_status != 404 {
        // Name the provider's own reason, so a rejected key reads differently from a throttle.
        let reason = response
            .bytes()
            .await
            .ok()
            .and_then(|bytes| error_code(&bytes));
        let mut error = check_status(http_status, retry_after).expect_err("an error status");
        if let Some(reason) = reason {
            error.message = format!("{} ({reason})", error.message);
        }
        return Err(error);
    }
    if !check_status(http_status, retry_after)? {
        return Ok(None);
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| safe(ErrorKind::Network, "Spicy Lyrics response download failed"))?;
    let envelope: Envelope = serde_json::from_slice(&bytes)
        .map_err(|_| safe(ErrorKind::Unavailable, "Invalid Spicy Lyrics response"))?;
    if !check_status(envelope.status, retry_after)? {
        return Ok(None);
    }
    parse(envelope.body, track_id).map(Some)
}

/// The short machine-readable `error` code of a provider error body, if it looks like one.
fn error_code(bytes: &[u8]) -> Option<String> {
    let body: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let code = body.get("Body")?.get("error")?.as_str()?;
    (code.len() <= 40 && code.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
        .then(|| code.to_owned())
}

fn check_status(status: u16, retry_after: Option<u64>) -> Result<bool, AppError> {
    let (kind, message) = match status {
        200..=299 => return Ok(true),
        404 => return Ok(false),
        401 => (
            ErrorKind::Authentication,
            "Check the Spicy Lyrics API key in Settings → Lyrics",
        ),
        403 => (
            ErrorKind::Permission,
            "The Spicy Lyrics key does not have permission to fetch lyrics",
        ),
        429 => (
            ErrorKind::RateLimited,
            "Spicy Lyrics rate limit reached; retry later",
        ),
        400 => (
            ErrorKind::InvalidInput,
            "Spicy Lyrics rejected the track request",
        ),
        _ => (
            ErrorKind::Unavailable,
            "Spicy Lyrics is unavailable; retry later",
        ),
    };
    let mut error = safe(kind, message);
    if matches!(status, 429 | 503) {
        error.retry_after_seconds = retry_after;
    }
    Err(error)
}

#[derive(Deserialize)]
struct Envelope {
    #[serde(rename = "Status")]
    status: u16,
    #[serde(rename = "Body")]
    body: serde_json::Value,
}

#[derive(Deserialize)]
struct Body {
    #[serde(default)]
    source: String,
    #[serde(rename = "UploadAttribution")]
    attribution: Option<Attribution>,
    #[serde(flatten)]
    lyrics: Content,
}

#[derive(Deserialize)]
#[serde(tag = "Type")]
enum Content {
    Syllable {
        #[serde(rename = "Content")]
        lines: Vec<SyllableContent>,
    },
    Line {
        #[serde(rename = "Content")]
        lines: Vec<LineContent>,
    },
    Static {
        #[serde(rename = "Lines")]
        lines: Vec<TextLine>,
    },
}

#[derive(Deserialize)]
#[serde(tag = "Type")]
enum SyllableContent {
    Vocal {
        #[serde(rename = "OppositeAligned", default)]
        opposite: bool,
        #[serde(rename = "Lead")]
        lead: VocalGroup,
        #[serde(rename = "Background", default)]
        background: Vec<VocalGroup>,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct VocalGroup {
    syllables: Vec<TimedSyllable>,
    start_time: f64,
    end_time: f64,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct TimedSyllable {
    text: String,
    start_time: f64,
    end_time: f64,
    #[serde(default)]
    is_part_of_word: bool,
}

#[derive(Deserialize)]
#[serde(tag = "Type")]
enum LineContent {
    Vocal {
        #[serde(rename = "OppositeAligned", default)]
        opposite: bool,
        #[serde(rename = "Text")]
        text: String,
        #[serde(rename = "StartTime")]
        start: f64,
        #[serde(rename = "EndTime")]
        end: f64,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct TextLine {
    #[serde(rename = "Text")]
    text: String,
}

#[derive(Deserialize)]
struct Attribution {
    #[serde(rename = "Uploader")]
    uploader: Option<ProviderContributor>,
    #[serde(rename = "Maker")]
    maker: Option<ProviderContributor>,
}

#[derive(Deserialize)]
struct ProviderContributor {
    username: String,
    url: String,
}
impl From<ProviderContributor> for Contributor {
    fn from(value: ProviderContributor) -> Self {
        Self {
            name: value.username,
            url: value.url,
        }
    }
}

fn milliseconds(seconds: f64) -> u64 {
    (seconds * 1000.0).round().max(0.0) as u64
}

fn parse(body: serde_json::Value, track_id: &SpotifyId) -> Result<Lyrics, AppError> {
    let body: Body = serde_json::from_value(body)
        .map_err(|_| safe(ErrorKind::Unavailable, "Invalid Spicy Lyrics content"))?;
    let source = match body.source.as_str() {
        "spicy_lyrics" => LyricsSource::Community,
        "apple_music" => LyricsSource::AppleMusic,
        "spotify" => LyricsSource::Spotify,
        _ => LyricsSource::Unknown,
    };
    let attribution = body
        .attribution
        .filter(|_| source == LyricsSource::Community);
    let (uploader, maker) = attribution
        .map(|a| (a.uploader.map(Into::into), a.maker.map(Into::into)))
        .unwrap_or_default();
    let (sync, lines) = match body.lyrics {
        Content::Syllable { lines } => (
            LyricsSync::Syllable,
            lines
                .into_iter()
                .filter_map(|line| {
                    let SyllableContent::Vocal {
                        opposite,
                        lead,
                        background,
                    } = line
                    else {
                        return None;
                    };
                    let start_ms = milliseconds(lead.start_time);
                    let end_ms = milliseconds(lead.end_time);
                    let convert = |s: TimedSyllable| Syllable {
                        text: s.text.trim().to_owned(),
                        start_ms: milliseconds(s.start_time),
                        end_ms: milliseconds(s.end_time),
                        joins_next: s.is_part_of_word,
                    };
                    Some(LyricsLine {
                        start_ms,
                        end_ms,
                        opposite,
                        lead: lead.syllables.into_iter().map(convert).collect(),
                        background: background
                            .into_iter()
                            .flat_map(|g| g.syllables)
                            .map(convert)
                            .collect(),
                    })
                })
                .collect::<Vec<_>>(),
        ),
        Content::Line { lines } => (
            LyricsSync::Line,
            lines
                .into_iter()
                .filter_map(|line| {
                    let LineContent::Vocal {
                        opposite,
                        text,
                        start,
                        end,
                    } = line
                    else {
                        return None;
                    };
                    Some(text_line(
                        text,
                        milliseconds(start),
                        milliseconds(end),
                        opposite,
                    ))
                })
                .collect(),
        ),
        Content::Static { lines } => (
            LyricsSync::Static,
            lines
                .into_iter()
                .map(|line| text_line(line.text, 0, 0, false))
                .collect(),
        ),
    };
    Ok(Lyrics {
        track_id: track_id.clone(),
        sync,
        lines: lines
            .into_iter()
            .filter(|line| {
                line.lead
                    .iter()
                    .chain(&line.background)
                    .any(|s| !s.text.is_empty())
            })
            .collect(),
        credit: LyricsCredit {
            source,
            uploader,
            maker,
        },
    })
}

fn text_line(text: String, start_ms: u64, end_ms: u64, opposite: bool) -> LyricsLine {
    LyricsLine {
        start_ms,
        end_ms,
        opposite,
        lead: vec![Syllable {
            text: text.trim().to_owned(),
            start_ms,
            end_ms,
            joins_next: false,
        }],
        background: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn track() -> SpotifyId {
        SpotifyId("0123456789012345678901".into())
    }

    #[test]
    fn parses_cached_syllable_shape_with_background_and_attribution() {
        // Shortened local Spotify cache payload, with synthetic text and attribution.
        let envelope: Envelope =
            serde_json::from_str(include_str!("fixtures/lyrics-syllable.json")).unwrap();
        let lyrics = parse(envelope.body, &track()).unwrap();
        assert_eq!(lyrics.track_id, track());
        assert_eq!(lyrics.sync, LyricsSync::Syllable);
        assert_eq!(lyrics.lines.len(), 1);
        let line = &lyrics.lines[0];
        assert_eq!(
            (line.start_ms, line.end_ms, line.opposite),
            (5698, 8263, true)
        );
        assert_eq!(
            line.lead[0],
            Syllable {
                text: "Hel".into(),
                start_ms: 5698,
                end_ms: 5884,
                joins_next: true
            }
        );
        assert_eq!(
            line.background
                .iter()
                .map(|s| s.text.as_str())
                .collect::<Vec<_>>(),
            ["oh", "yeah"]
        );
        assert_eq!(
            (line.background[0].start_ms, line.background[0].end_ms),
            (5701, 5899)
        );
        assert_eq!(lyrics.credit.source, LyricsSource::Community);
        assert_eq!(lyrics.credit.uploader.unwrap().name, "Uploader");
        assert_eq!(
            lyrics.credit.maker.unwrap().url,
            "https://spicylyrics.org/u/maker"
        );
    }

    #[test]
    fn parses_line_and_static_lyrics_and_unknown_sources() {
        let lyrics = parse(json!({"Type":"Line", "source":"apple_music", "Content":[
            {"Type":"Vocal", "Text":" Hello ", "StartTime":-2.0, "EndTime":1.2346, "OppositeAligned":true, "Translation":"ignored"},
            {"Type":"Vocal", "Text":" \n ", "StartTime":2, "EndTime":3}, {"Type":"Interlude"}
        ]}), &track()).unwrap();
        assert_eq!(lyrics.sync, LyricsSync::Line);
        assert_eq!(lyrics.credit.source, LyricsSource::AppleMusic);
        assert_eq!(lyrics.lines, [text_line("Hello".into(), 0, 1235, true)]);
        let lyrics = parse(json!({"Type":"Static", "source":"future_provider", "Lines":[{"Text":" Hello "},{"Text":"  "}], "UploadAttribution":{"Uploader":{"username":"ignored","url":"ignored"}}}), &track()).unwrap();
        assert_eq!(lyrics.sync, LyricsSync::Static);
        assert_eq!(lyrics.lines, [text_line("Hello".into(), 0, 0, false)]);
        assert_eq!(lyrics.credit.source, LyricsSource::Unknown);
        assert!(lyrics.credit.uploader.is_none());
        let lyrics = parse(
            json!({"Type":"Static", "source":"spotify", "Lines":[]}),
            &track(),
        )
        .unwrap();
        assert_eq!(lyrics.credit.source, LyricsSource::Spotify);
        assert_eq!(milliseconds(f64::NAN), 0);
        assert_eq!(milliseconds(-1.0), 0);
    }

    #[test]
    fn maps_provider_errors_without_echoing_provider_messages() {
        assert!(check_status(200, None).unwrap());
        assert!(!check_status(404, None).unwrap());
        for (status, kind) in [
            (400, ErrorKind::InvalidInput),
            (401, ErrorKind::Authentication),
            (403, ErrorKind::Permission),
            (429, ErrorKind::RateLimited),
            (500, ErrorKind::Unavailable),
            (502, ErrorKind::Unavailable),
            (503, ErrorKind::Unavailable),
        ] {
            let error = check_status(status, Some(17)).unwrap_err();
            assert_eq!(error.kind, kind);
            assert_eq!(
                error.retry_after_seconds,
                matches!(status, 429 | 503).then_some(17)
            );
        }
        assert!(
            check_status(401, None)
                .unwrap_err()
                .message
                .contains("Settings")
        );
    }

    #[test]
    fn validates_keys_and_track_ids_before_network_work() {
        let missing = Client::new(None).unwrap();
        assert_eq!(
            missing.request(&track()).err().unwrap().kind,
            ErrorKind::Authentication
        );
        let client = Client::new(Some("sl_sk_test".into())).unwrap();
        assert_eq!(
            client
                .request(&SpotifyId("../bad".into()))
                .err()
                .unwrap()
                .kind,
            ErrorKind::InvalidInput
        );
        let request = client.request(&track()).unwrap().build().unwrap();
        assert!(
            request
                .headers()
                .contains_key(reqwest::header::AUTHORIZATION)
        );
        assert!(!request.headers().contains_key(reqwest::header::ORIGIN));
        for invalid in ["", "sl_sk_a b", "sl_sk_\n", "é", "\0", &"x".repeat(257)] {
            assert_eq!(
                validate_key(invalid).unwrap_err().kind,
                ErrorKind::InvalidInput
            );
        }
        assert!(validate_key("sl_pk_example").is_ok());
    }
}
