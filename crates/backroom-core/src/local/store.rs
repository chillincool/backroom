//! The SQLite database behind standalone mode: one file holding networks,
//! joined channels, every message and read markers.
//!
//! Calls are synchronous and quick (an indexed insert or a page of one
//! buffer), so the coordinator makes them directly.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};

use crate::model::{ChatMessage, MessageKind, NetworkId, buffer_key};

#[derive(Debug, thiserror::Error)]
pub enum LocalError {
    #[error("history database: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("history database: {0}")]
    Corrupt(String),
}

/// Schema versions, applied in order; `PRAGMA user_version` counts them.
const MIGRATIONS: &[&str] = &[r"
    CREATE TABLE network (
        id INTEGER PRIMARY KEY,
        attributes TEXT NOT NULL            -- JSON object, no passwords
    );
    CREATE TABLE channel (
        network INTEGER NOT NULL REFERENCES network(id) ON DELETE CASCADE,
        key TEXT NOT NULL,                  -- buffer_key(name)
        name TEXT NOT NULL,
        has_key INTEGER NOT NULL DEFAULT 0, -- its key is in the vault
        PRIMARY KEY (network, key)
    );
    CREATE TABLE message (
        id INTEGER PRIMARY KEY,
        network INTEGER NOT NULL REFERENCES network(id) ON DELETE CASCADE,
        buffer TEXT NOT NULL,               -- buffer_key(buffer_name)
        buffer_name TEXT NOT NULL,
        time INTEGER NOT NULL,              -- milliseconds since 1970
        kind TEXT NOT NULL,
        sender TEXT NOT NULL,
        text TEXT NOT NULL,
        own INTEGER NOT NULL,
        msgid TEXT
    );
    CREATE INDEX message_buffer_time ON message (network, buffer, time);
    CREATE UNIQUE INDEX message_msgid ON message (network, msgid) WHERE msgid IS NOT NULL;
    CREATE TABLE read_marker (
        network INTEGER NOT NULL REFERENCES network(id) ON DELETE CASCADE,
        buffer TEXT NOT NULL,
        time INTEGER NOT NULL,
        PRIMARY KEY (network, buffer)
    );
"];

/// A network's saved attributes.
pub type Attributes = BTreeMap<String, String>;

pub struct LocalStore {
    db: Mutex<Connection>,
}

impl LocalStore {
    /// Opens (creating if needed) the database at `path`.
    pub fn open(path: &Path) -> Result<Self, LocalError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| LocalError::Corrupt(e.to_string()))?;
        }
        let db = Connection::open(path)?;
        db.pragma_update(None, "journal_mode", "WAL")?;
        // WAL with NORMAL survives app crashes; only a power cut can lose
        // the last messages.
        db.pragma_update(None, "synchronous", "NORMAL")?;
        Self::init(db)
    }

    /// A throwaway database, for tests.
    pub fn in_memory() -> Result<Self, LocalError> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(db: Connection) -> Result<Self, LocalError> {
        db.pragma_update(None, "foreign_keys", true)?;
        let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
        let version = usize::try_from(version).unwrap_or(usize::MAX);
        if version > MIGRATIONS.len() {
            return Err(LocalError::Corrupt(format!(
                "written by a newer Backroom (schema {version})"
            )));
        }
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(version) {
            db.execute_batch(&format!(
                "BEGIN; {sql} PRAGMA user_version = {}; COMMIT;",
                i + 1
            ))?;
        }
        Ok(LocalStore { db: Mutex::new(db) })
    }

    fn db(&self) -> MutexGuard<'_, Connection> {
        self.db.lock().unwrap_or_else(PoisonError::into_inner)
    }

    // -- networks -------------------------------------------------------------

    pub fn networks(&self) -> Result<Vec<(NetworkId, Attributes)>, LocalError> {
        let db = self.db();
        let mut stmt = db.prepare("SELECT id, attributes FROM network ORDER BY id")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        rows.map(|row| {
            let (id, json) = row?;
            let attrs =
                serde_json::from_str(&json).map_err(|e| LocalError::Corrupt(e.to_string()))?;
            Ok((id.to_string(), attrs))
        })
        .collect()
    }

    pub fn add_network(
        &self,
        attributes: &BTreeMap<String, String>,
    ) -> Result<NetworkId, LocalError> {
        let db = self.db();
        db.execute(
            "INSERT INTO network (attributes) VALUES (?1)",
            [json(attributes)],
        )?;
        Ok(db.last_insert_rowid().to_string())
    }

    pub fn set_network(
        &self,
        id: &str,
        attributes: &BTreeMap<String, String>,
    ) -> Result<(), LocalError> {
        self.db().execute(
            "UPDATE network SET attributes = ?2 WHERE id = ?1",
            params![net(id)?, json(attributes)],
        )?;
        Ok(())
    }

    /// Deletes a network with its channels, history and read markers.
    pub fn remove_network(&self, id: &str) -> Result<(), LocalError> {
        self.db()
            .execute("DELETE FROM network WHERE id = ?1", [net(id)?])?;
        Ok(())
    }

    // -- channels -------------------------------------------------------------

    /// Channels to join on connect, as last seen, and whether each has a
    /// key in the vault.
    pub fn channels(&self, network: &str) -> Result<Vec<(String, bool)>, LocalError> {
        let db = self.db();
        let mut stmt =
            db.prepare("SELECT name, has_key FROM channel WHERE network = ?1 ORDER BY key")?;
        let rows = stmt.query_map([net(network)?], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Remembers a joined channel; `has_key` once its key is saved (a
    /// later join without one keeps it).
    pub fn add_channel(&self, network: &str, name: &str, has_key: bool) -> Result<(), LocalError> {
        self.db().execute(
            "INSERT INTO channel (network, key, name, has_key) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (network, key)
             DO UPDATE SET name = excluded.name, has_key = max(has_key, excluded.has_key)",
            params![net(network)?, buffer_key(name), name, has_key],
        )?;
        Ok(())
    }

    pub fn remove_channel(&self, network: &str, name: &str) -> Result<(), LocalError> {
        self.db().execute(
            "DELETE FROM channel WHERE network = ?1 AND key = ?2",
            params![net(network)?, buffer_key(name)],
        )?;
        Ok(())
    }

    // -- history --------------------------------------------------------------

    /// Stores a message. One the network already sent (same `msgid`) is
    /// skipped.
    pub fn append(&self, network: &str, buffer: &str, msg: &ChatMessage) -> Result<(), LocalError> {
        self.db().execute(
            "INSERT INTO message (network, buffer, buffer_name, time, kind, sender, text, own, msgid)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT DO NOTHING",
            params![
                net(network)?,
                buffer_key(buffer),
                buffer,
                msg.time.timestamp_millis(),
                kind_name(msg.kind),
                msg.sender,
                msg.text,
                msg.own,
                msg.id,
            ],
        )?;
        Ok(())
    }

    /// Up to `limit` messages before `before` (or the latest), oldest
    /// first.
    pub fn before(
        &self,
        network: &str,
        buffer: &str,
        before: Option<DateTime<Utc>>,
        limit: usize,
    ) -> Result<Vec<ChatMessage>, LocalError> {
        let db = self.db();
        let mut stmt = db.prepare(
            "SELECT time, kind, sender, text, own, msgid FROM message
             WHERE network = ?1 AND buffer = ?2 AND time < ?3
             ORDER BY time DESC, id DESC LIMIT ?4",
        )?;
        let before = before.map_or(i64::MAX, |t| t.timestamp_millis());
        let mut out = stmt
            .query_map(
                params![net(network)?, buffer_key(buffer), before, limit as i64],
                row_message,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        out.reverse();
        Ok(out)
    }

    /// Messages in `buffer` containing `text` (case-insensitive for
    /// ASCII), the latest `limit`, oldest first.
    pub fn search(
        &self,
        network: &str,
        buffer: &str,
        text: &str,
        limit: usize,
    ) -> Result<Vec<ChatMessage>, LocalError> {
        let db = self.db();
        let mut stmt = db.prepare(
            "SELECT time, kind, sender, text, own, msgid FROM message
             WHERE network = ?1 AND buffer = ?2 AND kind IN ('privmsg', 'notice', 'action')
               AND text LIKE ?3 ESCAPE '\\'
             ORDER BY time DESC, id DESC LIMIT ?4",
        )?;
        let pattern = format!(
            "%{}%",
            text.replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        let mut out = stmt
            .query_map(
                params![net(network)?, buffer_key(buffer), pattern, limit as i64],
                row_message,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        out.reverse();
        Ok(out)
    }

    /// Buffers with real conversation (messages or actions), for private
    /// conversations to come back after a restart. Names as last seen.
    pub fn conversations(&self, network: &str) -> Result<Vec<String>, LocalError> {
        let db = self.db();
        let mut stmt = db.prepare(
            "SELECT buffer_name FROM message
             WHERE id IN (SELECT max(id) FROM message
                          WHERE network = ?1 AND kind IN ('privmsg', 'action')
                          GROUP BY buffer)
             ORDER BY time",
        )?;
        let rows = stmt.query_map([net(network)?], |r| r.get(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    // -- read markers ---------------------------------------------------------

    pub fn read_marker(
        &self,
        network: &str,
        buffer: &str,
    ) -> Result<Option<DateTime<Utc>>, LocalError> {
        let ms: Option<i64> = self
            .db()
            .query_row(
                "SELECT time FROM read_marker WHERE network = ?1 AND buffer = ?2",
                params![net(network)?, buffer_key(buffer)],
                |r| r.get(0),
            )
            .optional()?;
        Ok(ms.and_then(DateTime::from_timestamp_millis))
    }

    /// Moves the marker forward to `time`; never back, like soju. Returns
    /// the marker afterwards.
    pub fn mark_read(
        &self,
        network: &str,
        buffer: &str,
        time: DateTime<Utc>,
    ) -> Result<Option<DateTime<Utc>>, LocalError> {
        self.db().execute(
            "INSERT INTO read_marker (network, buffer, time) VALUES (?1, ?2, ?3)
             ON CONFLICT (network, buffer) DO UPDATE SET time = max(time, excluded.time)",
            params![net(network)?, buffer_key(buffer), time.timestamp_millis()],
        )?;
        self.read_marker(network, buffer)
    }
}

/// Network IDs are the integer row IDs, as strings.
fn net(id: &str) -> Result<i64, LocalError> {
    id.parse()
        .map_err(|_| LocalError::Corrupt(format!("not a local network ID: {id}")))
}

fn json(attributes: &BTreeMap<String, String>) -> String {
    serde_json::to_string(attributes).unwrap_or_else(|_| "{}".into())
}

fn row_message(r: &rusqlite::Row<'_>) -> rusqlite::Result<ChatMessage> {
    let kind: String = r.get(1)?;
    Ok(ChatMessage {
        time: DateTime::from_timestamp_millis(r.get(0)?).unwrap_or_default(),
        kind: kind_from_name(&kind),
        sender: r.get(2)?,
        text: r.get(3)?,
        own: r.get(4)?,
        id: r.get(5)?,
    })
}

const KINDS: &[(MessageKind, &str)] = &[
    (MessageKind::Privmsg, "privmsg"),
    (MessageKind::Notice, "notice"),
    (MessageKind::Action, "action"),
    (MessageKind::Join, "join"),
    (MessageKind::Part, "part"),
    (MessageKind::Kick, "kick"),
    (MessageKind::Topic, "topic"),
    (MessageKind::Quit, "quit"),
    (MessageKind::Nick, "nick"),
    (MessageKind::Mode, "mode"),
    (MessageKind::Invite, "invite"),
    (MessageKind::Reply, "reply"),
    (MessageKind::Error, "error"),
];

fn kind_name(kind: MessageKind) -> &'static str {
    KINDS
        .iter()
        .find(|(k, _)| *k == kind)
        .map_or("reply", |(_, n)| n)
}

fn kind_from_name(name: &str) -> MessageKind {
    KINDS
        .iter()
        .find(|(_, n)| *n == name)
        .map_or(MessageKind::Reply, |(k, _)| *k)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(text: &str, secs: i64, kind: MessageKind) -> ChatMessage {
        ChatMessage {
            id: None,
            time: DateTime::from_timestamp(secs, 0).unwrap(),
            sender: "erin".into(),
            kind,
            text: text.into(),
            own: false,
        }
    }

    fn store_with_network() -> (LocalStore, String) {
        let store = LocalStore::in_memory().unwrap();
        let attrs = BTreeMap::from([("host".to_owned(), "irc.example".to_owned())]);
        let id = store.add_network(&attrs).unwrap();
        (store, id)
    }

    #[test]
    fn every_kind_has_a_name() {
        for (kind, name) in KINDS {
            assert_eq!(kind_from_name(kind_name(*kind)), *kind, "{name}");
        }
    }

    #[test]
    fn networks_round_trip() {
        let (store, id) = store_with_network();
        let mut attrs = store.networks().unwrap()[0].1.clone();
        attrs.insert("name".into(), "Example".into());
        store.set_network(&id, &attrs).unwrap();
        assert_eq!(store.networks().unwrap(), [(id.clone(), attrs)]);
        store.remove_network(&id).unwrap();
        assert!(store.networks().unwrap().is_empty());
    }

    #[test]
    fn history_pages_oldest_first() {
        let (store, id) = store_with_network();
        for i in 1..=5 {
            store
                .append(
                    &id,
                    "#Rust",
                    &msg(&format!("m{i}"), i, MessageKind::Privmsg),
                )
                .unwrap();
        }
        let texts = |v: Vec<ChatMessage>| v.into_iter().map(|m| m.text).collect::<Vec<_>>();
        assert_eq!(
            texts(store.before(&id, "#rust", None, 2).unwrap()),
            ["m4", "m5"]
        );
        let before = DateTime::from_timestamp(4, 0);
        assert_eq!(
            texts(store.before(&id, "#RUST", before, 10).unwrap()),
            ["m1", "m2", "m3"]
        );
        assert!(store.before(&id, "#other", None, 10).unwrap().is_empty());
    }

    #[test]
    fn msgid_duplicates_are_skipped() {
        let (store, id) = store_with_network();
        let mut m = msg("hi", 1, MessageKind::Privmsg);
        m.id = Some("abc".into());
        store.append(&id, "#a", &m).unwrap();
        store.append(&id, "#a", &m).unwrap();
        store
            .append(&id, "#a", &msg("hi", 1, MessageKind::Privmsg))
            .unwrap();
        assert_eq!(store.before(&id, "#a", None, 10).unwrap().len(), 2);
    }

    #[test]
    fn search_matches_text_literally() {
        let (store, id) = store_with_network();
        for (i, text) in ["Hello world", "100% sure", "a_b", "hello again"]
            .iter()
            .enumerate()
        {
            store
                .append(&id, "#a", &msg(text, i as i64, MessageKind::Privmsg))
                .unwrap();
        }
        store
            .append(&id, "#a", &msg("hello", 9, MessageKind::Join))
            .unwrap();
        let found = |q: &str| {
            store
                .search(&id, "#a", q, 10)
                .unwrap()
                .into_iter()
                .map(|m| m.text)
                .collect::<Vec<_>>()
        };
        assert_eq!(found("HELLO"), ["Hello world", "hello again"]);
        assert_eq!(found("0%"), ["100% sure"]);
        assert_eq!(found("_"), ["a_b"]);
    }

    #[test]
    fn conversations_need_real_messages() {
        let (store, id) = store_with_network();
        store
            .append(&id, "NickServ", &msg("x", 1, MessageKind::Notice))
            .unwrap();
        store
            .append(&id, "erin", &msg("hi", 2, MessageKind::Privmsg))
            .unwrap();
        store
            .append(&id, "Erin", &msg("hey", 3, MessageKind::Privmsg))
            .unwrap();
        assert_eq!(store.conversations(&id).unwrap(), ["Erin"]);
    }

    #[test]
    fn channels_and_markers() {
        let (store, id) = store_with_network();
        store.add_channel(&id, "#Rust", true).unwrap();
        store.add_channel(&id, "#rust", false).unwrap();
        store.add_channel(&id, "#go", false).unwrap();
        assert_eq!(
            store.channels(&id).unwrap(),
            [("#go".to_owned(), false), ("#rust".to_owned(), true)]
        );
        store.remove_channel(&id, "#GO").unwrap();
        assert_eq!(store.channels(&id).unwrap().len(), 1);

        assert_eq!(store.read_marker(&id, "#rust").unwrap(), None);
        let t = |s| DateTime::from_timestamp(s, 0).unwrap();
        assert_eq!(store.mark_read(&id, "#rust", t(20)).unwrap(), Some(t(20)));
        assert_eq!(store.mark_read(&id, "#RUST", t(10)).unwrap(), Some(t(20)));
    }

    #[test]
    fn removing_a_network_deletes_its_history() {
        let (store, id) = store_with_network();
        store.add_channel(&id, "#a", false).unwrap();
        store
            .append(&id, "#a", &msg("x", 1, MessageKind::Privmsg))
            .unwrap();
        store.remove_network(&id).unwrap();
        let db = store.db();
        let count: i64 = db
            .query_row("SELECT count(*) FROM message", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn reopens_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/standalone.sqlite");
        let id = {
            let store = LocalStore::open(&path).unwrap();
            let id = store.add_network(&BTreeMap::new()).unwrap();
            store
                .append(&id, "#a", &msg("kept", 1, MessageKind::Privmsg))
                .unwrap();
            id
        };
        let store = LocalStore::open(&path).unwrap();
        assert_eq!(store.before(&id, "#a", None, 5).unwrap()[0].text, "kept");
    }
}
