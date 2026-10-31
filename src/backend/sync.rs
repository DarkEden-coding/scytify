//! Listening stats shared between this user's devices over Tailscale.
//!
//! Every linked device listens on its Tailscale address and, once a minute, asks each
//! other device for listens and history that changed since the last time. Devices also
//! pass on what they received from others and the devices they know, so a group of any
//! size stays in step as long as it stays connected. Tailscale encrypts and limits who can
//! connect; a group key shared through the link code keeps out other devices on the
//! tailnet.
use super::auth::Vault;
use super::storage::{Storage, SyncBatch};
use crate::{contracts::SyncAction, domain::*};
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};

const PORT: u16 = 47_812;
const EVERY: Duration = Duration::from_secs(60);
const TIMEOUT: Duration = Duration::from_secs(15);
/// Changes per message, and messages per exchange with one device.
const BATCH: usize = 500;
const MAX_ROUNDS: usize = 200;
/// Longest message accepted; a full batch of listens is well under this.
const MAX_MESSAGE: u64 = 16 * 1024 * 1024;
const KEY_ENTRY: &str = "listening-sync-key";

#[derive(Serialize, Deserialize)]
struct Request {
    key: String,
    device_id: String,
    /// The asking device's own Tailscale address, if it has one.
    address: Option<String>,
    cursor: i64,
}

#[derive(Serialize, Deserialize)]
struct KnownDevice {
    device_id: String,
    address: String,
}

#[derive(Serialize, Deserialize)]
enum Reply {
    Changes {
        device_id: String,
        devices: Vec<KnownDevice>,
        batch: SyncBatch,
        next: i64,
        more: bool,
    },
    Refused(String),
}

/// What the listener and the exchange loop share.
struct Shared {
    store: Mutex<Storage>,
    key: Mutex<Option<String>>,
    /// This device's address while it listens.
    address: Mutex<Option<String>>,
    /// Counts merges that stored something new.
    revision: Mutex<u64>,
}

impl Shared {
    fn store(&self) -> std::sync::MutexGuard<'_, Storage> {
        self.store.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn key(&self) -> Option<String> {
        self.key.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    fn address(&self) -> Option<String> {
        self.address
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    fn merge(&self, batch: &SyncBatch) -> Result<(), String> {
        if self.store().merge_sync(batch)? {
            *self.revision.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        }
        Ok(())
    }
}

/// Starts sync on its own task with its own connection to `store`. Send it actions;
/// watch its state.
pub(super) fn start(store: Storage) -> (mpsc::Sender<SyncAction>, watch::Receiver<SyncState>) {
    let (actions, receiver) = mpsc::channel(16);
    let (state, watcher) = watch::channel(SyncState::default());
    tokio::spawn(run(store, receiver, state));
    (actions, watcher)
}

async fn run(
    store: Storage,
    mut actions: mpsc::Receiver<SyncAction>,
    state: watch::Sender<SyncState>,
) {
    let key = Vault::load_entry::<String>(KEY_ENTRY).unwrap_or_else(|error| {
        log::warn!("could not read the sync key: {error}");
        None
    });
    let mut enabled = store.sync_enabled().unwrap_or(false);
    let shared = Arc::new(Shared {
        store: Mutex::new(store),
        key: Mutex::new(key),
        address: Mutex::new(None),
        revision: Mutex::new(0),
    });
    let mut listener: Option<(Ipv4Addr, tokio::task::JoinHandle<()>)> = None;
    // The outcome of the last action, until the next one.
    let mut notice: Option<String> = None;
    let mut tick = tokio::time::interval(EVERY);
    loop {
        let mut joined: Option<String> = None;
        tokio::select! {
            action = actions.recv() => match action.inspect(|_| notice = None) {
                None => break,
                Some(SyncAction::Enable(on)) => {
                    enabled = on;
                    if let Err(error) = enable(&shared, on) {
                        notice = Some(error);
                    }
                }
                Some(SyncAction::Join(code)) => match parse_code(&code) {
                    Ok((address, key)) => {
                        enabled = true;
                        match store_key(&shared, Some(key)).and_then(|()| enable(&shared, true)) {
                            Ok(()) => joined = Some(address),
                            Err(error) => notice = Some(error),
                        }
                    }
                    Err(error) => notice = Some(error.into()),
                },
                Some(SyncAction::Forget(device_id)) => {
                    if let Err(error) = shared.store().forget_sync_peer(&device_id) {
                        log::warn!("could not forget a sync device: {error}");
                    }
                }
                Some(SyncAction::Now) => {}
            },
            _ = tick.tick() => {}
        }
        if !enabled {
            if let Some((_, task)) = listener.take() {
                task.abort();
            }
            *shared.address.lock().unwrap_or_else(|e| e.into_inner()) = None;
            publish(&shared, &state, false, notice.clone());
            continue;
        }
        let unreachable = listen(&shared, &mut listener).err();
        publish(
            &shared,
            &state,
            true,
            notice.clone().or(unreachable.clone()),
        );
        if let Some(address) = joined
            && let Err(error) = exchange(&shared, &address, None).await
        {
            notice = Some(format!("Could not link with {address}: {error}"));
        }
        let peers = shared.store().sync_peers().unwrap_or_default();
        futures_util::future::join_all(
            peers
                .iter()
                .map(|peer| exchange(&shared, &peer.address, Some((&peer.device_id, peer.cursor)))),
        )
        .await;
        publish(&shared, &state, true, notice.clone().or(unreachable));
    }
    if let Some((_, task)) = listener {
        task.abort();
    }
}

/// Turns sync on or off, creating a group key the first time.
fn enable(shared: &Shared, on: bool) -> Result<(), String> {
    shared.store().set_sync_enabled(on)?;
    if on && shared.key().is_none() {
        store_key(shared, Some(format!("{:032x}", rand::random::<u128>())))?;
    }
    Ok(())
}

fn store_key(shared: &Shared, key: Option<String>) -> Result<(), String> {
    if let Some(key) = &key {
        Vault::store_entry(KEY_ENTRY, key)?;
    }
    *shared.key.lock().unwrap_or_else(|e| e.into_inner()) = key;
    Ok(())
}

fn publish(
    shared: &Shared,
    state: &watch::Sender<SyncState>,
    enabled: bool,
    problem: Option<String>,
) {
    let address = shared.address();
    let code = match (enabled, &address, shared.key()) {
        (true, Some(address), Some(key)) => Some(format!("{address}/{key}")),
        _ => None,
    };
    let peers = shared
        .store()
        .sync_peers()
        .unwrap_or_default()
        .into_iter()
        .map(|peer| SyncPeer {
            device_id: peer.device_id,
            address: peer.address,
            last_synced: peer.last_synced,
            error: peer.error,
        })
        .collect();
    let revision = *shared.revision.lock().unwrap_or_else(|e| e.into_inner());
    state.send_if_modified(|current| {
        let next = SyncState {
            enabled,
            code,
            problem,
            peers,
            revision,
        };
        let changed = *current != next;
        *current = next;
        changed
    });
}

/// Keeps a listener on this device's current Tailscale address.
fn listen(
    shared: &Arc<Shared>,
    listener: &mut Option<(Ipv4Addr, tokio::task::JoinHandle<()>)>,
) -> Result<(), String> {
    let Some(ip) = tailscale_ip() else {
        if let Some((_, task)) = listener.take() {
            task.abort();
        }
        *shared.address.lock().unwrap_or_else(|e| e.into_inner()) = None;
        return Err("Tailscale isn't connected on this device".into());
    };
    if listener
        .as_ref()
        .is_some_and(|(current, task)| *current == ip && !task.is_finished())
    {
        return Ok(());
    }
    if let Some((_, task)) = listener.take() {
        task.abort();
    }
    let socket = std::net::TcpListener::bind((ip, PORT))
        .and_then(|socket| socket.set_nonblocking(true).map(|()| socket))
        .and_then(TcpListener::from_std)
        .map_err(|error| format!("Could not listen on {ip}:{PORT}: {error}"))?;
    *shared.address.lock().unwrap_or_else(|e| e.into_inner()) = Some(format!("{ip}:{PORT}"));
    let shared = shared.clone();
    *listener = Some((
        ip,
        tokio::spawn(async move {
            while let Ok((stream, from)) = socket.accept().await {
                let shared = shared.clone();
                tokio::spawn(async move {
                    if let Err(error) = tokio::time::timeout(TIMEOUT, answer(&shared, stream, from))
                        .await
                        .unwrap_or(Err("timed out".into()))
                    {
                        log::info!("sync request from {from} failed: {error}");
                    }
                });
            }
        }),
    ));
    Ok(())
}

/// This device's Tailscale IPv4 address: the source address of the route to Tailscale's
/// own resolver, when that lies in Tailscale's range. Sends nothing.
fn tailscale_ip() -> Option<Ipv4Addr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("100.100.100.100:53").ok()?;
    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(ip) if in_tailnet(ip) => Some(ip),
        _ => None,
    }
}

/// Tailscale gives devices addresses in 100.64.0.0/10.
fn in_tailnet(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    a == 100 && (64..128).contains(&b)
}

/// A link code is `address:port/key`.
fn parse_code(code: &str) -> Result<(String, String), &'static str> {
    let (address, key) = code
        .trim()
        .split_once('/')
        .ok_or("That isn't a link code")?;
    let socket: SocketAddr = address.parse().map_err(|_| "That isn't a link code")?;
    let tailnet = matches!(socket.ip(), IpAddr::V4(ip) if in_tailnet(ip));
    if !tailnet {
        return Err("The code's address isn't a Tailscale address");
    }
    if key.len() != 32 || !key.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("That isn't a link code");
    }
    Ok((socket.to_string(), key.to_owned()))
}

/// Answers one request from another device.
async fn answer(shared: &Shared, stream: TcpStream, from: SocketAddr) -> Result<(), String> {
    let (read, mut write) = stream.into_split();
    let request: Request = read_message(read).await?;
    let reply = reply_to(shared, &request, from.ip());
    write_message(&mut write, &reply).await
}

fn reply_to(shared: &Shared, request: &Request, from: IpAddr) -> Reply {
    if shared.key().is_none_or(|key| key != request.key) {
        return Reply::Refused("This device belongs to another sync group".into());
    }
    let store = shared.store();
    match store.sync_peer_removed(&request.device_id) {
        Ok(false) => {}
        Ok(true) => return Reply::Refused("This device was unlinked".into()),
        Err(error) => return Reply::Refused(error),
    }
    // Trust the address the request came from over the one it names.
    let address = request
        .address
        .as_deref()
        .and_then(|a| a.parse::<SocketAddr>().ok())
        .map_or(SocketAddr::new(from, PORT), |a| {
            SocketAddr::new(from, a.port())
        });
    if let Err(error) = store.add_sync_peer(&request.device_id, &address.to_string(), false) {
        return Reply::Refused(error);
    }
    let devices = store
        .sync_peers()
        .unwrap_or_default()
        .into_iter()
        .map(|peer| KnownDevice {
            device_id: peer.device_id,
            address: peer.address,
        })
        .collect();
    match store.sync_changes(request.cursor, &request.device_id, BATCH) {
        Ok((batch, next, more)) => Reply::Changes {
            device_id: store.device_id().to_owned(),
            devices,
            batch,
            next,
            more,
        },
        Err(error) => Reply::Refused(error),
    }
}

/// Pulls everything new from the device at `address`. `known` is its id and how far this
/// device has merged, unless this is the first contact.
async fn exchange(
    shared: &Shared,
    address: &str,
    known: Option<(&str, i64)>,
) -> Result<(), String> {
    let mut cursor = known.map_or(0, |(_, cursor)| cursor);
    let mut result = Ok(());
    for _ in 0..MAX_ROUNDS {
        let round = tokio::time::timeout(TIMEOUT, ask(shared, address, cursor))
            .await
            .unwrap_or(Err("timed out".into()));
        let reply = match round {
            Ok(Reply::Changes {
                device_id,
                devices,
                batch,
                next,
                more,
            }) => match store_reply(shared, address, known, &device_id, devices, &batch, next) {
                Ok(()) => {
                    cursor = next;
                    Ok(more)
                }
                Err(error) => Err((Some(device_id), error)),
            },
            Ok(Reply::Refused(reason)) => Err((known.map(|k| k.0.to_owned()), reason)),
            Err(error) => Err((known.map(|k| k.0.to_owned()), error)),
        };
        match reply {
            Ok(true) => continue,
            Ok(false) => break,
            Err((device_id, error)) => {
                if let Some(device_id) = device_id
                    && let Err(e) = shared.store().sync_peer_result(&device_id, Err(&error))
                {
                    log::warn!("could not record a sync failure: {e}");
                }
                result = Err(error);
                break;
            }
        }
    }
    result
}

/// Keeps what one reply from `address` brought: its sender, the devices it knows, and
/// its changes up to `next`.
fn store_reply(
    shared: &Shared,
    address: &str,
    known: Option<(&str, i64)>,
    device_id: &str,
    devices: Vec<KnownDevice>,
    batch: &SyncBatch,
    next: i64,
) -> Result<(), String> {
    if known.is_some_and(|(id, _)| id != device_id) {
        return Err("a different device now uses this address".into());
    }
    {
        let store = shared.store();
        store.add_sync_peer(device_id, address, known.is_none())?;
        for device in devices {
            store.add_sync_peer(&device.device_id, &device.address, false)?;
        }
    }
    shared.merge(batch)?;
    shared.store().sync_peer_result(device_id, Ok(next))
}

async fn ask(shared: &Shared, address: &str, cursor: i64) -> Result<Reply, String> {
    let request = Request {
        key: shared.key().ok_or("no sync key")?,
        device_id: shared.store().device_id().to_owned(),
        address: shared.address(),
        cursor,
    };
    let stream = TcpStream::connect(address)
        .await
        .map_err(|e| e.to_string())?;
    let (read, mut write) = stream.into_split();
    write_message(&mut write, &request).await?;
    read_message(read).await
}

/// Messages are one line of JSON each.
async fn write_message(
    write: &mut tokio::net::tcp::OwnedWriteHalf,
    message: &impl Serialize,
) -> Result<(), String> {
    let mut line = serde_json::to_vec(message).map_err(|e| e.to_string())?;
    line.push(b'\n');
    write.write_all(&line).await.map_err(|e| e.to_string())?;
    write.shutdown().await.map_err(|e| e.to_string())
}

async fn read_message<T: for<'de> Deserialize<'de>>(
    read: tokio::net::tcp::OwnedReadHalf,
) -> Result<T, String> {
    let mut line = Vec::new();
    BufReader::new(read.take(MAX_MESSAGE))
        .read_until(b'\n', &mut line)
        .await
        .map_err(|e| e.to_string())?;
    serde_json::from_slice(&line).map_err(|_| "unreadable message".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_codes_need_a_tailscale_address_and_a_key() {
        let key = "0123456789abcdef0123456789abcdef";
        assert_eq!(
            parse_code(&format!(" 100.101.2.3:47812/{key} ")),
            Ok(("100.101.2.3:47812".into(), key.into()))
        );
        assert!(parse_code(&format!("192.168.1.2:47812/{key}")).is_err());
        assert!(parse_code("100.101.2.3:47812/short").is_err());
        assert!(parse_code(key).is_err());
    }

    /// Two devices with the same key exchange listens over a socket and learn of each
    /// other; a wrong key is refused.
    #[tokio::test]
    async fn devices_exchange_listening_over_a_socket() {
        let dir = |name: &str| {
            std::env::temp_dir().join(format!("scytify-link-{name}-{}", rand::random::<u64>()))
        };
        let shared = |root: &std::path::Path| {
            Arc::new(Shared {
                store: Mutex::new(Storage::open_at(root).unwrap()),
                key: Mutex::new(Some("k".into())),
                address: Mutex::new(None),
                revision: Mutex::new(0),
            })
        };
        let (root_a, root_b) = (dir("a"), dir("b"));
        let (a, b) = (shared(&root_a), shared(&root_b));
        let track: Track = serde_json::from_value(serde_json::json!({
            "id": "a".repeat(22), "name": "Song", "artists": [], "album": null,
            "duration_ms": 1000, "playable": true, "source_index": null
        }))
        .unwrap();
        a.store().save_listen(None, &track, 1, 5_000, true).unwrap();
        let socket = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = socket.local_addr().unwrap().to_string();
        let server = a.clone();
        tokio::spawn(async move {
            while let Ok((stream, from)) = socket.accept().await {
                answer(&server, stream, from).await.unwrap();
            }
        });
        exchange(&b, &address, None).await.unwrap();
        assert_eq!(*b.revision.lock().unwrap(), 1);
        let a_id = a.store().device_id().to_owned();
        let peers = b.store().sync_peers().unwrap();
        assert_eq!(
            (peers[0].device_id.as_str(), peers[0].address.as_str()),
            (a_id.as_str(), address.as_str())
        );
        assert_eq!(b.store().listen_totals(i64::MIN).unwrap()[0].1, 5_000);
        // a now knows b, at b's address as a saw it.
        assert_eq!(a.store().sync_peers().unwrap().len(), 1);
        *b.key.lock().unwrap() = Some("other".into());
        let refused = exchange(&b, &address, Some((&a_id, peers[0].cursor))).await;
        assert!(refused.is_err());
        assert!(b.store().sync_peers().unwrap()[0].error.is_some());
        drop((a, b));
        for root in [root_a, root_b] {
            let _ = std::fs::remove_dir_all(root);
        }
    }
}
