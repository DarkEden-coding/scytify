use super::{api, auth, playback, playback_auth, queue::Queue, storage};
use crate::{contracts::*, domain::*};
use std::collections::{HashMap, HashSet};
use tokio::task::JoinSet;

type BootstrapResult = Result<(auth::Tokens, String, playback_auth::Setup, Vec<Track>), AppError>;

pub(super) enum Job {
    Login(
        RequestId,
        u64,
        Result<(auth::Tokens, String, playback_auth::Setup), AppError>,
    ),
    Bootstrap(u64, BootstrapResult),
    PlaybackAuthorization(RequestId, u64, playback_auth::Setup),
    Refresh(u64, Result<(auth::Tokens, String), AppError>),
    Response(RequestId, Result<Response, AppError>),
    Artwork(RequestId, String, Result<Vec<u8>, AppError>),
    QueuePage(
        RequestId,
        u64,
        PageRequest,
        u64,
        Vec<QueueEntry>,
        HashSet<u64>,
        Result<Vec<Track>, AppError>,
    ),
    Source(
        RequestId,
        u64,
        u64,
        Option<String>,
        Result<api::LoadedSource, AppError>,
    ),
    Track(
        RequestId,
        u64,
        u64,
        Option<String>,
        SpotifyId,
        Result<Vec<Track>, AppError>,
    ),
    Connect(RequestId, u64, Option<String>, playback_auth::Setup),
    CurrentMetadata(SpotifyId, Result<Vec<Track>, AppError>),
    RemotePoll(u64, u64, Result<super::remote::Observation, AppError>),
    RemoteCommand(
        RequestId,
        u64,
        Option<String>,
        Result<Option<super::remote::Observation>, AppError>,
    ),
    LocalTransfer(
        RequestId,
        u64,
        u64,
        Option<String>,
        Result<super::transfer::LocalTransfer, AppError>,
    ),
    ObservedMetadata(u64, Result<Vec<Track>, AppError>),
}

/// Runs the responsive mutable backend owner. Network/auth/session jobs are cancellable and
/// bounded; player events and commands continue to be consumed while those jobs are pending.
pub(super) async fn run(mut port: BackendPort) -> Result<(), AppError> {
    let store = storage::Storage::open().map_err(|e| safe(ErrorKind::Internal, &e))?;
    let settings = store
        .settings()
        .map_err(|e| safe(ErrorKind::Internal, &e))?;
    let mut q = match store.session().map_err(|e| safe(ErrorKind::Internal, &e))? {
        Some(s) => Queue::restore(s, rand::random()).map_err(|e| safe(ErrorKind::Internal, e))?,
        None => Queue::empty(rand::random()),
    };
    let api = api::Api::new()?;
    let mut tokens = auth::Vault::load().map_err(|e| safe(ErrorKind::CredentialStore, &e))?;
    let mut state = super::initial_state();
    state.settings = settings.clone();
    state.cache_limit_bytes = settings.cache_limit_bytes;
    store.measure_audio();
    state.cache_used_bytes = store
        .used_bytes()
        .map_err(|e| safe(ErrorKind::Internal, &e))?;
    playback::set_audio_cache(store.audio_dir(), state.cache_limit_bytes);
    state.player.volume = q.snapshot.volume;
    state.player.shuffle = q.snapshot.shuffle;
    state.player.repeat = q.snapshot.repeat;
    state.player.current_entry_id = q.snapshot.current.as_ref().map(|x| x.entry_id);
    state.player.status = if q.snapshot.current.is_some() {
        PlaybackStatus::Paused
    } else {
        PlaybackStatus::Stopped
    };
    state.auth = if settings.client_id.is_none() {
        AuthState::NeedsClientId
    } else if tokens.is_some() {
        AuthState::Authorizing
    } else {
        AuthState::SignedOut
    };
    let _ = port.state.send(state.clone());

    let mut jobs = JoinSet::new();
    // Thumbnail downloads have their own small budget so visible grids never starve browsing.
    let mut art_jobs = JoinSet::new();
    let mut handles: HashMap<RequestId, tokio::task::AbortHandle> = HashMap::new();
    let mut cancelled = HashSet::new();
    let mut login_ids = HashSet::new();
    let mut player: Option<playback::Playback> = None;
    let mut player_events: Option<librespot_playback::player::PlayerEventChannel> = None;
    let mut metadata: HashMap<SpotifyId, Track> = HashMap::new();
    let mut metadata_requested_for: Option<u64> = None;
    let mut auth_epoch = 0_u64;
    let mut background_auth: Option<tokio::task::AbortHandle> = None;
    if let (Some(t), Some(cid)) = (tokens.clone(), settings.client_id.clone()) {
        let a = api.clone();
        let current = q.snapshot.current.as_ref().map(|e| e.track_id.clone());
        background_auth =
            Some(jobs.spawn(async move {
                Job::Bootstrap(0, bootstrap_ready(a, cid, t, current).await)
            }));
    }
    let mut last_position_checkpoint = std::time::Instant::now();
    let mut refresh_tick = tokio::time::interval(std::time::Duration::from_secs(30));
    refresh_tick.tick().await;
    let mut observation_tick = tokio::time::interval(std::time::Duration::from_millis(100));
    observation_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut polling = super::remote::Polling::new();

    let mut logged_auth = (String::new(), PlaybackAuth::default());
    loop {
        let auth_now = (
            format!("{:?}", state.auth),
            state.connect.playback_auth.clone(),
        );
        if auth_now != logged_auth {
            log::info!("auth={} playback_auth={:?}", auth_now.0, auth_now.1);
            logged_auth = auth_now;
        }
        if !state.connect.is_remote()
            && player.as_ref().is_none_or(|p| p.connect.is_none())
            && let Some(entry) = &q.snapshot.current
            && state.player.current.is_none()
            && metadata_requested_for != Some(entry.entry_id)
            && jobs.len() < 4
            && let Some(token) = tokens.as_ref()
        {
            metadata_requested_for = Some(entry.entry_id);
            let track_id = entry.track_id.clone();
            let access_token = token.access_token.clone();
            let api = api.clone();
            jobs.spawn(async move {
                let result = api
                    .hydrate_tracks(&access_token, std::slice::from_ref(&track_id))
                    .await;
                Job::CurrentMetadata(track_id, result)
            });
        }
        tokio::select! {
            biased;
            command = port.commands.recv(), if port.events.capacity() > 0 || port.commands.is_closed() => {
                let Some(command) = command else {
                    jobs.abort_all(); if let Some(p) = &player { p.stop(); }
                    return store.save_session(&q.snapshot).map_err(|e| safe(ErrorKind::Internal, &e));
                };
                let id = command.id;
                if matches!(command.action, Action::Quit) {
                    jobs.abort_all(); if let Some(p) = &player { p.stop(); }
                    store.save_session(&q.snapshot).map_err(|e| safe(ErrorKind::Internal, &e))?;
                    emit_shutdown(&port.events, id).await; return Ok(());
                }
                let persist = action_changes_session(&command.action) && !state.connect.is_remote();
                if let Action::SetVisible(visible) = &command.action {
                    polling.visible = *visible;
                    if *visible { polling.next = std::time::Instant::now(); }
                }
                let invalidates_auth = matches!(&command.action, Action::Logout | Action::BeginLogin)
                    || matches!(&command.action, Action::Configure(settings) if settings.client_id != state.settings.client_id);
                if invalidates_auth {
                    auth_epoch = auth_epoch.wrapping_add(1);
                    jobs.abort_all();
                    for (request, handle) in handles.drain() {
                        handle.abort();
                        emit(&port.events, Event::Completed { id: request, result: Err(safe(ErrorKind::Authentication, "account changed; retry the request")) });
                    }
                    cancelled.clear();
                    login_ids.clear();
                    if let Some(h) = background_auth.take() { h.abort(); }
                    polling = super::remote::Polling::new();
                    state.connect = ConnectState::default();
                }
                let result = super::commands::handle_command(command.action, id, auth_epoch, &api, &store, &mut q, &mut state,
                    &mut tokens, &mut player, &mut player_events, &metadata, &mut jobs, &mut art_jobs, &mut handles, &mut cancelled, &mut login_ids, &port.events);
                if let Some(result) = result { emit(&port.events, Event::Completed { id, result }); }
                if persist { checkpoint(&store, &q, &port.events); }
                let _ = port.state.send(state.clone());
            }
            Some(joined) = jobs.join_next(), if !jobs.is_empty() => {
                if let Ok(job) = joined {
                    finish_job(job, auth_epoch, &store, &port.events, &mut q, &mut state, &mut tokens, &mut player,
                        &mut player_events, &mut metadata, &mut jobs, &mut handles, &mut cancelled, &mut polling);
                    if background_auth.as_ref().is_some_and(|h| h.is_finished()) { background_auth = None; }
                    let _ = port.state.send(state.clone());
                }
            }
            Some(joined) = art_jobs.join_next(), if !art_jobs.is_empty() => {
                if let Ok(job) = joined {
                    finish_job(job, auth_epoch, &store, &port.events, &mut q, &mut state, &mut tokens, &mut player,
                        &mut player_events, &mut metadata, &mut jobs, &mut handles, &mut cancelled, &mut polling);
                    let _ = port.state.send(state.clone());
                }
            }
            _ = observation_tick.tick() => {
                let mut live = None;
                let mut healthy = false;
                let mut spirc = false;
                if let Some(p) = player.as_mut()
                    && let Some(connect) = p.connect.as_mut() {
                        spirc = true;
                        healthy = connect.healthy();
                        state.connect.this_device_id = Some(connect.device_id.clone());
                        if connect.observations.has_changed().unwrap_or(false) {
                            live = connect.observations.borrow_and_update().clone();
                        }
                }
                if let Some(observation) = live {
                    polling.last_live = Some(std::time::Instant::now());
                    polling.revision = polling.revision.wrapping_add(1);
                    super::remote::apply(&mut state, observation, RemoteSource::Live, spirc);
                    hydrate_observation(&mut state, &metadata);
                    if spirc && !state.connect.is_remote() && q.observe_connect(&state.player, &state.connect.queue_preview).unwrap_or(false) {
                        checkpoint(&store, &q, &port.events);
                    }
                    if jobs.len() < 4 && let Some(token) = tokens.as_ref() {
                        let ids = observation_ids(&state, &metadata);
                        if !ids.is_empty() {
                            let a = api.clone(); let token = token.access_token.clone(); let epoch = auth_epoch;
                            jobs.spawn(async move { Job::ObservedMetadata(epoch, a.hydrate_tracks(&token, &ids).await) });
                        }
                    }
                    let _ = port.state.send(state.clone());
                }
                let recent = super::connect::live_recent(polling.last_live, healthy);
                if !recent && state.connect.source == RemoteSource::Live {
                    state.connect.source = RemoteSource::Polling;
                    let _ = port.state.send(state.clone());
                }
                if polling.due() && !recent && jobs.len() < 4
                    && matches!(state.auth, AuthState::Ready { .. })
                    && let Some(token) = tokens.as_ref() {
                    polling.pending = true;
                    let details = std::time::Instant::now() >= polling.details_at;
                    if details { polling.details_at = std::time::Instant::now() + std::time::Duration::from_secs(10); }
                    let a = api.clone(); let token = token.access_token.clone(); let epoch = auth_epoch; let revision = polling.revision;
                    jobs.spawn(async move { Job::RemotePoll(epoch, revision, a.observe(&token, details).await) });
                }
            }
            _ = refresh_tick.tick() => {
                if background_auth.is_none() && jobs.len() < 4
                    && let (Some(t), Some(cid)) = (tokens.clone().filter(auth::expiring), state.settings.client_id.clone()) {
                        let a=api.clone(); let epoch=auth_epoch;
                        background_auth = Some(jobs.spawn(async move{Job::Refresh(epoch, refresh_ready(a,cid,t).await)}));
                    }
            }
            _ = port.events.closed() => {
                jobs.abort_all(); if let Some(p) = &player { p.stop(); }
                return store.save_session(&q.snapshot).map_err(|e| safe(ErrorKind::Internal, &e));
            }
            event = async { match player_events.as_mut() { Some(rx) => rx.recv().await, None => std::future::pending().await } } => {
                match event {
                    Some(event) => {
                        let dirty = handle_player_event(event, &mut q, &mut state, player.as_ref(), &metadata, &port.events);
                        if dirty || last_position_checkpoint.elapsed() >= std::time::Duration::from_secs(30) {
                            checkpoint(&store, &q, &port.events); last_position_checkpoint = std::time::Instant::now();
                        }
                        let _ = port.state.send(state.clone());
                    }
                    None => {
                        player_events = None;
                        if let Some(p) = player.take() { p.stop(); }
                        state.connect.playback_auth = PlaybackAuth::Failed("Local audio session disconnected; enable playback again to reconnect".into());
                        state.connect.ensure_local_device(super::transfer::LOCAL_DEVICE_ID, super::connect::device_name(), state.player.volume);
                        if !state.connect.is_remote() { state.player.status = PlaybackStatus::Stopped; }
                        emit(&port.events, Event::Notice(safe(ErrorKind::Unavailable, "audio player event channel closed")));
                        let _ = port.state.send(state.clone());
                    }
                }
            }
            else => break,
        }
    }
    if let Some(p) = &player {
        p.stop();
    }
    store
        .save_session(&q.snapshot)
        .map_err(|e| safe(ErrorKind::Internal, &e))
}

#[allow(clippy::too_many_arguments)]
/// Applies one completed asynchronous job to actor-owned state.
fn finish_job(
    job: Job,
    auth_epoch: u64,
    store: &storage::Storage,
    events: &tokio::sync::mpsc::Sender<Event>,
    q: &mut Queue,
    state: &mut AppState,
    tokens: &mut Option<auth::Tokens>,
    player: &mut Option<playback::Playback>,
    player_events: &mut Option<librespot_playback::player::PlayerEventChannel>,
    metadata: &mut HashMap<SpotifyId, Track>,
    jobs: &mut JoinSet<Job>,
    handles: &mut HashMap<RequestId, tokio::task::AbortHandle>,
    cancelled: &mut HashSet<RequestId>,
    polling: &mut super::remote::Polling,
) {
    match job {
        Job::RemotePoll(epoch, revision, result) => {
            if epoch != auth_epoch {
                return;
            }
            polling.pending = false;
            if revision != polling.revision {
                return;
            }
            polling.finish(&result, &mut state.connect.source);
            match result {
                Ok(observation) => {
                    polling.notice_kind = None;
                    let was_remote = state.connect.is_remote();
                    super::remote::apply(
                        state,
                        observation,
                        RemoteSource::Polling,
                        player.as_ref().is_some_and(|p| p.connect.is_some()),
                    );
                    hydrate_observation(state, metadata);
                    if player.as_ref().is_some_and(|p| p.connect.is_some())
                        && !state.connect.is_remote()
                        && q.observe_connect(&state.player, &state.connect.queue_preview)
                            .unwrap_or(false)
                    {
                        checkpoint(store, q, events);
                    }
                    // Pause once when another device takes over, not on every poll.
                    if !was_remote
                        && state.connect.is_remote()
                        && let Some(p) = player.as_ref().filter(|p| p.connect.is_none())
                    {
                        p.pause();
                    }
                }
                Err(error) => {
                    if matches!(
                        error.kind,
                        ErrorKind::Permission | ErrorKind::Authentication
                    ) && polling.notice_kind != Some(error.kind)
                    {
                        polling.notice_kind = Some(error.kind);
                        emit(events, Event::Notice(error));
                    }
                }
            }
        }
        Job::LocalTransfer(id, epoch, revision, device, result) => {
            handles.remove(&id);
            if cancelled.remove(&id) || epoch != auth_epoch {
                return;
            }
            let result = result.and_then(|transfer| {
                if revision != state.player.queue_revision
                    || device != state.connect.active_device_id
                    || !player.as_ref().is_some_and(|p| p.connect.is_none())
                {
                    return Err(safe(
                        ErrorKind::StaleQueue,
                        "playback changed while transferring; retry",
                    ));
                }
                let source = transfer.source;
                let mut candidate = q.clone();
                candidate
                    .activate(
                        source.source_id,
                        source.snapshot_id,
                        source.tracks,
                        source.start,
                    )
                    .map_err(|e| safe(ErrorKind::InvalidInput, e))?;
                candidate.set_shuffle(transfer.shuffle);
                candidate.snapshot.repeat = transfer.repeat;
                candidate.snapshot.position_ms = transfer.position_ms;
                store
                    .save_session(&candidate.snapshot)
                    .map_err(|e| safe(ErrorKind::Internal, &e))?;
                *q = candidate;
                insert_metadata(
                    metadata,
                    source.metadata,
                    q.snapshot.current.as_ref().map(|e| &e.track_id),
                );
                state.connect.active_device_id = state.connect.this_device_id.clone();
                for device in &mut state.connect.devices {
                    device.is_active = device.is_this_app;
                }
                state.connect.context_uri = transfer.context_uri;
                state.connect.queue_preview.clear();
                sync_queue(state, q);
                state.player.current = q
                    .snapshot
                    .current
                    .as_ref()
                    .and_then(|e| metadata.get(&e.track_id))
                    .cloned();
                load_current(q, state, player, transfer.play);
                polling.revision = polling.revision.wrapping_add(1);
                Ok(Response::Accepted)
            });
            emit(events, Event::Completed { id, result });
        }
        Job::RemoteCommand(id, epoch, transfer_target, result) => {
            handles.remove(&id);
            if cancelled.remove(&id) || epoch != auth_epoch {
                return;
            }
            if let Ok(observation) = &result {
                if let Some(target) = transfer_target
                    && player.as_ref().is_some_and(|p| p.connect.is_none())
                {
                    state.player.status = PlaybackStatus::Paused;
                    state.player.current_entry_id = None;
                    state.connect.active_device_id = Some(target.clone());
                    for device in &mut state.connect.devices {
                        device.is_active = device.id.as_ref() == Some(&target);
                    }
                }
                if let Some(observation) = observation {
                    // Device/queue requests can finish after a newer dealer snapshot.
                    if observation.observed_at >= state.player.observed_at {
                        let source = if state.connect.source == RemoteSource::Live {
                            RemoteSource::Live
                        } else {
                            RemoteSource::Polling
                        };
                        super::remote::apply(
                            state,
                            observation.clone(),
                            source,
                            player.as_ref().is_some_and(|p| p.connect.is_some()),
                        );
                    }
                } else {
                    // A control may not produce a cluster update after a protocol failure.
                    polling.last_live = None;
                }
                polling.revision = polling.revision.wrapping_add(1);
                polling.next = std::time::Instant::now();
                polling.details_at = std::time::Instant::now();
            } else if let Err(error) = &result
                && error.kind == ErrorKind::RateLimited
            {
                polling.next = std::time::Instant::now()
                    + std::time::Duration::from_secs(error.retry_after_seconds.unwrap_or(5).max(1));
            }
            emit(
                events,
                Event::Completed {
                    id,
                    result: result.map(|_| Response::Accepted),
                },
            );
        }
        Job::ObservedMetadata(epoch, result) => {
            if epoch != auth_epoch {
                return;
            }
            if let Ok(tracks) = result {
                insert_metadata(
                    metadata,
                    tracks,
                    state.player.current.as_ref().map(|t| &t.id),
                );
                hydrate_observation(state, metadata);
            }
        }
        Job::CurrentMetadata(track_id, result) => {
            if !state.connect.is_remote()
                && player.as_ref().is_none_or(|p| p.connect.is_none())
                && q.snapshot
                    .current
                    .as_ref()
                    .is_some_and(|entry| entry.track_id == track_id)
            {
                match result {
                    Ok(tracks) => {
                        insert_metadata(metadata, tracks, Some(&track_id));
                        state.player.current = metadata.get(&track_id).cloned();
                    }
                    Err(error) => emit(events, Event::Notice(error)),
                }
            }
        }
        Job::Response(id, r) => {
            handles.remove(&id);
            if !cancelled.remove(&id) {
                emit(events, Event::Completed { id, result: r });
            }
        }
        Job::Artwork(id, key, r) => {
            handles.remove(&id);
            if cancelled.remove(&id) {
                return;
            }
            if let Ok(bytes) = &r {
                let budget: u64 = storage::artwork_budget(state.cache_limit_bytes);
                if let Err(e) = store.cache_artwork(&key, bytes, budget) {
                    emit(
                        events,
                        Event::Notice(safe(
                            ErrorKind::Internal,
                            &format!("could not cache artwork: {e}"),
                        )),
                    );
                }
                state.cache_used_bytes = store.used_bytes().unwrap_or(state.cache_used_bytes);
            }
            emit(
                events,
                Event::Completed {
                    id,
                    result: r.map(|bytes| Response::Artwork { bytes }),
                },
            );
        }
        Job::QueuePage(id, revision, page, total, entries, explicit, r) => {
            handles.remove(&id);
            if cancelled.remove(&id) {
                return;
            }
            match r {
                Err(e) => emit(events, Event::Completed { id, result: Err(e) }),
                Ok(xs) => {
                    insert_metadata(
                        metadata,
                        xs,
                        q.snapshot.current.as_ref().map(|e| &e.track_id),
                    );
                    if revision != state.player.queue_revision {
                        emit(
                            events,
                            Event::Completed {
                                id,
                                result: Err(safe(ErrorKind::StaleQueue, "queue changed")),
                            },
                        )
                    } else {
                        let next_offset: u64 = page.offset.saturating_add(entries.len() as u64);
                        let items = entries
                            .iter()
                            .map(|e| QueueItem {
                                entry: e.clone(),
                                track: metadata.get(&e.track_id).cloned(),
                                explicit: explicit.contains(&e.entry_id),
                            })
                            .collect();
                        emit(
                            events,
                            Event::Completed {
                                id,
                                result: Ok(Response::Page(PageData::Queue {
                                    revision,
                                    page: Page {
                                        offset: page.offset,
                                        total: Some(total),
                                        has_more: next_offset < total,
                                        items,
                                        next_offset,
                                    },
                                })),
                            },
                        )
                    }
                }
            }
        }
        Job::Source(id, epoch, revision, device, r) => {
            handles.remove(&id);
            if cancelled.remove(&id) {
                return;
            }
            if epoch != auth_epoch
                || revision != state.player.queue_revision
                || device != state.connect.active_device_id
            {
                emit(
                    events,
                    Event::Completed {
                        id,
                        result: Err(safe(
                            ErrorKind::StaleQueue,
                            "playback changed while the source was loading; retry",
                        )),
                    },
                );
                return;
            }
            match r {
                Err(e) => emit(events, Event::Completed { id, result: Err(e) }),
                Ok(s) => {
                    let selected = s.tracks.get(s.start).cloned();
                    insert_metadata(metadata, s.metadata, selected.as_ref());
                    let mut candidate = q.clone();
                    if let Err(e) =
                        candidate.activate(s.source_id, s.snapshot_id, s.tracks, s.start)
                    {
                        emit(
                            events,
                            Event::Completed {
                                id,
                                result: Err(safe(ErrorKind::InvalidInput, e)),
                            },
                        );
                        return;
                    }
                    if let Err(e) = store.save_session(&candidate.snapshot) {
                        emit(
                            events,
                            Event::Completed {
                                id,
                                result: Err(safe(
                                    ErrorKind::Internal,
                                    &format!("could not save playback session: {e}"),
                                )),
                            },
                        );
                        return;
                    }
                    *q = candidate;
                    sync_queue(state, q);
                    state.player.current = q
                        .snapshot
                        .current
                        .as_ref()
                        .and_then(|e| metadata.get(&e.track_id))
                        .cloned();
                    let r = start_current(id, auth_epoch, q, state, tokens, player, jobs, handles);
                    if let Some(result) = r {
                        emit(events, Event::Completed { id, result });
                    }
                }
            }
        }
        Job::Track(id, epoch, revision, device, track, r) => {
            handles.remove(&id);
            if cancelled.remove(&id) {
                return;
            }
            if epoch != auth_epoch
                || revision != state.player.queue_revision
                || device != state.connect.active_device_id
            {
                emit(
                    events,
                    Event::Completed {
                        id,
                        result: Err(safe(
                            ErrorKind::StaleQueue,
                            "playback changed while the source was loading; retry",
                        )),
                    },
                );
                return;
            }
            match r {
                Err(e) => emit(events, Event::Completed { id, result: Err(e) }),
                Ok(xs) => {
                    insert_metadata(metadata, xs, Some(&track));
                    if !metadata.contains_key(&track) {
                        emit(
                            events,
                            Event::Completed {
                                id,
                                result: Err(safe(
                                    ErrorKind::Unavailable,
                                    "Spotify track is unavailable",
                                )),
                            },
                        );
                        return;
                    }
                    let mut candidate = q.clone();
                    if let Err(e) = candidate.activate_track(track) {
                        emit(
                            events,
                            Event::Completed {
                                id,
                                result: Err(safe(ErrorKind::InvalidInput, e)),
                            },
                        );
                        return;
                    }
                    if let Err(e) = store.save_session(&candidate.snapshot) {
                        emit(
                            events,
                            Event::Completed {
                                id,
                                result: Err(safe(
                                    ErrorKind::Internal,
                                    &format!("could not save playback session: {e}"),
                                )),
                            },
                        );
                        return;
                    }
                    *q = candidate;
                    sync_queue(state, q);
                    state.player.current = q
                        .snapshot
                        .current
                        .as_ref()
                        .and_then(|e| metadata.get(&e.track_id))
                        .cloned();
                    if let Some(result) =
                        start_current(id, auth_epoch, q, state, tokens, player, jobs, handles)
                    {
                        emit(events, Event::Completed { id, result });
                    }
                }
            }
        }
        Job::Connect(id, epoch, device, r) => {
            handles.remove(&id);
            if cancelled.remove(&id) || epoch != auth_epoch {
                return;
            }
            if device != state.connect.active_device_id {
                emit(
                    events,
                    Event::Completed {
                        id,
                        result: Err(safe(
                            ErrorKind::StaleQueue,
                            "active device changed while connecting; retry",
                        )),
                    },
                );
                return;
            }
            match install_playback(r, state, events) {
                Ok(mut p) => {
                    install_connect_state(&p, state, events);
                    *player_events = p.take_events();
                    *player = Some(p);
                    load_current(q, state, player, true);
                    emit(
                        events,
                        Event::Completed {
                            id,
                            result: Ok(Response::Accepted),
                        },
                    );
                }
                Err(e) => {
                    state.player.status = PlaybackStatus::Stopped;
                    emit(
                        events,
                        Event::Completed {
                            id,
                            result: Err(safe(ErrorKind::Unavailable, &e)),
                        },
                    );
                }
            }
        }
        Job::PlaybackAuthorization(id, epoch, setup) => {
            handles.remove(&id);
            if cancelled.remove(&id) || epoch != auth_epoch {
                return;
            }
            let result = install_playback(setup, state, events)
                .map(|mut p| {
                    install_connect_state(&p, state, events);
                    *player_events = p.take_events();
                    if let Some(old) = player.take() {
                        old.stop();
                    }
                    *player = Some(p);
                    Response::Accepted
                })
                .map_err(|e| safe(ErrorKind::Unavailable, &e));
            emit(events, Event::Completed { id, result });
        }
        Job::Login(id, epoch, r) => {
            handles.remove(&id);
            if cancelled.remove(&id) || epoch != auth_epoch {
                return;
            }
            apply_login(r, Some(id), events, state, tokens, player, player_events, q)
        }
        Job::Bootstrap(epoch, r) => {
            if epoch == auth_epoch {
                match r {
                    Err(e) => apply_login(
                        Err(e),
                        None,
                        events,
                        state,
                        tokens,
                        player,
                        player_events,
                        q,
                    ),
                    Ok((t, uid, p, tracks)) => {
                        insert_metadata(
                            metadata,
                            tracks,
                            q.snapshot.current.as_ref().map(|e| &e.track_id),
                        );
                        apply_login(
                            Ok((t, uid, p)),
                            None,
                            events,
                            state,
                            tokens,
                            player,
                            player_events,
                            q,
                        );
                        state.player.current = q
                            .snapshot
                            .current
                            .as_ref()
                            .and_then(|e| metadata.get(&e.track_id))
                            .cloned();
                    }
                }
            }
        }
        Job::Refresh(epoch, r) => {
            if epoch != auth_epoch {
                return;
            }
            match r {
                Err(e) => emit(events, Event::Notice(e)),
                Ok((t, uid)) => match auth::Vault::store(&t) {
                    Err(e) => emit(events, Event::Notice(safe(ErrorKind::CredentialStore, &e))),
                    Ok(()) => {
                        *tokens = Some(t);
                        state.auth = AuthState::Ready { user_id: uid };
                    }
                },
            }
        }
    }
}

/// Persists successful authorization and installs any connected player.
#[allow(
    clippy::too_many_arguments,
    reason = "Install auth and decoder state together on their single owner."
)]
fn apply_login(
    r: Result<(auth::Tokens, String, playback_auth::Setup), AppError>,
    id: Option<RequestId>,
    events: &tokio::sync::mpsc::Sender<Event>,
    state: &mut AppState,
    tokens: &mut Option<auth::Tokens>,
    player: &mut Option<playback::Playback>,
    player_events: &mut Option<librespot_playback::player::PlayerEventChannel>,
    q: &Queue,
) {
    match r {
        Err(e) => {
            state.auth = AuthState::SignedOut;
            if let Some(id) = id {
                emit(events, Event::Completed { id, result: Err(e) })
            } else {
                emit(events, Event::Notice(e))
            }
        }
        Ok((t, uid, p)) => {
            if let Err(e) = auth::Vault::store(&t) {
                let e = safe(ErrorKind::CredentialStore, &e);
                state.auth = AuthState::Blocked {
                    reason: e.message.clone(),
                };
                if let Some(id) = id {
                    emit(events, Event::Completed { id, result: Err(e) })
                } else {
                    emit(events, Event::Notice(e))
                }
                return;
            }
            *tokens = Some(t);
            state.auth = AuthState::Ready { user_id: uid };
            if let Ok(mut p) = install_playback(p, state, events) {
                p.set_volume(q.snapshot.volume);
                install_connect_state(&p, state, events);
                *player_events = p.take_events();
                if let Some(old) = player.take() {
                    old.stop();
                }
                *player = Some(p);
                if q.snapshot.current.is_some()
                    && player.as_ref().is_none_or(|p| p.connect.is_none())
                {
                    load_current(q, state, player, false);
                }
            }
            if let Some(id) = id {
                emit(
                    events,
                    Event::Completed {
                        id,
                        result: Ok(Response::Accepted),
                    },
                )
            }
        }
    }
}

/// Performs startup login and hydrates the restored current item before exposing state.
async fn bootstrap_ready(
    api: api::Api,
    client_id: String,
    t: auth::Tokens,
    current: Option<SpotifyId>,
) -> BootstrapResult {
    let (t, uid, player) = login_ready(api.clone(), client_id, t).await?;
    let tracks = match current {
        Some(id) => api
            .hydrate_tracks(&t.access_token, &[id])
            .await
            .unwrap_or_default(),
        None => Vec::new(),
    };
    Ok((t, uid, player, tracks))
}

/// Refreshes credentials, verifies identity, and independently attempts audio connection.
pub(super) async fn login_ready(
    api: api::Api,
    client_id: String,
    mut t: auth::Tokens,
) -> Result<(auth::Tokens, String, playback_auth::Setup), AppError> {
    if auth::expiring(&t) {
        t = auth::refresh(&client_id, &t)
            .await
            .map_err(|e| safe(ErrorKind::Authentication, &e))?;
    }
    let identity = api.identity(&t.access_token).await?;
    let p = playback_auth::connect(client_id, identity.clone(), 1.0, false).await;
    Ok((t, identity, p))
}

/// Refreshes and verifies credentials without replacing a live decoder/output session.
async fn refresh_ready(
    api: api::Api,
    client_id: String,
    mut t: auth::Tokens,
) -> Result<(auth::Tokens, String), AppError> {
    if auth::expiring(&t) {
        t = auth::refresh(&client_id, &t)
            .await
            .map_err(|e| safe(ErrorKind::Authentication, &e))?;
    }
    let identity = api.identity(&t.access_token).await?;
    Ok((t, identity))
}

/// Starts the selected occurrence or launches a cancellable session connection.
#[allow(clippy::too_many_arguments)]
pub(super) fn start_current(
    id: RequestId,
    auth_epoch: u64,
    q: &Queue,
    state: &mut AppState,
    tokens: &Option<auth::Tokens>,
    player: &mut Option<playback::Playback>,
    jobs: &mut JoinSet<Job>,
    handles: &mut HashMap<RequestId, tokio::task::AbortHandle>,
) -> Option<Result<Response, AppError>> {
    if player.is_some() {
        load_current(q, state, player, true);
        return Some(Ok(Response::Accepted));
    }
    let (Some(_), Some(cid)) = (tokens.as_ref(), state.settings.client_id.clone()) else {
        return Some(Err(safe(ErrorKind::Authentication, "sign in first")));
    };
    if !matches!(state.connect.playback_auth, PlaybackAuth::Ready) {
        return Some(Err(safe(
            ErrorKind::Authentication,
            "Enable playback & Spotify Connect in Settings → Account first",
        )));
    }
    state.player.status = PlaybackStatus::Loading;
    let user_id = match &state.auth {
        AuthState::Ready { user_id } => user_id.clone(),
        _ => return Some(Err(safe(ErrorKind::Authentication, "sign in first"))),
    };
    let volume = q.snapshot.volume;
    let device = state.connect.active_device_id.clone();
    let h = jobs.spawn(async move {
        Job::Connect(
            id,
            auth_epoch,
            device,
            playback_auth::connect(cid, user_id, volume, false).await,
        )
    });
    handles.insert(id, h);
    None
}
/// Loads the queue current occurrence and prefetches its successor.
pub(super) fn load_current(
    q: &Queue,
    state: &mut AppState,
    player: &mut Option<playback::Playback>,
    play: bool,
) {
    if let Some(connect) = player.as_ref().and_then(|p| p.connect.as_ref()) {
        let tracks = q
            .snapshot
            .current
            .iter()
            .chain(q.snapshot.explicit_queue.iter())
            .chain(q.snapshot.remaining.iter())
            .map(|e| e.track_id.clone())
            .collect::<Vec<_>>();
        if !tracks.is_empty() {
            let _ = connect.load(tracks, 0, q.snapshot.position_ms, play);
            let _ = connect.control(&Action::SetShuffle(q.snapshot.shuffle));
            let _ = connect.control(&Action::SetRepeat(q.snapshot.repeat));
        }
        return;
    }
    if let Some(entry) = q.snapshot.current.as_ref() {
        if play {
            state.connect.active_device_id = state.connect.this_device_id.clone();
            for device in &mut state.connect.devices {
                device.is_active = device.is_this_app;
            }
        }
        state.player.current_entry_id = Some(entry.entry_id);
        state.player.position_ms = q.snapshot.position_ms;
        state.player.status = PlaybackStatus::Loading;
        if let Some(p) = player
            && p.load(&entry.track_id, q.snapshot.position_ms, play)
                .is_ok()
            && let Some(next) = q
                .snapshot
                .explicit_queue
                .first()
                .or_else(|| q.snapshot.remaining.first())
        {
            let _ = p.preload(&next.track_id);
        }
    } else {
        state.player.current_entry_id = None;
        state.player.current = None;
        state.player.status = PlaybackStatus::Stopped;
        if let Some(p) = player {
            p.stop();
        }
    }
    state.player.observed_at = std::time::Instant::now();
}
/// Reduces a librespot event and advances natural playback when required.
fn handle_player_event(
    event: librespot_playback::player::PlayerEvent,
    q: &mut Queue,
    state: &mut AppState,
    player: Option<&playback::Playback>,
    metadata: &HashMap<SpotifyId, Track>,
    events: &tokio::sync::mpsc::Sender<Event>,
) -> bool {
    if state.connect.is_remote() {
        return false;
    }
    if let Some(player) = player.filter(|p| p.connect.is_some()) {
        // Spirc owns load generations and queue advancement. Decoder events provide
        // precise local anchors without feeding terminal events into the fallback queue.
        if let Some(update) = player.update(event) {
            match update {
                playback::Update::Loading(ms) => {
                    state.player.status = PlaybackStatus::Loading;
                    state.player.position_ms = ms;
                }
                playback::Update::Playing(ms) => {
                    state.player.status = PlaybackStatus::Playing;
                    state.player.position_ms = ms;
                }
                playback::Update::Paused(ms) => {
                    state.player.status = PlaybackStatus::Paused;
                    state.player.position_ms = ms;
                }
                playback::Update::Position(ms) => state.player.position_ms = ms,
                playback::Update::Seeked(ms) => {
                    state.player.position_ms = ms;
                    state.player.seek_revision = state.player.seek_revision.wrapping_add(1);
                }
                playback::Update::End | playback::Update::Unavailable => return false,
            }
            state.player.observed_at = std::time::Instant::now();
            if q.snapshot.current.as_ref().map(|e| &e.track_id)
                == state.player.current.as_ref().map(|t| &t.id)
            {
                q.snapshot.position_ms = state.player.position_ms;
            }
        }
        return false;
    }
    let Some(update) = player.and_then(|p| p.update(event)) else {
        return false;
    };
    let mut queue_dirty = false;
    match Some(update) {
        Some(playback::Update::Loading(ms)) => {
            state.player.status = PlaybackStatus::Loading;
            state.player.position_ms = ms
        }
        Some(playback::Update::Playing(ms)) => {
            state.player.status = PlaybackStatus::Playing;
            state.player.position_ms = ms
        }
        Some(playback::Update::Paused(ms)) => {
            state.player.status = PlaybackStatus::Paused;
            state.player.position_ms = ms
        }
        Some(playback::Update::Position(ms)) => {
            q.snapshot.position_ms = ms;
            state.player.position_ms = ms
        }
        Some(playback::Update::Seeked(ms)) => {
            q.snapshot.position_ms = ms;
            state.player.position_ms = ms;
            state.player.seek_revision = state.player.seek_revision.wrapping_add(1);
        }
        Some(playback::Update::End) => {
            queue_dirty = true;
            q.next(true);
            sync_queue(state, q);
            state.player.current = q
                .snapshot
                .current
                .as_ref()
                .and_then(|e| metadata.get(&e.track_id))
                .cloned();
            if let Some(p) = player {
                if let Some(e) = q.snapshot.current.as_ref() {
                    let _ = p.load(&e.track_id, 0, true);
                } else {
                    state.player.status = PlaybackStatus::Stopped;
                }
            }
        }
        Some(playback::Update::Unavailable) => {
            queue_dirty = true;
            emit(
                events,
                Event::Notice(safe(
                    ErrorKind::Unavailable,
                    "Spotify reported this track unavailable",
                )),
            );
            q.next(true);
            sync_queue(state, q);
            state.player.current = q
                .snapshot
                .current
                .as_ref()
                .and_then(|e| metadata.get(&e.track_id))
                .cloned();
            if let (Some(p), Some(e)) = (player, q.snapshot.current.as_ref()) {
                let _ = p.load(&e.track_id, 0, true);
            }
        }
        None => {}
    }
    state.player.observed_at = std::time::Instant::now();
    queue_dirty
}

/// Copies queue anchors into observable state and increments its revision.
pub(super) fn sync_queue(s: &mut AppState, q: &Queue) {
    s.player.current_entry_id = q.snapshot.current.as_ref().map(|x| x.entry_id);
    s.player.shuffle = q.snapshot.shuffle;
    s.player.repeat = q.snapshot.repeat;
    s.player.queue_revision = s.player.queue_revision.wrapping_add(1);
    s.player.observed_at = std::time::Instant::now()
}
/// Persists queue state and emits an actionable notice on failure.
fn checkpoint(store: &storage::Storage, q: &Queue, events: &tokio::sync::mpsc::Sender<Event>) {
    if let Err(e) = store.save_session(&q.snapshot) {
        emit(
            events,
            Event::Notice(safe(
                ErrorKind::Internal,
                &format!("could not save playback session: {e}"),
            )),
        )
    }
}
/// Sends without blocking audio handling. Command intake is gated when the event queue is full,
/// bounding the number of short-lived completion sender tasks.
pub(super) fn emit(tx: &tokio::sync::mpsc::Sender<Event>, event: Event) {
    match &event {
        Event::Notice(e) => log::warn!("notice: {:?}: {}", e.kind, e.message),
        Event::Completed { id, result: Err(e) } => {
            log::warn!("request {id:?} failed: {:?}: {}", e.kind, e.message)
        }
        _ => {}
    }
    if let Err(tokio::sync::mpsc::error::TrySendError::Full(event)) = tx.try_send(event) {
        let tx = tx.clone();
        tokio::spawn(async move {
            let _ = tx.send(event).await;
        });
    }
}

/// Delivers completion and QuitReady in strict order after audio/jobs have stopped.
async fn emit_shutdown(tx: &tokio::sync::mpsc::Sender<Event>, id: RequestId) {
    let _ = tx
        .send(Event::Completed {
            id,
            result: Ok(Response::Accepted),
        })
        .await;
    let _ = tx.send(Event::QuitReady).await;
}

/// Returns whether an accepted command changes the durable playback checkpoint.
fn action_changes_session(action: &Action) -> bool {
    matches!(
        action,
        Action::Logout
            | Action::Next
            | Action::Previous
            | Action::Seek { .. }
            | Action::SetVolume(_)
            | Action::SetShuffle(_)
            | Action::SetRepeat(_)
            | Action::PlayNext(_)
            | Action::AddToQueue(_)
            | Action::RemoveQueued { .. }
    )
}

/// Adds hydrated tracks while keeping actor memory independent of library/source size.
fn insert_metadata(
    map: &mut HashMap<SpotifyId, Track>,
    tracks: Vec<Track>,
    protected: Option<&SpotifyId>,
) {
    const MAX_METADATA: usize = 512;
    for track in tracks {
        map.insert(track.id.clone(), track);
    }
    let mut remaining =
        MAX_METADATA - usize::from(protected.is_some_and(|id| map.contains_key(id)));
    map.retain(|id, _| {
        if protected == Some(id) {
            true
        } else if remaining > 0 {
            remaining -= 1;
            true
        } else {
            false
        }
    });
}

/// Downloads allowlisted artwork while bounding redirects and streamed bytes.
pub(super) async fn artwork(raw: &str) -> Result<Vec<u8>, AppError> {
    const MAX: usize = 2 * 1024 * 1024;
    let client = super::http_client()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|_| safe(ErrorKind::Internal, "HTTP client unavailable"))?;
    let mut url =
        url::Url::parse(raw).map_err(|_| safe(ErrorKind::InvalidInput, "invalid artwork URL"))?;
    for redirects in 0..=3 {
        validate_artwork_url(&url)?;
        let mut r = client
            .get(url.clone())
            .send()
            .await
            .map_err(|_| safe(ErrorKind::Network, "artwork download failed"))?;
        if r.status().is_redirection() {
            if redirects == 3 {
                return Err(safe(ErrorKind::Network, "too many artwork redirects"));
            }
            let loc = r
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| safe(ErrorKind::Network, "invalid artwork redirect"))?;
            url = url
                .join(loc)
                .map_err(|_| safe(ErrorKind::Network, "invalid artwork redirect"))?;
            continue;
        }
        if !r.status().is_success() {
            return Err(safe(ErrorKind::Network, "artwork download failed"));
        }
        if r.content_length().is_some_and(|n| n > MAX as u64) {
            return Err(safe(ErrorKind::Unavailable, "artwork exceeds 2 MiB"));
        }
        let mut out = Vec::new();
        while let Some(chunk) = r
            .chunk()
            .await
            .map_err(|_| safe(ErrorKind::Network, "artwork download failed"))?
        {
            if out.len().saturating_add(chunk.len()) > MAX {
                return Err(safe(ErrorKind::Unavailable, "artwork exceeds 2 MiB"));
            }
            out.extend_from_slice(&chunk)
        }
        return Ok(out);
    }
    unreachable!()
}
/// Enforces the HTTPS Spotify CDN redirect allowlist.
fn validate_artwork_url(u: &url::Url) -> Result<(), AppError> {
    let host = u.host_str().unwrap_or("");
    if u.scheme() != "https"
        || u.port().is_some()
        || !(host == "i.scdn.co" || host.ends_with(".scdn.co"))
    {
        Err(safe(ErrorKind::InvalidInput, "artwork host is not allowed"))
    } else {
        Ok(())
    }
}
/// Constructs a display-safe application error without secret context.
pub(super) fn safe(kind: ErrorKind, message: &str) -> AppError {
    AppError {
        kind,
        message: message.into(),
        retry_after_seconds: None,
    }
}

/// Saves credentials on the actor after epoch validation, so logout cannot be undone
/// by an old authorization job finishing late.
fn install_playback(
    setup: playback_auth::Setup,
    state: &mut AppState,
    events: &tokio::sync::mpsc::Sender<Event>,
) -> Result<playback::Playback, String> {
    if let Some(record) = setup.record
        && let Err(error) = auth::Vault::store_entry(auth::PLAYBACK_ENTRY, &record)
    {
        state.connect.playback_auth = PlaybackAuth::Failed(error.clone());
        emit(
            events,
            Event::Notice(safe(ErrorKind::CredentialStore, &error)),
        );
        return Err(error);
    }
    match setup.playback {
        Ok(player) => {
            state.connect.playback_auth = PlaybackAuth::Ready;
            Ok(player)
        }
        Err(auth) => {
            let message = match &auth {
                PlaybackAuth::Failed(error) => error.clone(),
                _ => "Enable playback & Spotify Connect in Settings → Account. Remote controls remain available.".into(),
            };
            state.connect.playback_auth = auth;
            state.connect.ensure_local_device(
                super::transfer::LOCAL_DEVICE_ID,
                super::connect::device_name(),
                state.player.volume,
            );
            Err(message)
        }
    }
}

fn install_connect_state(
    player: &playback::Playback,
    state: &mut AppState,
    events: &tokio::sync::mpsc::Sender<Event>,
) {
    let id = player
        .connect
        .as_ref()
        .map_or(super::transfer::LOCAL_DEVICE_ID, |c| c.device_id.as_str());
    state
        .connect
        .ensure_local_device(id, super::connect::device_name(), state.player.volume);
    if let Some(error) = &player.connect_error {
        log::warn!("{error}");
        emit(
            events,
            Event::Notice(safe(
                ErrorKind::Unavailable,
                "Spotify Connect unavailable. This computer can still play locally; device updates use polling. Enable RUST_LOG for details.",
            )),
        );
    }
}

fn observation_ids(state: &AppState, metadata: &HashMap<SpotifyId, Track>) -> Vec<SpotifyId> {
    // Each track is its own request, so only the head of the queue preview is hydrated.
    const PREVIEW_HYDRATION: usize = 10;
    let mut ids = Vec::new();
    for track in state
        .player
        .current
        .iter()
        .chain(state.connect.queue_preview.iter().take(PREVIEW_HYDRATION))
    {
        if !metadata.contains_key(&track.id) && !ids.contains(&track.id) {
            ids.push(track.id.clone());
        }
    }
    ids
}

fn hydrate_observation(state: &mut AppState, metadata: &HashMap<SpotifyId, Track>) {
    if let Some(track) = state.player.current.as_mut()
        && let Some(full) = metadata.get(&track.id)
    {
        *track = full.clone();
    }
    for track in &mut state.connect.queue_preview {
        if let Some(full) = metadata.get(&track.id) {
            *track = full.clone();
        }
    }
}

#[cfg(test)]
mod regression_tests {
    use super::*;

    #[test]
    fn missing_playback_consent_keeps_web_sign_in_and_synthetic_device() {
        let mut state = super::super::initial_state();
        state.auth = AuthState::Ready {
            user_id: "user".into(),
        };
        let (events, _rx) = tokio::sync::mpsc::channel(4);
        let setup = playback_auth::Setup {
            playback: Err(PlaybackAuth::NeedsAuthorization),
            record: None,
        };
        assert!(install_playback(setup, &mut state, &events).is_err());
        assert!(matches!(state.auth, AuthState::Ready { .. }));
        assert_eq!(
            state.connect.playback_auth,
            PlaybackAuth::NeedsAuthorization
        );
        assert_eq!(state.connect.devices.len(), 1);
        assert!(state.connect.devices[0].is_this_app);
        assert_eq!(
            state.connect.this_device_id.as_deref(),
            Some(super::super::transfer::LOCAL_DEVICE_ID)
        );
    }

    #[test]
    fn large_metadata_batches_keep_current_track_with_bounded_working_set() {
        let protected = SpotifyId("current".into());
        let tracks = (0..5000)
            .map(|index| Track {
                id: if index == 0 {
                    protected.clone()
                } else {
                    SpotifyId(index.to_string())
                },
                name: "Track".into(),
                artists: vec![],
                album: None,
                duration_ms: 1000,
                playable: true,
                source_index: None,
            })
            .collect();
        let mut metadata = HashMap::new();
        insert_metadata(&mut metadata, tracks, Some(&protected));
        assert_eq!(metadata.len(), 512);
        assert!(metadata.contains_key(&protected));
    }

    #[test]
    fn artwork_allowlist_rejects_suffix_tricks_ports_and_http() {
        for raw in [
            "https://i.scdn.co.evil.test/a",
            "https://i.scdn.co:444/a",
            "http://i.scdn.co/a",
        ] {
            assert!(validate_artwork_url(&url::Url::parse(raw).unwrap()).is_err());
        }
        assert!(
            validate_artwork_url(
                &url::Url::parse("https://image-cdn-ak.spotifycdn.com/a").unwrap()
            )
            .is_err()
        );
        assert!(validate_artwork_url(&url::Url::parse("https://i.scdn.co/a").unwrap()).is_ok());
    }
    #[tokio::test]
    async fn saturated_event_queue_still_delivers_quit_terminal_events_in_order() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        tx.send(Event::Notice(safe(ErrorKind::Internal, "filler")))
            .await
            .unwrap();
        let task = tokio::spawn(async move { emit_shutdown(&tx, RequestId(41)).await });
        assert!(matches!(rx.recv().await, Some(Event::Notice(_))));
        assert!(matches!(
            rx.recv().await,
            Some(Event::Completed {
                id: RequestId(41),
                result: Ok(Response::Accepted)
            })
        ));
        assert!(matches!(rx.recv().await, Some(Event::QuitReady)));
        task.await.unwrap();
    }

    #[test]
    fn requested_decoder_policy_is_real_and_bounded() {
        let c = playback::player_config();
        assert!(c.gapless && c.normalisation);
        assert_eq!(
            c.position_update_interval,
            Some(std::time::Duration::from_secs(1))
        );
    }
}
