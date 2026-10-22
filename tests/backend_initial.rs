use custom_spotify::{
    backend::initial_state,
    domain::{AuthState, PlaybackStatus},
};

#[test]
fn initial_state_never_claims_credentials_or_tray() {
    let state = initial_state();
    assert!(matches!(state.auth, AuthState::NeedsClientId));
    assert_eq!(state.player.status, PlaybackStatus::Stopped);
    assert_eq!(state.cache_limit_bytes, 1_000_000_000);
    assert!(state.settings.client_id.is_none());
}
