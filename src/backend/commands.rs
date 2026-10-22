use super::actor::{Job, artwork, emit, load_current, login_ready, safe, sync_queue};
use super::{api, auth, playback, playback_auth, queue::Queue, storage};
use crate::{contracts::*, domain::*};
use std::collections::{HashMap, HashSet};
use tokio::task::JoinSet;

/// Concurrent artwork downloads. Headroom over the UI bound absorbs cancelled tasks that the
/// actor has aborted but not yet joined, so ordinary fast scrolling does not report a spurious busy error.
pub(super) const ARTWORK_JOBS: usize = 2 * crate::contracts::ARTWORK_IN_FLIGHT;

#[allow(clippy::too_many_arguments)]
/// Validates and dispatches one command, spawning bounded jobs for network work.
pub(super) fn handle_command(
    action: Action,
    id: RequestId,
    auth_epoch: u64,
    api: &api::Api,
    store: &storage::Storage,
    q: &mut Queue,
    state: &mut AppState,
    tokens: &mut Option<auth::Tokens>,
    player: &mut Option<playback::Playback>,
    player_events: &mut Option<librespot_playback::player::PlayerEventChannel>,
    metadata: &HashMap<SpotifyId, Track>,
    jobs: &mut JoinSet<Job>,
    art_jobs: &mut JoinSet<Job>,
    handles: &mut HashMap<RequestId, tokio::task::AbortHandle>,
    cancelled: &mut HashSet<RequestId>,
    login_ids: &mut HashSet<RequestId>,
    events: &tokio::sync::mpsc::Sender<Event>,
) -> Option<Result<Response, AppError>> {
    macro_rules! ok {
        () => {
            Some(Ok(Response::Accepted))
        };
    }
    if let Action::TransferPlayback { device_id, .. } = &action
        && device_id == super::transfer::LOCAL_DEVICE_ID
        && player.is_none()
    {
        return Some(Err(safe(
            ErrorKind::Authentication,
            "Enable playback & Spotify Connect in Settings → Account first",
        )));
    }
    if let Action::TransferPlayback { device_id, .. } = &action
        && player.as_ref().is_some_and(|p| p.connect.is_none())
        && state.connect.this_device_id.as_ref() == Some(device_id)
    {
        let Some(token) = tokens.as_ref() else {
            return Some(Err(safe(ErrorKind::Authentication, "sign in first")));
        };
        if jobs.len() >= 4 {
            return Some(Err(safe(ErrorKind::Unavailable, "backend is busy")));
        }
        let api = api.clone();
        let token = token.access_token.clone();
        let revision = state.player.queue_revision;
        let device = state.connect.active_device_id.clone();
        let h = jobs.spawn(async move {
            Job::LocalTransfer(
                id,
                auth_epoch,
                revision,
                device,
                api.transfer_local(&token).await,
            )
        });
        handles.insert(id, h);
        return None;
    }
    if super::remote::routes_remote(state, &action)
        || player.as_ref().is_some_and(|p| p.connect.is_some())
            && matches!(
                action,
                Action::AddToQueue(_) | Action::PlayNext(_) | Action::RemoveQueued { .. }
            )
    {
        let Some(token) = tokens.as_ref() else {
            return Some(Err(safe(ErrorKind::Authentication, "sign in first")));
        };
        if jobs.len() >= 4 {
            return Some(Err(safe(ErrorKind::Unavailable, "backend is busy")));
        }
        let access_token = token.access_token.clone();
        let api = api.clone();
        let device = if state.connect.is_remote() {
            state.connect.active_device_id.clone()
        } else {
            state
                .connect
                .active_device_id
                .clone()
                .or_else(|| state.connect.this_device_id.clone())
        };
        let transfer_target = if let Action::TransferPlayback { device_id, .. } = &action {
            if let Some(p) = player.as_ref().filter(|p| p.connect.is_none()) {
                p.pause();
                state.player.status = PlaybackStatus::Paused;
            }
            Some(device_id.clone())
        } else {
            None
        };
        let h = jobs.spawn(async move {
            let result = if matches!(action, Action::RefreshDevices) {
                api.observe(&access_token, true).await.map(Some)
            } else {
                api.control(&access_token, device.as_deref(), &action)
                    .await
                    .map(|_| None)
            };
            Job::RemoteCommand(id, auth_epoch, transfer_target, result)
        });
        handles.insert(id, h);
        return None;
    }
    if let Action::Fetch {
        query: Query::Queue { revision },
        page,
    } = &action
        && (state.connect.is_remote() || player.as_ref().is_some_and(|p| p.connect.is_some()))
    {
        return Some(super::remote::queue_page(state, *revision, *page));
    }
    if let Some(connect) = player.as_ref().and_then(|p| p.connect.as_ref())
        && matches!(
            action,
            Action::Resume
                | Action::Pause
                | Action::Next
                | Action::Previous
                | Action::Seek { .. }
                | Action::SetVolume(_)
                | Action::SetShuffle(_)
                | Action::SetRepeat(_)
        )
    {
        if matches!(action, Action::Resume)
            && state.connect.active_device_id.is_none()
            && q.snapshot.current.is_some()
        {
            load_current(q, state, player, true);
            return Some(Ok(Response::Accepted));
        }
        return Some(
            connect
                .control(&action)
                .map(|_| Response::Accepted)
                .map_err(|e| safe(ErrorKind::Unavailable, &e)),
        );
    }
    match action {
        Action::Configure(s) => match store.save_settings(&s) {
            Err(e) => Some(Err(safe(ErrorKind::InvalidInput, &e))),
            Ok(()) => {
                let client_changed = state.settings.client_id != s.client_id;
                state.settings = s;
                state.cache_limit_bytes = state.settings.cache_limit_bytes;
                if client_changed {
                    abort_login_jobs(login_ids, handles, cancelled);
                    if let Some(p) = player.take() {
                        p.stop();
                    }
                    *player_events = None;
                    let had_tokens = tokens.take().is_some();
                    if had_tokens && let Err(e) = auth::Vault::clear() {
                        state.auth = AuthState::Blocked { reason: e.clone() };
                        return Some(Err(safe(ErrorKind::CredentialStore, &e)));
                    }
                    state.auth = if state.settings.client_id.is_some() {
                        AuthState::SignedOut
                    } else {
                        AuthState::NeedsClientId
                    };
                }
                ok!()
            }
        },
        Action::AuthorizePlayback => {
            let AuthState::Ready { user_id } = &state.auth else {
                return Some(Err(safe(ErrorKind::Authentication, "sign in first")));
            };
            let user_id = user_id.clone();
            if matches!(state.connect.playback_auth, PlaybackAuth::Authorizing) {
                return Some(Err(safe(
                    ErrorKind::Unavailable,
                    "playback authorization is already in progress",
                )));
            }
            if jobs.len() >= 4 {
                return Some(Err(safe(ErrorKind::Unavailable, "backend is busy")));
            }
            let cid = state.settings.client_id.clone().unwrap_or_default();
            let volume = q.snapshot.volume;
            state.connect.playback_auth = PlaybackAuth::Authorizing;
            let h = jobs.spawn(async move {
                Job::PlaybackAuthorization(
                    id,
                    auth_epoch,
                    playback_auth::connect(cid, user_id, volume, true).await,
                )
            });
            handles.insert(id, h);
            login_ids.insert(id);
            None
        }
        Action::BeginLogin => {
            if matches!(state.auth, AuthState::Authorizing) {
                return Some(Err(safe(
                    ErrorKind::Unavailable,
                    "authorization is already in progress",
                )));
            }
            let Some(cid) = state.settings.client_id.clone() else {
                return Some(Err(safe(
                    ErrorKind::InvalidInput,
                    "configure a developer client ID first",
                )));
            };
            if jobs.len() >= 4 {
                return Some(Err(safe(ErrorKind::Unavailable, "backend is busy")));
            }
            state.auth = AuthState::Authorizing;
            let a = api.clone();
            let h = jobs.spawn(async move {
                Job::Login(
                    id,
                    auth_epoch,
                    async {
                        let t = auth::authorize(&cid)
                            .await
                            .map_err(|e| safe(ErrorKind::Authentication, &e))?;
                        login_ready(a, cid, t).await
                    }
                    .await,
                )
            });
            handles.insert(id, h);
            login_ids.insert(id);
            None
        }
        Action::CancelLogin => {
            for target in login_ids.drain().collect::<Vec<_>>() {
                if let Some(h) = handles.remove(&target) {
                    h.abort();
                    cancelled.insert(target);
                    emit(
                        events,
                        Event::Completed {
                            id: target,
                            result: Err(safe(ErrorKind::Authentication, "authorization cancelled")),
                        },
                    );
                }
            }
            if matches!(state.auth, AuthState::Ready { .. }) {
                if matches!(state.connect.playback_auth, PlaybackAuth::Authorizing) {
                    state.connect.playback_auth = PlaybackAuth::NeedsAuthorization;
                }
            } else {
                state.auth = if state.settings.client_id.is_some() {
                    AuthState::SignedOut
                } else {
                    AuthState::NeedsClientId
                };
            }
            ok!()
        }
        Action::CancelRequest(target) => {
            if let Some(h) = handles.remove(&target) {
                h.abort();
                cancelled.insert(target);
                if login_ids.remove(&target)
                    && matches!(state.connect.playback_auth, PlaybackAuth::Authorizing)
                {
                    state.connect.playback_auth = PlaybackAuth::NeedsAuthorization;
                }
                emit(
                    events,
                    Event::Completed {
                        id: target,
                        result: Err(safe(ErrorKind::Unavailable, "request cancelled")),
                    },
                );
            }
            ok!()
        }
        Action::Logout => {
            abort_login_jobs(login_ids, handles, cancelled);
            let cleared = auth::Vault::clear();
            state.connect = ConnectState::default();
            if let Some(p) = player.take() {
                p.stop();
            }
            *player_events = None;
            *tokens = None;
            *q = Queue::empty(rand::random());
            state.auth = if state.settings.client_id.is_some() {
                AuthState::SignedOut
            } else {
                AuthState::NeedsClientId
            };
            state.player.current = None;
            state.player.status = PlaybackStatus::Stopped;
            sync_queue(state, q);
            Some(
                cleared
                    .map(|()| Response::Accepted)
                    .map_err(|e| safe(ErrorKind::CredentialStore, &e)),
            )
        }
        Action::ClearCache => Some(
            store
                .clear_cache()
                .map(|_| {
                    state.cache_used_bytes = 0;
                    Response::Accepted
                })
                .map_err(|e| safe(ErrorKind::Internal, &e)),
        ),
        Action::Fetch { query, page } => {
            if page.limit == 0 || page.limit > MAX_PAGE_SIZE {
                return Some(Err(safe(ErrorKind::InvalidInput, "invalid page size")));
            }
            if let Query::Queue { revision } = query {
                if revision != state.player.queue_revision {
                    return Some(Err(safe(ErrorKind::StaleQueue, "queue changed")));
                }
                let all: Vec<_> = q
                    .snapshot
                    .explicit_queue
                    .iter()
                    .chain(q.snapshot.remaining.iter())
                    .cloned()
                    .collect();
                let start = (page.offset as usize).min(all.len());
                let end = (start + page.limit as usize).min(all.len());
                let slice = all[start..end].to_vec();
                if slice.iter().all(|e| metadata.contains_key(&e.track_id)) {
                    let items = slice
                        .iter()
                        .map(|e| QueueItem {
                            entry: e.clone(),
                            track: metadata.get(&e.track_id).cloned(),
                            explicit: q
                                .snapshot
                                .explicit_queue
                                .iter()
                                .any(|x| x.entry_id == e.entry_id),
                        })
                        .collect();
                    return Some(Ok(Response::Page(PageData::Queue {
                        revision,
                        page: Page {
                            offset: page.offset,
                            total: Some(all.len() as u64),
                            items,
                            has_more: end < all.len(),
                            next_offset: page.offset + slice.len() as u64,
                        },
                    })));
                }
                if jobs.len() >= 4 {
                    return Some(Err(safe(ErrorKind::Unavailable, "backend is busy")));
                }
                let ids = slice.iter().map(|e| e.track_id.clone()).collect::<Vec<_>>();
                let explicit = q
                    .snapshot
                    .explicit_queue
                    .iter()
                    .map(|e| e.entry_id)
                    .collect();
                let a = api.clone();
                let Some(t) = tokens.clone() else {
                    return Some(Err(safe(ErrorKind::Authentication, "sign in first")));
                };
                let h = jobs.spawn(async move {
                    let r = a.hydrate_tracks(&t.access_token, &ids).await;
                    Job::QueuePage(id, revision, page, all.len() as u64, slice, explicit, r)
                });
                handles.insert(id, h);
                return None;
            }
            let (Some(t), AuthState::Ready { user_id }) = (tokens.clone(), state.auth.clone())
            else {
                return Some(Err(safe(ErrorKind::Authentication, "sign in first")));
            };
            if jobs.len() >= 4 {
                return Some(Err(safe(ErrorKind::Unavailable, "backend is busy")));
            }
            let a = api.clone();
            let h = jobs.spawn(async move {
                Job::Response(
                    id,
                    a.fetch(&t.access_token, &user_id, &query, page)
                        .await
                        .map(Response::Page),
                )
            });
            handles.insert(id, h);
            None
        }
        Action::Artwork { url, .. } => {
            match store.cached_artwork(&url) {
                Ok(Some(bytes)) => return Some(Ok(Response::Artwork { bytes })),
                Err(e) => emit(
                    events,
                    Event::Notice(safe(
                        ErrorKind::Internal,
                        &format!("could not read artwork cache: {e}"),
                    )),
                ),
                Ok(None) => {}
            }
            if art_jobs.len() >= ARTWORK_JOBS {
                return Some(Err(safe(
                    ErrorKind::Unavailable,
                    "artwork downloads are busy",
                )));
            }
            let key = url.clone();
            let h = art_jobs.spawn(async move { Job::Artwork(id, key, artwork(&url).await) });
            handles.insert(id, h);
            None
        }
        Action::Play(PlaySource::Track(t)) => {
            let Some(tok) = tokens.clone() else {
                return Some(Err(safe(ErrorKind::Authentication, "sign in first")));
            };
            if jobs.len() >= 4 {
                return Some(Err(safe(ErrorKind::Unavailable, "backend is busy")));
            }
            let a = api.clone();
            let tid = t.clone();
            let revision = state.player.queue_revision;
            let device = state.connect.active_device_id.clone();
            let h = jobs.spawn(async move {
                let r = a
                    .hydrate_tracks(&tok.access_token, std::slice::from_ref(&tid))
                    .await;
                Job::Track(id, auth_epoch, revision, device, tid, r)
            });
            handles.insert(id, h);
            None
        }
        Action::Play(source) => {
            let Some(t) = tokens.clone() else {
                return Some(Err(safe(ErrorKind::Authentication, "sign in first")));
            };
            if jobs.len() >= 4 {
                return Some(Err(safe(ErrorKind::Unavailable, "backend is busy")));
            }
            let a = api.clone();
            let revision = state.player.queue_revision;
            let device = state.connect.active_device_id.clone();
            let h = jobs.spawn(async move {
                Job::Source(
                    id,
                    auth_epoch,
                    revision,
                    device,
                    a.load_source(&t.access_token, &source).await,
                )
            });
            handles.insert(id, h);
            None
        }
        Action::Resume => {
            if player.is_some() {
                // A restored session may have a connected player but no decoder loaded.
                load_current(q, state, player, true);
                ok!()
            } else {
                Some(Err(safe(
                    ErrorKind::Unavailable,
                    "audio session is not connected; select a track",
                )))
            }
        }
        Action::Pause => {
            if let Some(p) = player {
                p.pause();
            }
            state.player.status = PlaybackStatus::Paused;
            ok!()
        }
        Action::Next => {
            q.next(false);
            sync_queue(state, q);
            state.player.current = q
                .snapshot
                .current
                .as_ref()
                .and_then(|e| metadata.get(&e.track_id))
                .cloned();
            load_current(q, state, player, true);
            ok!()
        }
        Action::Previous => {
            q.previous();
            sync_queue(state, q);
            state.player.current = q
                .snapshot
                .current
                .as_ref()
                .and_then(|e| metadata.get(&e.track_id))
                .cloned();
            load_current(q, state, player, true);
            ok!()
        }
        Action::Seek { position_ms } => {
            let Some(p) = player.as_mut().filter(|_| q.snapshot.current.is_some()) else {
                return Some(Err(safe(
                    ErrorKind::Unavailable,
                    "No active decoder to seek",
                )));
            };
            let position_ms = position_ms.min(
                state
                    .player
                    .current
                    .as_ref()
                    .map_or(u64::from(u32::MAX), |track| track.duration_ms),
            );
            q.snapshot.position_ms = position_ms;
            state.player.position_ms = position_ms;
            p.seek(position_ms);
            ok!()
        }
        Action::SetVolume(v) => {
            if v.is_finite() && (0.0..=1.0).contains(&v) {
                q.snapshot.volume = v;
                state.player.volume = v;
                if let Some(p) = player {
                    p.set_volume(v);
                }
                ok!()
            } else {
                Some(Err(safe(
                    ErrorKind::InvalidInput,
                    "volume must be between zero and one",
                )))
            }
        }
        Action::SetShuffle(v) => {
            q.set_shuffle(v);
            sync_queue(state, q);
            ok!()
        }
        Action::SetRepeat(v) => {
            q.snapshot.repeat = v;
            state.player.repeat = v;
            ok!()
        }
        Action::PlayNext(v) => Some(
            q.enqueue(v, true)
                .map(|_| {
                    sync_queue(state, q);
                    Response::Accepted
                })
                .map_err(|e| safe(ErrorKind::InvalidInput, e)),
        ),
        Action::AddToQueue(v) => Some(
            q.enqueue(v, false)
                .map(|_| {
                    sync_queue(state, q);
                    Response::Accepted
                })
                .map_err(|e| safe(ErrorKind::InvalidInput, e)),
        ),
        Action::RemoveQueued { entry_id } => {
            if let Some(i) = q
                .snapshot
                .explicit_queue
                .iter()
                .position(|x| x.entry_id == entry_id)
            {
                if i < q.snapshot.play_next_count {
                    q.snapshot.play_next_count -= 1
                }
                q.snapshot.explicit_queue.remove(i);
                sync_queue(state, q);
                ok!()
            } else {
                Some(Err(safe(
                    ErrorKind::InvalidInput,
                    "queue occurrence not found",
                )))
            }
        }
        Action::SetVisible(_) => ok!(),
        Action::Quit
        | Action::TransferPlayback { .. }
        | Action::SetDeviceVolume { .. }
        | Action::RefreshDevices => unreachable!(),
    }
}

/// Aborts authorization jobs when account/client ownership is invalidated.
fn abort_login_jobs(
    login_ids: &mut HashSet<RequestId>,
    handles: &mut HashMap<RequestId, tokio::task::AbortHandle>,
    cancelled: &mut HashSet<RequestId>,
) {
    for id in login_ids.drain() {
        if let Some(handle) = handles.remove(&id) {
            handle.abort();
        }
        cancelled.insert(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn connect_queue_edits_never_mutate_the_local_fallback_queue() {
        let root =
            std::env::temp_dir().join(format!("custom-spotify-routing-{}", rand::random::<u64>()));
        let store = storage::Storage::open_at(&root).unwrap();
        let api = api::Api::new().unwrap();
        let mut queue = Queue::empty(1);
        let mut state = crate::backend::initial_state();
        state.connect.this_device_id = Some("this-app".into());
        let mut tokens = Some(auth::Tokens {
            access_token: "fictitious-unused-token".into(),
            refresh_token: None,
            expires_at: u64::MAX,
        });
        let mut player = None;
        let mut player_events = None;
        let metadata = HashMap::new();
        let mut jobs = JoinSet::new();
        let mut art_jobs = JoinSet::new();
        let mut handles = HashMap::new();
        let mut cancelled = HashSet::new();
        let mut login_ids = HashSet::new();
        let (events, _receiver) = tokio::sync::mpsc::channel(8);
        for remote in [false, true] {
            state.connect.active_device_id = Some(if remote { "phone" } else { "this-app" }.into());
            let result = handle_command(
                Action::PlayNext(vec![SpotifyId("0123456789012345678901".into())]),
                RequestId(1),
                0,
                &api,
                &store,
                &mut queue,
                &mut state,
                &mut tokens,
                &mut player,
                &mut player_events,
                &metadata,
                &mut jobs,
                &mut art_jobs,
                &mut handles,
                &mut cancelled,
                &mut login_ids,
                &events,
            );
            if remote {
                assert!(result.is_none());
                // Unsupported remote edits return before constructing an HTTP request.
                let job = jobs.join_next().await.unwrap().unwrap();
                assert!(matches!(
                    job,
                    Job::RemoteCommand(
                        _,
                        _,
                        _,
                        Err(AppError {
                            kind: ErrorKind::Unavailable,
                            ..
                        })
                    )
                ));
            } else {
                assert!(matches!(result, Some(Ok(Response::Accepted))));
            }
            assert_eq!(queue.snapshot.explicit_queue.len(), 1);
        }
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
}
