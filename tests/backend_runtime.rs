use custom_spotify::{
    backend::initial_state,
    contracts::{self, Action, Command, RequestId},
};

#[tokio::test]
async fn bounded_command_transport_remains_non_blocking_when_full() {
    let (front, _back) = contracts::channels(initial_state());
    for n in 0..contracts::COMMAND_CAPACITY {
        front
            .commands
            .try_send(Command {
                id: RequestId(n as u64),
                action: Action::SetVisible(true),
            })
            .unwrap();
    }
    assert!(
        front
            .commands
            .try_send(Command {
                id: RequestId(999),
                action: Action::Quit
            })
            .is_err()
    );
}

#[test]
fn restored_public_startup_policy_does_not_claim_desktop_or_audio() {
    let state = initial_state();
    assert!(state.player.current.is_none());
    assert_eq!(state.player.volume, 1.0);
}
