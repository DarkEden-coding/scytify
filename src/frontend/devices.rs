//! Spotify Connect devices as the interface needs them: the current one, what the picker
//! offers, how a remote context matches the open page, and the held value of a slider that
//! sends to a remote device only when released.
use super::icons::Icon;
use crate::contracts::Query;
use crate::domain::{ConnectState, DeviceType, PlaybackDevice, RemoteSource};
use std::time::{Duration, Instant};

/// The device that is playing: the one named by `active_device_id`, else the one flagged active.
pub(super) fn active(connect: &ConnectState) -> Option<&PlaybackDevice> {
    connect
        .active_device_id
        .as_ref()
        .and_then(|id| {
            connect
                .devices
                .iter()
                .find(|device| device.id.as_ref() == Some(id))
        })
        .or_else(|| connect.devices.iter().find(|device| device.is_active))
}

/// Every device except the current one, in the backend's order.
pub(super) fn others(connect: &ConnectState) -> Vec<&PlaybackDevice> {
    let current: Option<&PlaybackDevice> = active(connect);
    connect
        .devices
        .iter()
        .filter(|device| current.is_none_or(|current| !std::ptr::eq(current, *device)))
        .collect()
}

/// Id to transfer to; `None` when Spotify forbids it or omits the id.
pub(super) fn transfer_id(device: &PlaybackDevice) -> Option<&str> {
    device.id.as_deref().filter(|_| !device.is_restricted)
}

/// Whether the volume control reaches the playing device.
pub(super) fn volume_supported(connect: &ConnectState) -> bool {
    !connect.is_remote() || active(connect).is_some_and(|device| device.supports_volume)
}

pub(super) fn icon(kind: &DeviceType) -> Icon {
    match kind {
        DeviceType::Computer => Icon::Laptop,
        DeviceType::Smartphone => Icon::Smartphone,
        DeviceType::Tablet => Icon::Tablet,
        DeviceType::Speaker => Icon::Speaker,
        DeviceType::Tv => Icon::Tv,
        DeviceType::Other(_) => Icon::Radio,
    }
}

/// Short subtitle of a device row.
pub(super) fn kind_label(device: &PlaybackDevice) -> &'static str {
    if device.is_this_app {
        return "This computer";
    }
    match device.device_type {
        DeviceType::Computer => "Computer",
        DeviceType::Smartphone => "Phone",
        DeviceType::Tablet => "Tablet",
        DeviceType::Speaker => "Speaker",
        DeviceType::Tv => "TV",
        DeviceType::Other(_) => "Spotify Connect",
    }
}

/// Footer text for a degraded connection, and whether it is an error. `Live` shows nothing.
pub(super) fn source_hint(source: &RemoteSource) -> Option<(&'static str, bool)> {
    match source {
        RemoteSource::Live => None,
        RemoteSource::Polling => Some(("Updating every second", false)),
        RemoteSource::Unavailable => Some(("Can't reach Spotify Connect", true)),
    }
}

/// Spotify URI of the context a page shows, when playing it can be recognized.
pub(super) fn context_uri(query: &Query) -> Option<String> {
    match query {
        Query::PlaylistTracks { playlist_id } => {
            Some(format!("spotify:playlist:{}", playlist_id.0))
        }
        Query::AlbumTracks { album_id } => Some(format!("spotify:album:{}", album_id.0)),
        _ => None,
    }
}

/// Whether `uri` is the Liked Songs collection (`spotify:user:<id>:collection`).
pub(super) fn is_liked_context(uri: &str) -> bool {
    uri.ends_with(":collection")
}

/// How long a released value stays on screen while the backend catches up.
const SETTLE: Duration = Duration::from_secs(2);

/// A slider value kept while it is dragged, and briefly after release, so the control does
/// not jump back to the old reading before the remote device reports the new one.
#[derive(Clone, Copy, Debug)]
pub(super) struct Held<T> {
    pub(super) value: T,
    released: Option<Instant>,
}

impl<T: Copy> Held<T> {
    pub(super) fn dragging(value: T) -> Self {
        Self {
            value,
            released: None,
        }
    }
    pub(super) fn is_dragging(&self) -> bool {
        self.released.is_none()
    }
    pub(super) fn release(&mut self) {
        self.released = Some(Instant::now());
    }
    /// The value to show, or `None` once the settle time has passed.
    pub(super) fn shown(&self) -> Option<T> {
        match self.released {
            Some(at) if at.elapsed() >= SETTLE => None,
            _ => Some(self.value),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::SpotifyId;

    fn device(id: Option<&str>, active: bool, this_app: bool) -> PlaybackDevice {
        PlaybackDevice {
            id: id.map(Into::into),
            name: "Device".into(),
            device_type: DeviceType::Speaker,
            is_active: active,
            is_this_app: this_app,
            volume_percent: Some(30),
            supports_volume: true,
            is_restricted: false,
        }
    }

    #[test]
    fn current_device_is_excluded_from_the_list() {
        let connect = ConnectState {
            active_device_id: Some("b".into()),
            devices: vec![
                device(Some("a"), false, true),
                device(Some("b"), true, false),
                device(None, false, false),
            ],
            ..ConnectState::default()
        };
        assert_eq!(active(&connect).unwrap().id.as_deref(), Some("b"));
        let ids: Vec<Option<&str>> = others(&connect).iter().map(|d| d.id.as_deref()).collect();
        assert_eq!(ids, [Some("a"), None]);
    }

    #[test]
    fn unusable_devices_cannot_be_transferred_to() {
        let mut restricted = device(Some("a"), false, false);
        restricted.is_restricted = true;
        assert_eq!(transfer_id(&restricted), None);
        assert_eq!(transfer_id(&device(None, false, false)), None);
        assert_eq!(transfer_id(&device(Some("a"), false, false)), Some("a"));
    }

    #[test]
    fn volume_follows_the_remote_device() {
        let mut remote = device(Some("b"), true, false);
        let mut connect = ConnectState {
            this_device_id: Some("a".into()),
            active_device_id: Some("b".into()),
            devices: vec![remote.clone()],
            ..ConnectState::default()
        };
        assert!(volume_supported(&connect));
        remote.supports_volume = false;
        connect.devices = vec![remote];
        assert!(!volume_supported(&connect));
        connect.active_device_id = Some("a".into());
        assert!(volume_supported(&connect), "local playback is unaffected");
    }

    #[test]
    fn pages_map_to_spotify_contexts() {
        let playlist = Query::PlaylistTracks {
            playlist_id: SpotifyId("abc".into()),
        };
        assert_eq!(
            context_uri(&playlist).as_deref(),
            Some("spotify:playlist:abc")
        );
        assert_eq!(context_uri(&Query::LikedTracks), None);
        assert!(is_liked_context("spotify:user:me:collection"));
    }

    #[test]
    fn released_values_settle() {
        let mut held = Held::dragging(0.5_f32);
        assert!(held.is_dragging() && held.shown() == Some(0.5));
        held.release();
        assert!(!held.is_dragging() && held.shown() == Some(0.5));
        held.released = Some(Instant::now() - SETTLE);
        assert_eq!(held.shown(), None);
    }
}
