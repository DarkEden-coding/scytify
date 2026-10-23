use crate::domain::{Lyrics, SessionSnapshot, Settings, SpotifyId};
use directories::ProjectDirs;
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Durable non-secret backend storage. Authentication material is never accepted here.
pub(crate) struct Storage {
    root: PathBuf,
    db: Connection,
    /// Cached audio size at its last measurement.
    audio_bytes: std::cell::Cell<u64>,
}

impl Storage {
    /// Opens the per-user data database and applies its small versioned schema.
    pub(crate) fn open() -> Result<Self, String> {
        let dirs = ProjectDirs::from("dev", "CustomSpotify", "CustomSpotify")
            .ok_or("user data directory is unavailable")?;
        Self::open_at(dirs.data_local_dir())
    }
    /// Opens storage at an explicit path, primarily for isolated regression tests.
    pub(crate) fn open_at(root: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
        let db = Connection::open(root.join("metadata.sqlite3")).map_err(|e| e.to_string())?;
        db.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| e.to_string())?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS kv(k TEXT PRIMARY KEY,v BLOB NOT NULL); CREATE TABLE IF NOT EXISTS cache(k TEXT PRIMARY KEY,path TEXT NOT NULL,size INTEGER NOT NULL,atime INTEGER NOT NULL);").map_err(|e| e.to_string())?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS lyrics_cache(track_id TEXT PRIMARY KEY, body BLOB NOT NULL, stored_at INTEGER NOT NULL, expires_at INTEGER NOT NULL);").map_err(|e| e.to_string())?;
        // Not a cache: Discover relies on it to skip songs heard before, so Clear cache keeps it.
        db.execute_batch("CREATE TABLE IF NOT EXISTS listen_history(track_id TEXT PRIMARY KEY, last_heard INTEGER NOT NULL, plays INTEGER NOT NULL);").map_err(|e| e.to_string())?;
        let store = Self {
            root: root.to_owned(),
            db,
            audio_bytes: std::cell::Cell::new(0),
        };
        store.prune_lyrics(now())?;
        Ok(store)
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
        for id in track_ids {
            self.db
                .execute(
                    "INSERT INTO listen_history(track_id,last_heard,plays) VALUES(?1,?2,1) ON CONFLICT(track_id) DO UPDATE SET last_heard=excluded.last_heard,plays=plays+1",
                    params![id.0, timestamp],
                )
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    /// Every track heard on this device, or seen in Spotify's recent plays.
    pub(crate) fn heard_tracks(&self) -> Result<std::collections::HashSet<SpotifyId>, String> {
        let mut statement = self
            .db
            .prepare("SELECT track_id FROM listen_history")
            .map_err(|e| e.to_string())?;
        statement
            .query_map([], |r| r.get::<_, String>(0).map(SpotifyId))
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())
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
        let root =
            std::env::temp_dir().join(format!("custom-spotify-lyrics-{}", rand::random::<u64>()));
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
        let root =
            std::env::temp_dir().join(format!("custom-spotify-history-{}", rand::random::<u64>()));
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
        let root =
            std::env::temp_dir().join(format!("custom-spotify-audio-{}", rand::random::<u64>()));
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
        let root =
            std::env::temp_dir().join(format!("custom-spotify-cache-{}", rand::random::<u64>()));
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
}
