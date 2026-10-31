use crate::domain::{Lyrics, SessionSnapshot, Settings, SpotifyId, Track};
use directories::ProjectDirs;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Durable non-secret backend storage. Authentication material is never accepted here.
pub(crate) struct Storage {
    root: PathBuf,
    db: Connection,
    /// Cached audio size at its last measurement.
    audio_bytes: std::cell::Cell<u64>,
    /// This installation's random identity, recorded on every listen it makes.
    device_id: String,
}

/// Listens and history changed since a sync cursor, for another device to merge.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct SyncBatch {
    pub listens: Vec<SyncListen>,
    pub heard: Vec<SyncHeard>,
    /// Stored metadata (JSON) of each song in `listens`.
    pub tracks: Vec<(String, String)>,
}

/// One listen, identified by the device that made it and its row there.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct SyncListen {
    pub origin: String,
    pub origin_row: i64,
    pub track_id: String,
    pub started_at: i64,
    pub listened_ms: i64,
    pub counted: bool,
}

/// One device's history of one song.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct SyncHeard {
    pub origin: String,
    pub track_id: String,
    pub last_heard: i64,
    pub plays: i64,
}

/// A linked device as stored.
#[derive(Debug)]
pub(crate) struct StoredPeer {
    pub device_id: String,
    pub address: String,
    /// The peer's change number this device has merged up to.
    pub cursor: i64,
    pub last_synced: Option<i64>,
    pub error: Option<String>,
}

impl Storage {
    /// Opens the per-user data database and applies its small versioned schema.
    pub(crate) fn open() -> Result<Self, String> {
        let dirs = ProjectDirs::from("dev", "Scytify", "Scytify")
            .ok_or("user data directory is unavailable")?;
        Self::open_at(dirs.data_local_dir())
    }
    /// Opens storage at an explicit path, primarily for isolated regression tests.
    pub(crate) fn open_at(root: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
        let db = Connection::open(root.join("metadata.sqlite3")).map_err(|e| e.to_string())?;
        db.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| e.to_string())?;
        // Sync writes through its own connection; wait for the other writer instead of failing.
        db.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS kv(k TEXT PRIMARY KEY,v BLOB NOT NULL); CREATE TABLE IF NOT EXISTS cache(k TEXT PRIMARY KEY,path TEXT NOT NULL,size INTEGER NOT NULL,atime INTEGER NOT NULL);").map_err(|e| e.to_string())?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS lyrics_cache(track_id TEXT PRIMARY KEY, body BLOB NOT NULL, stored_at INTEGER NOT NULL, expires_at INTEGER NOT NULL);").map_err(|e| e.to_string())?;
        // Not a cache: Discover relies on it to skip songs heard before, so Clear cache keeps it.
        db.execute_batch("CREATE TABLE IF NOT EXISTS listen_history(track_id TEXT PRIMARY KEY, last_heard INTEGER NOT NULL, plays INTEGER NOT NULL);").map_err(|e| e.to_string())?;
        // Listening stats: one row per listen, and the last known metadata of each song.
        db.execute_batch("CREATE TABLE IF NOT EXISTS listens(id INTEGER PRIMARY KEY, track_id TEXT NOT NULL, started_at INTEGER NOT NULL, listened_ms INTEGER NOT NULL, counted INTEGER NOT NULL); CREATE INDEX IF NOT EXISTS listens_started ON listens(started_at); CREATE TABLE IF NOT EXISTS listened_tracks(track_id TEXT PRIMARY KEY, body BLOB NOT NULL);").map_err(|e| e.to_string())?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS sync_peers(device_id TEXT PRIMARY KEY, address TEXT NOT NULL, cursor INTEGER NOT NULL DEFAULT 0, last_synced INTEGER, error TEXT, removed INTEGER NOT NULL DEFAULT 0);").map_err(|e| e.to_string())?;
        db.execute(
            "INSERT OR IGNORE INTO kv(k,v) VALUES('device_id',?1)",
            [format!("{:032x}", rand::random::<u128>())],
        )
        .map_err(|e| e.to_string())?;
        let device_id: String = db
            .query_row("SELECT v FROM kv WHERE k='device_id'", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        migrate_for_sync(&db, &device_id).map_err(|e| e.to_string())?;
        let store = Self {
            root: root.to_owned(),
            db,
            audio_bytes: std::cell::Cell::new(0),
            device_id,
        };
        store.prune_lyrics(now())?;
        Ok(store)
    }
    /// Another connection to the same data, for work on another task.
    pub(crate) fn reopen(&self) -> Result<Self, String> {
        Self::open_at(&self.root)
    }
    pub(crate) fn device_id(&self) -> &str {
        &self.device_id
    }
    /// Runs `write` in one transaction that holds the write lock from the start, so change
    /// numbers commit in the order they are taken.
    fn write<T>(
        &self,
        write: impl FnOnce(&Connection) -> rusqlite::Result<T>,
    ) -> Result<T, String> {
        self.db
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|e| e.to_string())?;
        match write(&self.db).and_then(|value| self.db.execute_batch("COMMIT").map(|_| value)) {
            Ok(value) => Ok(value),
            Err(error) => {
                let _ = self.db.execute_batch("ROLLBACK");
                Err(error.to_string())
            }
        }
    }
    /// Loads validated settings, defaulting only when absent and reporting corruption.
    pub(crate) fn settings(&self) -> Result<Settings, String> {
        match self.get_json("settings")? {
            None => Ok(default_settings()),
            Some(v) => serde_json::from_slice(&v)
                .map_err(|_| "stored settings are corrupt".to_owned())
                .and_then(|s| {
                    if valid_settings(&s) {
                        Ok(s)
                    } else {
                        Err("stored settings are invalid".into())
                    }
                }),
        }
    }
    /// Atomically records validated settings.
    pub(crate) fn save_settings(&self, s: &Settings) -> Result<(), String> {
        if !valid_settings(s) {
            return Err("invalid settings".into());
        }
        self.put_json(
            "settings",
            &serde_json::to_vec(s).map_err(|e| e.to_string())?,
        )
    }
    /// Loads and structurally validates a queue checkpoint.
    pub(crate) fn session(&self) -> Result<Option<SessionSnapshot>, String> {
        self.get_json("session")?
            .map(|v| {
                serde_json::from_slice(&v)
                    .map_err(|_| "stored playback session is corrupt".to_owned())
            })
            .transpose()
    }
    /// Atomically checkpoints queue state in SQLite WAL.
    pub(crate) fn save_session(&self, s: &SessionSnapshot) -> Result<(), String> {
        self.put_json(
            "session",
            &serde_json::to_vec(s).map_err(|e| e.to_string())?,
        )
    }
    fn get_json(&self, k: &str) -> Result<Option<Vec<u8>>, String> {
        match self
            .db
            .query_row("SELECT v FROM kv WHERE k=?1", [k], |r| r.get(0))
        {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }
    fn put_json(&self, k: &str, v: &[u8]) -> Result<(), String> {
        self.db
            .execute(
                "INSERT INTO kv(k,v) VALUES(?1,?2) ON CONFLICT(k) DO UPDATE SET v=excluded.v",
                params![k, v],
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    pub(crate) fn cached_lyrics(
        &self,
        track_id: &SpotifyId,
    ) -> Result<Option<Option<Lyrics>>, String> {
        self.cached_lyrics_at(track_id, now())
    }
    fn cached_lyrics_at(
        &self,
        track_id: &SpotifyId,
        timestamp: i64,
    ) -> Result<Option<Option<Lyrics>>, String> {
        self.prune_lyrics(timestamp)?;
        self.db
            .query_row(
                "SELECT body FROM lyrics_cache WHERE track_id=?1",
                [&track_id.0],
                |r| r.get::<_, Vec<u8>>(0),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .map(|body| serde_json::from_slice(&body).map_err(|e| e.to_string()))
            .transpose()
    }
    pub(crate) fn cache_lyrics(
        &self,
        track_id: &SpotifyId,
        lyrics: &Option<Lyrics>,
    ) -> Result<(), String> {
        self.cache_lyrics_at(track_id, lyrics, now())
    }
    fn cache_lyrics_at(
        &self,
        track_id: &SpotifyId,
        lyrics: &Option<Lyrics>,
        timestamp: i64,
    ) -> Result<(), String> {
        let ttl = if lyrics.is_some() { 30 * 86400 } else { 86400 };
        let body = serde_json::to_vec(lyrics).map_err(|e| e.to_string())?;
        self.db.execute("INSERT INTO lyrics_cache(track_id,body,stored_at,expires_at) VALUES(?1,?2,?3,?4) ON CONFLICT(track_id) DO UPDATE SET body=excluded.body,stored_at=excluded.stored_at,expires_at=excluded.expires_at",
            params![track_id.0, body, timestamp, timestamp.saturating_add(ttl)]).map(|_| ()).map_err(|e| e.to_string())
    }
    /// Discard expired content even when those tracks are never requested again.
    pub(crate) fn prune_lyrics(&self, timestamp: i64) -> Result<(), String> {
        self.db
            .execute(
                "DELETE FROM lyrics_cache WHERE expires_at<=?1 OR stored_at>?1",
                [timestamp],
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    /// Records that these tracks were heard, counting a play for each.
    pub(crate) fn record_listens(&self, track_ids: &[SpotifyId]) -> Result<(), String> {
        let timestamp = now();
        self.write(|db| {
            for id in track_ids {
                db.execute(
                    "INSERT INTO listen_history(origin,track_id,last_heard,plays,seq) VALUES(?1,?2,?3,1,?4) ON CONFLICT(origin,track_id) DO UPDATE SET last_heard=excluded.last_heard,plays=plays+1,seq=excluded.seq",
                    params![self.device_id, id.0, timestamp, next_seq(db)?],
                )?;
            }
            Ok(())
        })
    }
    /// Every track heard on this device, or seen in Spotify's recent plays.
    pub(crate) fn heard_tracks(&self) -> Result<std::collections::HashSet<SpotifyId>, String> {
        let mut statement = self
            .db
            .prepare("SELECT DISTINCT track_id FROM listen_history")
            .map_err(|e| e.to_string())?;
        statement
            .query_map([], |r| r.get::<_, String>(0).map(SpotifyId))
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())
    }
    /// Records one listen of a song that started at `started_at`, or updates the listen
    /// stored before as `row`. Returns the listen's row.
    pub(crate) fn save_listen(
        &self,
        row: Option<i64>,
        track: &Track,
        started_at: i64,
        listened_ms: u64,
        counted: bool,
    ) -> Result<i64, String> {
        let body = serde_json::to_vec(track).map_err(|e| e.to_string())?;
        self.write(|db| {
            db.execute(
                "INSERT INTO listened_tracks(track_id,body) VALUES(?1,?2) ON CONFLICT(track_id) DO UPDATE SET body=excluded.body",
                params![track.id.0, body],
            )?;
            let seq = next_seq(db)?;
            if let Some(row) = row {
                db.execute(
                    "UPDATE listens SET listened_ms=?2,counted=?3,seq=?4 WHERE id=?1",
                    params![row, listened_ms as i64, counted, seq],
                )?;
                return Ok(row);
            }
            db.execute(
                "INSERT INTO listens(track_id,started_at,listened_ms,counted,origin,seq) VALUES(?1,?2,?3,?4,?5,?6)",
                params![track.id.0, started_at, listened_ms as i64, counted, self.device_id, seq],
            )?;
            let row = db.last_insert_rowid();
            db.execute("UPDATE listens SET origin_row=?1 WHERE id=?1", [row])?;
            Ok(row)
        })
    }
    /// Each song listened to since `since`, with its total listening time and plays.
    pub(crate) fn listen_totals(&self, since: i64) -> Result<Vec<(Track, u64, u64)>, String> {
        let mut statement = self
            .db
            .prepare("SELECT t.body, SUM(l.listened_ms), SUM(l.counted) FROM listens l JOIN listened_tracks t ON t.track_id=l.track_id WHERE l.started_at>=?1 GROUP BY l.track_id")
            .map_err(|e| e.to_string())?;
        let rows = statement
            .query_map([since], |r| {
                Ok((
                    r.get::<_, Vec<u8>>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        let mut totals = Vec::new();
        for row in rows {
            let (body, ms, plays) = row.map_err(|e| e.to_string())?;
            // Skip a song whose stored metadata no longer parses rather than failing all stats.
            if let Ok(track) = serde_json::from_slice::<Track>(&body) {
                totals.push((track, ms.max(0) as u64, plays.max(0) as u64));
            }
        }
        Ok(totals)
    }
    /// Listening time since `since` by local hour of day when each listen started.
    pub(crate) fn listen_hours(&self, since: i64) -> Result<[u64; 24], String> {
        let mut statement = self
            .db
            .prepare("SELECT CAST(strftime('%H',started_at,'unixepoch','localtime') AS INTEGER), SUM(listened_ms) FROM listens WHERE started_at>=?1 GROUP BY 1")
            .map_err(|e| e.to_string())?;
        let rows = statement
            .query_map([since], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))
            .map_err(|e| e.to_string())?;
        let mut hours = [0; 24];
        for row in rows {
            let (hour, ms) = row.map_err(|e| e.to_string())?;
            if let Some(slot) = usize::try_from(hour).ok().and_then(|h| hours.get_mut(h)) {
                *slot = ms.max(0) as u64;
            }
        }
        Ok(hours)
    }
    /// Local day numbers with at least one play, oldest first, and the day number of `now`.
    pub(crate) fn listen_days(&self, now: i64) -> Result<(i64, Vec<i64>), String> {
        // Julian day numbers: consecutive local dates differ by exactly one.
        const DAY: &str = "CAST(julianday(?1,'unixepoch','localtime')+0.5 AS INTEGER)";
        let today: i64 = self
            .db
            .query_row(&format!("SELECT {DAY}"), [now], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        let mut statement = self
            .db
            .prepare(&format!(
                "SELECT DISTINCT {} FROM listens WHERE counted=1 ORDER BY 1",
                DAY.replace("?1", "started_at")
            ))
            .map_err(|e| e.to_string())?;
        let days = statement
            .query_map([], |r| r.get::<_, i64>(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        Ok((today, days))
    }
    /// Up to `limit` listens and history rows changed after change number `cursor`, leaving
    /// out what `except` (the asking device) recorded itself. Returns the batch, the change
    /// number to continue from, and whether more follow.
    pub(crate) fn sync_changes(
        &self,
        cursor: i64,
        except: &str,
        limit: usize,
    ) -> Result<(SyncBatch, i64, bool), String> {
        let run = || -> rusqlite::Result<(SyncBatch, i64, bool)> {
            let mut listens: Vec<(i64, SyncListen)> = self
                .db
                .prepare("SELECT seq,origin,origin_row,track_id,started_at,listened_ms,counted FROM listens WHERE seq>?1 AND origin<>?2 ORDER BY seq LIMIT ?3")?
                .query_map(params![cursor, except, limit as i64 + 1], |r| {
                    Ok((r.get(0)?, SyncListen {
                        origin: r.get(1)?,
                        origin_row: r.get(2)?,
                        track_id: r.get(3)?,
                        started_at: r.get(4)?,
                        listened_ms: r.get(5)?,
                        counted: r.get(6)?,
                    }))
                })?
                .collect::<Result<_, _>>()?;
            let mut heard: Vec<(i64, SyncHeard)> = self
                .db
                .prepare("SELECT seq,origin,track_id,last_heard,plays FROM listen_history WHERE seq>?1 AND origin<>?2 ORDER BY seq LIMIT ?3")?
                .query_map(params![cursor, except, limit as i64 + 1], |r| {
                    Ok((r.get(0)?, SyncHeard {
                        origin: r.get(1)?,
                        track_id: r.get(2)?,
                        last_heard: r.get(3)?,
                        plays: r.get(4)?,
                    }))
                })?
                .collect::<Result<_, _>>()?;
            // Keep the `limit` earliest changes of both tables, so the cursor skips none.
            let mut seqs: Vec<i64> = listens
                .iter()
                .map(|l| l.0)
                .chain(heard.iter().map(|h| h.0))
                .collect();
            seqs.sort_unstable();
            let more = seqs.len() > limit;
            let next = seqs
                .get(limit.min(seqs.len()).wrapping_sub(1))
                .copied()
                .unwrap_or(cursor);
            listens.retain(|l| l.0 <= next);
            heard.retain(|h| h.0 <= next);
            let mut tracks = Vec::new();
            let mut body = self
                .db
                .prepare("SELECT body FROM listened_tracks WHERE track_id=?1")?;
            let ids: std::collections::BTreeSet<&str> =
                listens.iter().map(|l| l.1.track_id.as_str()).collect();
            for id in ids {
                if let Some(bytes) = body
                    .query_row([id], |r| r.get::<_, Vec<u8>>(0))
                    .optional()?
                    && let Ok(json) = String::from_utf8(bytes)
                {
                    tracks.push((id.to_owned(), json));
                }
            }
            let batch = SyncBatch {
                listens: listens.into_iter().map(|l| l.1).collect(),
                heard: heard.into_iter().map(|h| h.1).collect(),
                tracks,
            };
            Ok((batch, next, more))
        };
        run().map_err(|e| e.to_string())
    }
    /// Merges changes from another device. Listens only grow and history only counts up,
    /// so a row passed on again by a third device never goes back. Returns whether anything
    /// new was stored.
    pub(crate) fn merge_sync(&self, batch: &SyncBatch) -> Result<bool, String> {
        self.write(|db| {
            let mut changed = 0;
            for (id, json) in &batch.tracks {
                // Store only metadata that this version can read back.
                if serde_json::from_str::<Track>(json).is_ok_and(|t| &t.id.0 == id) {
                    db.execute(
                        "INSERT INTO listened_tracks(track_id,body) VALUES(?1,?2) ON CONFLICT(track_id) DO NOTHING",
                        params![id, json.as_bytes()],
                    )?;
                }
            }
            for l in batch.listens.iter().filter(|l| l.origin != self.device_id) {
                changed += db.execute(
                    "INSERT INTO listens(track_id,started_at,listened_ms,counted,origin,origin_row,seq) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(origin,origin_row) DO UPDATE SET listened_ms=excluded.listened_ms,counted=excluded.counted,seq=excluded.seq WHERE excluded.listened_ms>listens.listened_ms",
                    params![l.track_id, l.started_at, l.listened_ms.max(0), l.counted, l.origin, l.origin_row, next_seq(db)?],
                )?;
            }
            for h in batch.heard.iter().filter(|h| h.origin != self.device_id) {
                changed += db.execute(
                    "INSERT INTO listen_history(origin,track_id,last_heard,plays,seq) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(origin,track_id) DO UPDATE SET last_heard=MAX(last_heard,excluded.last_heard),plays=MAX(plays,excluded.plays),seq=excluded.seq WHERE excluded.plays>plays OR excluded.last_heard>last_heard",
                    params![h.origin, h.track_id, h.last_heard, h.plays.max(0), next_seq(db)?],
                )?;
            }
            Ok(changed > 0)
        })
    }
    /// Linked devices, apart from forgotten ones.
    pub(crate) fn sync_peers(&self) -> Result<Vec<StoredPeer>, String> {
        let mut statement = self
            .db
            .prepare("SELECT device_id,address,cursor,last_synced,error FROM sync_peers WHERE removed=0 ORDER BY address")
            .map_err(|e| e.to_string())?;
        statement
            .query_map([], |r| {
                Ok(StoredPeer {
                    device_id: r.get(0)?,
                    address: r.get(1)?,
                    cursor: r.get(2)?,
                    last_synced: r.get(3)?,
                    error: r.get(4)?,
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())
    }
    /// Remembers a device at `address`. A forgotten device comes back only when `relink`.
    pub(crate) fn add_sync_peer(
        &self,
        device_id: &str,
        address: &str,
        relink: bool,
    ) -> Result<(), String> {
        if device_id == self.device_id {
            return Ok(());
        }
        self.db
            .execute(
                "INSERT INTO sync_peers(device_id,address) VALUES(?1,?2) ON CONFLICT(device_id) DO UPDATE SET address=excluded.address,removed=CASE WHEN ?3 THEN 0 ELSE removed END",
                params![device_id, address, relink],
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    /// Whether a device was forgotten here and should be refused.
    pub(crate) fn sync_peer_removed(&self, device_id: &str) -> Result<bool, String> {
        self.db
            .query_row(
                "SELECT removed FROM sync_peers WHERE device_id=?1",
                [device_id],
                |r| r.get(0),
            )
            .optional()
            .map(|removed| removed.unwrap_or(false))
            .map_err(|e| e.to_string())
    }
    /// Records the outcome of an exchange with a device: its new cursor, or an error.
    pub(crate) fn sync_peer_result(
        &self,
        device_id: &str,
        result: Result<i64, &str>,
    ) -> Result<(), String> {
        match result {
            Ok(cursor) => self.db.execute(
                "UPDATE sync_peers SET cursor=?2,last_synced=?3,error=NULL WHERE device_id=?1",
                params![device_id, cursor, now()],
            ),
            Err(error) => self.db.execute(
                "UPDATE sync_peers SET error=?2 WHERE device_id=?1",
                params![device_id, error],
            ),
        }
        .map(|_| ())
        .map_err(|e| e.to_string())
    }
    /// Stops exchanging with a device and refuses it from now on.
    pub(crate) fn forget_sync_peer(&self, device_id: &str) -> Result<(), String> {
        self.db
            .execute(
                "UPDATE sync_peers SET removed=1,cursor=0,error=NULL WHERE device_id=?1",
                [device_id],
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    pub(crate) fn sync_enabled(&self) -> Result<bool, String> {
        Ok(self.get_json("sync_enabled")?.is_some_and(|v| v == b"true"))
    }
    pub(crate) fn set_sync_enabled(&self, enabled: bool) -> Result<(), String> {
        self.put_json("sync_enabled", if enabled { b"true" } else { b"false" })
    }
    /// Reads and touches a previously validated artwork cache entry.
    pub(crate) fn cached_artwork(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        let path: Option<String> = self
            .db
            .query_row("SELECT path FROM cache WHERE k=?1", [key], |r| r.get(0))
            .optional()
            .map_err(|e| e.to_string())?;
        let Some(path) = path else { return Ok(None) };
        match std::fs::read(self.root.join("cache").join(&path)) {
            Ok(bytes) => {
                self.db
                    .execute("UPDATE cache SET atime=?2 WHERE k=?1", params![key, now()])
                    .map_err(|e| e.to_string())?;
                Ok(Some(bytes))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.db
                    .execute("DELETE FROM cache WHERE k=?1", [key])
                    .map_err(|e| e.to_string())?;
                Ok(None)
            }
            Err(e) => Err(e.to_string()),
        }
    }
    /// Atomically stores artwork and evicts least-recently-used files to the configured bound.
    pub(crate) fn cache_artwork(&self, key: &str, bytes: &[u8], limit: u64) -> Result<(), String> {
        if bytes.len() as u64 > limit {
            return Ok(());
        }
        let dir = self.root.join("cache");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let name = format!("{:x}.art", Sha256::digest(key.as_bytes()));
        let tmp = dir.join(format!("{name}.tmp"));
        std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, dir.join(&name)).map_err(|e| e.to_string())?;
        self.db.execute("INSERT INTO cache(k,path,size,atime) VALUES(?1,?2,?3,?4) ON CONFLICT(k) DO UPDATE SET path=excluded.path,size=excluded.size,atime=excluded.atime", params![key, name, bytes.len() as i64, now()]).map_err(|e| e.to_string())?;
        while self.cache_bytes()? > limit {
            let row: Option<(String, String)> = self
                .db
                .query_row(
                    "SELECT k,path FROM cache ORDER BY atime ASC LIMIT 1",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(|e| e.to_string())?;
            let Some((old_key, old_path)) = row else {
                break;
            };
            let _ = std::fs::remove_file(dir.join(old_path));
            self.db
                .execute("DELETE FROM cache WHERE k=?1", [old_key])
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    /// Directory for librespot's encrypted audio files, which it evicts itself within
    /// `audio_budget`. Kept apart from artwork so neither cache evicts the other's files.
    pub(crate) fn audio_dir(&self) -> PathBuf {
        self.root.join("audio")
    }
    /// Re-measures cached audio on disk; walks the directory, so call it rarely.
    pub(crate) fn measure_audio(&self) {
        self.audio_bytes.set(self.audio_dir_bytes());
    }
    /// Artwork bytes from the index plus audio bytes at their last measurement.
    pub(crate) fn used_bytes(&self) -> Result<u64, String> {
        Ok(self.cache_bytes()? + self.audio_bytes.get())
    }
    fn audio_dir_bytes(&self) -> u64 {
        fn walk(dir: &Path) -> u64 {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return 0;
            };
            entries
                .flatten()
                .map(|entry| match entry.metadata() {
                    Ok(meta) if meta.is_dir() => walk(&entry.path()),
                    Ok(meta) => meta.len(),
                    Err(_) => 0,
                })
                .sum()
        }
        walk(&self.audio_dir())
    }
    /// Deletes only disposable cached files and rows, preserving settings and session.
    pub(crate) fn clear_cache(&self) -> Result<(), String> {
        for dir in [self.root.join("cache"), self.audio_dir()] {
            if dir.exists() {
                std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
            }
        }
        self.audio_bytes.set(0);
        self.db
            .execute("DELETE FROM lyrics_cache", [])
            .map_err(|e| e.to_string())?;
        self.db
            .execute("DELETE FROM cache", [])
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    /// Returns accounted disposable bytes recorded in the bounded cache index.
    pub(crate) fn cache_bytes(&self) -> Result<u64, String> {
        self.db
            .query_row("SELECT COALESCE(SUM(size),0) FROM cache", [], |r| {
                r.get::<_, i64>(0)
            })
            .map(|n| n.max(0) as u64)
            .map_err(|e| e.to_string())
    }
}

/// Takes the next change number. Call inside `Storage::write`.
fn next_seq(db: &Connection) -> rusqlite::Result<i64> {
    db.query_row(
        "INSERT INTO kv(k,v) VALUES('sync_seq',1) ON CONFLICT(k) DO UPDATE SET v=v+1 RETURNING v",
        [],
        |r| r.get(0),
    )
}

/// Adds what merging with other devices needs: the device each listen and history row
/// comes from, its row there, and a change number that orders every write. Rows from
/// before sync belong to this device.
fn migrate_for_sync(db: &Connection, device_id: &str) -> rusqlite::Result<()> {
    let has_origin = |table: &str| -> rusqlite::Result<bool> {
        db.prepare("SELECT 1 FROM pragma_table_info(?1) WHERE name='origin'")?
            .exists([table])
    };
    db.execute_batch("BEGIN IMMEDIATE")?;
    let migrate = || -> rusqlite::Result<()> {
        let clock = |db: &Connection| -> rusqlite::Result<i64> {
            db.query_row(
                "SELECT COALESCE((SELECT v FROM kv WHERE k='sync_seq'),0)",
                [],
                |r| r.get(0),
            )
        };
        if !has_origin("listens")? {
            let base = clock(db)?;
            db.execute_batch("ALTER TABLE listens ADD COLUMN origin TEXT; ALTER TABLE listens ADD COLUMN origin_row INTEGER; ALTER TABLE listens ADD COLUMN seq INTEGER;")?;
            db.execute(
                "UPDATE listens SET origin=?1,origin_row=id,seq=id+?2",
                params![device_id, base],
            )?;
            db.execute(
                "INSERT INTO kv(k,v) VALUES('sync_seq',?1) ON CONFLICT(k) DO UPDATE SET v=excluded.v",
                [base + db.query_row("SELECT COALESCE(MAX(id),0) FROM listens", [], |r| r.get::<_, i64>(0))?],
            )?;
        }
        if !has_origin("listen_history")? {
            let base = clock(db)?;
            db.execute_batch("CREATE TABLE listen_history_merged(origin TEXT NOT NULL, track_id TEXT NOT NULL, last_heard INTEGER NOT NULL, plays INTEGER NOT NULL, seq INTEGER NOT NULL, PRIMARY KEY(origin,track_id));")?;
            db.execute(
                "INSERT INTO listen_history_merged SELECT ?1,track_id,last_heard,plays,rowid+?2 FROM listen_history",
                params![device_id, base],
            )?;
            db.execute(
                "INSERT INTO kv(k,v) VALUES('sync_seq',?1) ON CONFLICT(k) DO UPDATE SET v=excluded.v",
                [base + db.query_row("SELECT COALESCE(MAX(rowid),0) FROM listen_history", [], |r| r.get::<_, i64>(0))?],
            )?;
            db.execute_batch("DROP TABLE listen_history; ALTER TABLE listen_history_merged RENAME TO listen_history;")?;
        }
        db.execute_batch("CREATE UNIQUE INDEX IF NOT EXISTS listens_origin ON listens(origin,origin_row); CREATE INDEX IF NOT EXISTS listens_seq ON listens(seq); CREATE INDEX IF NOT EXISTS listen_history_seq ON listen_history(seq);")
    };
    match migrate() {
        Ok(()) => db.execute_batch("COMMIT"),
        Err(error) => {
            let _ = db.execute_batch("ROLLBACK");
            Err(error)
        }
    }
}

/// Share of the total cache limit given to artwork; audio files take the rest.
pub(crate) fn artwork_budget(total: u64) -> u64 {
    total / 5
}
/// The audio share of the total cache limit.
pub(crate) fn audio_budget(total: u64) -> u64 {
    total - artwork_budget(total)
}

/// Returns conservative first-run settings without embedding a client ID.
pub(crate) fn default_settings() -> Settings {
    Settings {
        client_id: None,
        cache_limit_bytes: 1_000_000_000,
        theme_path: None,
        autoplay: true,
    }
}
/// Checks persisted setting ranges and client-ID syntax.
pub(super) fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn valid_settings(s: &Settings) -> bool {
    (16 * 1024 * 1024..=100_000_000_000).contains(&s.cache_limit_bytes)
        && s.client_id
            .as_ref()
            .map(|x| x.len() <= 128 && x.chars().all(|c| c.is_ascii_alphanumeric()))
            .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lyrics_cache_expires_success_and_missing_entries_and_clear_removes_both() {
        use crate::domain::{LyricsCredit, LyricsSource, LyricsSync};
        let root = std::env::temp_dir().join(format!("scytify-lyrics-{}", rand::random::<u64>()));
        let store = Storage::open_at(&root).unwrap();
        let track_id = SpotifyId("0123456789012345678901".into());
        let lyrics = Some(Lyrics {
            track_id: track_id.clone(),
            sync: LyricsSync::Static,
            lines: Vec::new(),
            credit: LyricsCredit {
                source: LyricsSource::Spotify,
                uploader: None,
                maker: None,
            },
        });
        let timestamp = now();
        store
            .cache_lyrics_at(&track_id, &lyrics, timestamp)
            .unwrap();
        assert_eq!(
            store
                .cached_lyrics_at(&track_id, timestamp + 30 * 86400 - 1)
                .unwrap(),
            Some(lyrics.clone())
        );
        assert_eq!(
            store
                .cached_lyrics_at(&track_id, timestamp + 30 * 86400)
                .unwrap(),
            None
        );
        assert_eq!(
            store
                .db
                .query_row("SELECT COUNT(*) FROM lyrics_cache", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        store.cache_lyrics_at(&track_id, &None, timestamp).unwrap();
        assert_eq!(
            store
                .cached_lyrics_at(&track_id, timestamp + 86400 - 1)
                .unwrap(),
            Some(None)
        );
        assert_eq!(
            store
                .cached_lyrics_at(&track_id, timestamp + 86400)
                .unwrap(),
            None
        );
        store.cache_lyrics(&track_id, &lyrics).unwrap();
        drop(store);
        let store = Storage::open_at(&root).unwrap();
        assert_eq!(store.cached_lyrics(&track_id).unwrap(), Some(lyrics));
        store.save_settings(&default_settings()).unwrap();
        store.clear_cache().unwrap();
        assert_eq!(store.cached_lyrics(&track_id).unwrap(), None);
        assert!(store.get_json("settings").unwrap().is_some());
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Listen history survives a restart and Clear cache, and repeat plays only count up.
    #[test]
    fn listen_history_persists_and_survives_clearing_the_cache() {
        let root = std::env::temp_dir().join(format!("scytify-history-{}", rand::random::<u64>()));
        let store = Storage::open_at(&root).unwrap();
        let a = SpotifyId("0123456789012345678901".into());
        let b = SpotifyId("1123456789012345678901".into());
        store.record_listens(std::slice::from_ref(&a)).unwrap();
        store.record_listens(&[a.clone(), b.clone()]).unwrap();
        store.clear_cache().unwrap();
        drop(store);
        let store = Storage::open_at(&root).unwrap();
        assert_eq!(store.heard_tracks().unwrap(), [a.clone(), b].into());
        let plays: i64 = store
            .db
            .query_row(
                "SELECT plays FROM listen_history WHERE track_id=?1",
                [&a.0],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(plays, 2);
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Audio files count toward usage and are removed with the rest of the cache.
    #[test]
    fn audio_cache_is_measured_and_cleared() {
        assert_eq!(artwork_budget(1000) + audio_budget(1000), 1000);
        let root = std::env::temp_dir().join(format!("scytify-audio-{}", rand::random::<u64>()));
        let store = Storage::open_at(&root).unwrap();
        let shard = store.audio_dir().join("ab");
        std::fs::create_dir_all(&shard).unwrap();
        std::fs::write(shard.join("cdef"), [0u8; 100]).unwrap();
        store.cache_artwork("art", &[1; 8], 1000).unwrap();
        store.measure_audio();
        assert_eq!(store.used_bytes().unwrap(), 108);
        store.clear_cache().unwrap();
        assert!(!store.audio_dir().exists());
        assert_eq!(store.used_bytes().unwrap(), 0);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn artwork_cache_accounts_reads_and_enforces_total_limit() {
        let root = std::env::temp_dir().join(format!("scytify-cache-{}", rand::random::<u64>()));
        let store = Storage::open_at(&root).unwrap();
        store.cache_artwork("one", &[1; 8], 12).unwrap();
        assert_eq!(store.cached_artwork("one").unwrap(), Some(vec![1; 8]));
        store.cache_artwork("two", &[2; 8], 12).unwrap();
        assert!(store.cache_bytes().unwrap() <= 12);
        store.clear_cache().unwrap();
        assert_eq!(store.cache_bytes().unwrap(), 0);
        drop(store);
        let _ = std::fs::remove_dir_all(root);
    }

    fn track(id: char) -> Track {
        Track {
            id: SpotifyId(id.to_string().repeat(22)),
            name: format!("Song {id}"),
            artists: Vec::new(),
            album: None,
            duration_ms: 200_000,
            playable: true,
            source_index: None,
        }
    }

    /// Stats and history from before sync become this device's; listens and history then
    /// pass from device to device, through a third one too, without counting anything twice
    /// or going back.
    #[test]
    fn listening_merges_across_devices_and_relays_without_duplicates() {
        let dir = |name: &str| {
            std::env::temp_dir().join(format!("scytify-sync-{name}-{}", rand::random::<u64>()))
        };
        let (root_a, root_b, root_c) = (dir("a"), dir("b"), dir("c"));
        // Device a predates sync.
        std::fs::create_dir_all(&root_a).unwrap();
        let old = Connection::open(root_a.join("metadata.sqlite3")).unwrap();
        old.execute_batch("CREATE TABLE listen_history(track_id TEXT PRIMARY KEY, last_heard INTEGER NOT NULL, plays INTEGER NOT NULL); INSERT INTO listen_history VALUES('xxxxxxxxxxxxxxxxxxxxxx',5,3); CREATE TABLE listens(id INTEGER PRIMARY KEY, track_id TEXT NOT NULL, started_at INTEGER NOT NULL, listened_ms INTEGER NOT NULL, counted INTEGER NOT NULL); CREATE TABLE listened_tracks(track_id TEXT PRIMARY KEY, body BLOB NOT NULL);").unwrap();
        drop(old);
        let a = Storage::open_at(&root_a).unwrap();
        let b = Storage::open_at(&root_b).unwrap();
        let c = Storage::open_at(&root_c).unwrap();
        let row = a.save_listen(None, &track('a'), 100, 40_000, true).unwrap();
        b.save_listen(None, &track('b'), 200, 10_000, false)
            .unwrap();
        b.record_listens(&[SpotifyId("x".repeat(22))]).unwrap();
        // Pull everything both ways, then pass a's listens on to c through b.
        let pull = |to: &Storage, from: &Storage, cursor: i64| {
            let (batch, next, more) = from.sync_changes(cursor, to.device_id(), 500).unwrap();
            assert!(!more);
            (to.merge_sync(&batch).unwrap(), next)
        };
        let (changed, b_from_a) = pull(&b, &a, 0);
        assert!(changed);
        pull(&a, &b, 0);
        pull(&c, &b, 0);
        // A listen that keeps playing reaches others again; repeating a pull changes nothing.
        a.save_listen(Some(row), &track('a'), 100, 90_000, true)
            .unwrap();
        assert!(pull(&b, &a, b_from_a).0);
        assert!(!pull(&b, &a, 0).0);
        pull(&c, &b, 0);
        for store in [&a, &b, &c] {
            let mut totals: Vec<(String, u64, u64)> = store
                .listen_totals(i64::MIN)
                .unwrap()
                .into_iter()
                .map(|(t, ms, plays)| (t.id.0, ms, plays))
                .collect();
            totals.sort();
            assert_eq!(
                totals,
                [("a".repeat(22), 90_000, 1), ("b".repeat(22), 10_000, 0)]
            );
            let plays: i64 = store
                .db
                .query_row(
                    "SELECT SUM(plays) FROM listen_history WHERE track_id=?1",
                    ["x".repeat(22)],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(plays, 4);
        }
        drop((a, b, c));
        for root in [root_a, root_b, root_c] {
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}
