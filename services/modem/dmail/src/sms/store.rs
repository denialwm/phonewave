//! The message store: every received PDU as it came, and the messages apps
//! see, multipart ones joined; the archive, the block list and the delivery
//! reports of sent parts.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};

use super::pdu::Deliver;

/// Parts still missing after this long are shown as they are.
const PART_WAIT: i64 = 24 * 60 * 60;
/// Phone numbers name the same conversation when this many last digits match,
/// as on Android: the network sends +39 333…, the SIM's own number can lack
/// the plus, and people type 333…. Fewer digits than `MIN_DIGITS` must match
/// whole.
const MATCH_DIGITS: usize = 9;
const MIN_DIGITS: usize = 7;

/// The schema, one step per version. `PRAGMA user_version` counts the steps a
/// store has taken. New steps go at the end; shipped ones never change.
const MIGRATIONS: [&str; 2] = [
    // 1: messages and received PDUs. Stores from before versions were counted
    // have these tables already.
    "CREATE TABLE IF NOT EXISTS messages (
         id INTEGER PRIMARY KEY,
         peer TEXT NOT NULL,
         outgoing INTEGER NOT NULL,
         text TEXT NOT NULL,
         time INTEGER NOT NULL,
         state TEXT NOT NULL,
         read INTEGER NOT NULL
     );
     CREATE INDEX IF NOT EXISTS messages_by_peer ON messages (peer, time);
     CREATE TABLE IF NOT EXISTS received (
         pdu BLOB PRIMARY KEY,
         arrived INTEGER NOT NULL,
         peer TEXT NOT NULL,
         reference INTEGER,
         total INTEGER,
         sequence INTEGER,
         text TEXT NOT NULL,
         time INTEGER NOT NULL,
         message INTEGER
     );",
    // 2: blocked senders' messages kept apart, the archive, the block list,
    // a delivery report per sent part, and search over the text.
    "ALTER TABLE messages ADD COLUMN blocked INTEGER NOT NULL DEFAULT 0;
     CREATE TABLE archived (peer TEXT PRIMARY KEY);
     CREATE TABLE blocked_numbers (number TEXT PRIMARY KEY);
     CREATE TABLE reports (
         message INTEGER NOT NULL,
         reference INTEGER NOT NULL,
         outcome TEXT
     );
     CREATE INDEX reports_by_message ON reports (message);
     CREATE INDEX reports_by_reference ON reports (reference);
     CREATE VIRTUAL TABLE messages_text USING fts5 (
         text,
         content = 'messages',
         content_rowid = 'id',
         tokenize = 'unicode61 remove_diacritics 2'
     );
     INSERT INTO messages_text (messages_text) VALUES ('rebuild');
     CREATE TRIGGER messages_added AFTER INSERT ON messages BEGIN
         INSERT INTO messages_text (rowid, text) VALUES (new.id, new.text);
     END;
     CREATE TRIGGER messages_deleted AFTER DELETE ON messages BEGIN
         INSERT INTO messages_text (messages_text, rowid, text)
             VALUES ('delete', old.id, old.text);
         DELETE FROM reports WHERE message = old.id;
     END;",
];

pub struct Store {
    db: Connection,
}

/// One row of `ListConversations`: peer, and of its last message the text,
/// time and whether it was outgoing; then the unread count.
pub type Conversation = (String, String, i64, bool, u32);
/// One row of `ListMessages`: id, outgoing, text, time, state.
pub type Entry = (u64, bool, String, i64, String);
/// One row of `Search`: the conversation's peer, then a `ListMessages` row.
pub type Found = (String, u64, bool, String, i64, String);
/// One row of `ListBlockedMessages`: id, sender, text, time.
pub type Blocked = (u64, String, String, i64);

impl Store {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        Self::init(Connection::open(path)?)
    }

    pub fn in_memory() -> rusqlite::Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    /// Takes the schema steps the store has not taken yet, each in a
    /// transaction of its own.
    fn init(mut db: Connection) -> rusqlite::Result<Self> {
        db.execute_batch("PRAGMA journal_mode = WAL")?;
        let done: u32 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
        for (version, sql) in (1u32..).zip(MIGRATIONS).skip(done as usize) {
            let tx = db.transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", version)?;
            tx.commit()?;
        }
        Ok(Self { db })
    }

    /// Stores a received PDU. Returns the new message's id and conversation
    /// peer once it is complete; nothing for a part still waiting for the
    /// others, for a PDU stored before (the network resends what was not
    /// acked), or for a message from a blocked sender.
    pub fn receive(
        &mut self,
        pdu: &[u8],
        m: &Deliver,
        now: i64,
    ) -> rusqlite::Result<Option<(u64, String)>> {
        let time = m.time.unwrap_or(now);
        let tx = self.db.transaction()?;
        let added = tx.execute(
            "INSERT OR IGNORE INTO received
                 (pdu, arrived, peer, reference, total, sequence, text, time)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                pdu,
                now,
                m.sender,
                m.part.map(|p| p.reference),
                m.part.map(|p| p.total),
                m.part.map(|p| p.sequence),
                m.text,
                time
            ],
        )?;
        if added == 0 {
            return Ok(None);
        }
        let done = match m.part {
            None => {
                let (id, peer) = insert_incoming(&tx, &m.sender, &m.text, time)?;
                tx.execute(
                    "UPDATE received SET message = ?1 WHERE pdu = ?2",
                    params![id as i64, pdu],
                )?;
                peer.map(|peer| (id, peer))
            }
            Some(p) => {
                let have: u32 = tx.query_row(
                    "SELECT COUNT(DISTINCT sequence) FROM received
                     WHERE peer = ?1 AND reference = ?2 AND total = ?3 AND message IS NULL",
                    params![m.sender, p.reference, p.total],
                    |r| r.get(0),
                )?;
                if have >= u32::from(p.total) {
                    join_parts(&tx, &m.sender, p.reference, p.total)?
                } else {
                    None
                }
            }
        };
        tx.commit()?;
        Ok(done)
    }

    /// Shows multipart messages whose missing parts never came. Returns the new
    /// messages' ids and peers, blocked senders' left out.
    pub fn flush_stale_parts(&mut self, now: i64) -> rusqlite::Result<Vec<(u64, String)>> {
        let tx = self.db.transaction()?;
        let groups: Vec<(String, u16, u8)> = tx
            .prepare(
                "SELECT peer, reference, total FROM received
                 WHERE message IS NULL AND total IS NOT NULL
                 GROUP BY peer, reference, total HAVING MIN(arrived) < ?1",
            )?
            .query_map([now - PART_WAIT], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let mut shown = Vec::new();
        for (peer, reference, total) in groups {
            shown.extend(join_parts(&tx, &peer, reference, total)?);
        }
        tx.commit()?;
        Ok(shown)
    }

    /// Adds an outgoing message in state `sending`. Its conversation leaves
    /// the archive.
    pub fn add_outgoing(&mut self, peer: &str, text: &str, now: i64) -> rusqlite::Result<u64> {
        let tx = self.db.transaction()?;
        let peer = conversation_peer(&tx, peer)?;
        tx.execute(
            "INSERT INTO messages (peer, outgoing, text, time, state, read)
             VALUES (?1, 1, ?2, ?3, 'sending', 1)",
            params![peer, text, now],
        )?;
        let id = tx.last_insert_rowid() as u64;
        tx.execute("DELETE FROM archived WHERE peer = ?1", [peer])?;
        tx.commit()?;
        Ok(id)
    }

    /// The peer and text of an outgoing message.
    pub fn outgoing(&self, id: u64) -> rusqlite::Result<Option<(String, String)>> {
        self.db
            .query_row(
                "SELECT peer, text FROM messages WHERE id = ?1 AND outgoing = 1",
                [id as i64],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
    }

    pub fn set_state(&self, id: u64, state: &str) -> rusqlite::Result<()> {
        self.db.execute(
            "UPDATE messages SET state = ?2 WHERE id = ?1",
            params![id as i64, state],
        )?;
        Ok(())
    }

    /// Outgoing messages a restart cut off while sending.
    pub fn fail_unsent(&self) -> rusqlite::Result<Vec<u64>> {
        let ids = self
            .db
            .prepare("SELECT id FROM messages WHERE state = 'sending'")?
            .query_map([], |r| r.get::<_, i64>(0))?
            .map(|id| id.map(|id| id as u64))
            .collect::<rusqlite::Result<Vec<_>>>()?;
        self.db.execute(
            "UPDATE messages SET state = 'failed' WHERE state = 'sending'",
            [],
        )?;
        Ok(ids)
    }

    /// Records that a sent part waits for its status report.
    pub fn await_report(&self, id: u64, reference: u16) -> rusqlite::Result<()> {
        self.db.execute(
            "INSERT INTO reports (message, reference) VALUES (?1, ?2)",
            params![id as i64, reference],
        )?;
        Ok(())
    }

    /// Takes a status report's outcome for the newest part sent to
    /// `recipient` with `reference` and still waiting. Once every part of its
    /// message has one, a `sent` message becomes `delivered` if all parts
    /// were, else takes the first other outcome. Returns the message's id and
    /// new state then.
    pub fn report(
        &mut self,
        recipient: &str,
        reference: u8,
        outcome: &str,
    ) -> rusqlite::Result<Option<(u64, String)>> {
        let tx = self.db.transaction()?;
        let waiting: Vec<(i64, i64, String)> = tx
            .prepare(
                "SELECT r.rowid, r.message, m.peer FROM reports r
                 JOIN messages m ON m.id = r.message
                 WHERE r.reference = ?1 AND r.outcome IS NULL ORDER BY r.rowid DESC",
            )?
            .query_map([reference], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let Some((part, id, _)) = waiting
            .into_iter()
            .find(|(_, _, peer)| same_peer(peer, recipient))
        else {
            return Ok(None);
        };
        tx.execute(
            "UPDATE reports SET outcome = ?2 WHERE rowid = ?1",
            params![part, outcome],
        )?;
        let outcomes: Vec<Option<String>> = tx
            .prepare("SELECT outcome FROM reports WHERE message = ?1 ORDER BY rowid")?
            .query_map([id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let state: String =
            tx.query_row("SELECT state FROM messages WHERE id = ?1", [id], |r| {
                r.get(0)
            })?;
        let done = if state == "sent" && outcomes.iter().all(Option::is_some) {
            let state = outcomes
                .into_iter()
                .flatten()
                .find(|o| o != "delivered")
                .unwrap_or_else(|| "delivered".into());
            tx.execute(
                "UPDATE messages SET state = ?2 WHERE id = ?1",
                params![id, state],
            )?;
            Some((id as u64, state))
        } else {
            None
        };
        tx.commit()?;
        Ok(done)
    }

    /// Newest conversation first: the archived ones, or the others.
    pub fn conversations(&self, archived: bool) -> rusqlite::Result<Vec<Conversation>> {
        self.db
            .prepare(
                "SELECT m.peer, m.text, m.time, m.outgoing,
                     (SELECT COUNT(*) FROM messages u
                      WHERE u.peer = m.peer AND u.read = 0 AND u.blocked = 0)
                 FROM messages m
                 WHERE m.id = (SELECT id FROM messages l WHERE l.peer = m.peer AND l.blocked = 0
                               ORDER BY time DESC, id DESC LIMIT 1)
                     AND (m.peer IN (SELECT peer FROM archived)) = ?1
                 ORDER BY m.time DESC",
            )?
            .query_map([archived], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })?
            .collect()
    }

    /// Up to `limit` messages with `peer` older than `before`, newest first.
    pub fn messages(&self, peer: &str, before: i64, limit: u32) -> rusqlite::Result<Vec<Entry>> {
        let peer = conversation_peer(&self.db, peer)?;
        self.db
            .prepare(
                "SELECT id, outgoing, text, time, state FROM messages
                 WHERE peer = ?1 AND blocked = 0 AND time < ?2
                 ORDER BY time DESC, id DESC LIMIT ?3",
            )?
            .query_map(params![peer, before, limit], |r| {
                Ok((
                    r.get::<_, i64>(0)? as u64,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                ))
            })?
            .collect()
    }

    /// Marks a conversation read. Returns its peer.
    pub fn mark_read(&self, peer: &str) -> rusqlite::Result<String> {
        let peer = conversation_peer(&self.db, peer)?;
        self.db.execute(
            "UPDATE messages SET read = 1 WHERE peer = ?1 AND blocked = 0",
            [&peer],
        )?;
        Ok(peer)
    }

    /// Deletes a message. The PDUs it came from stay, so the network resending
    /// one does not bring it back.
    pub fn delete(&self, id: u64) -> rusqlite::Result<bool> {
        Ok(self
            .db
            .execute("DELETE FROM messages WHERE id = ?1", [id as i64])?
            > 0)
    }

    /// Deletes a conversation's messages, as `delete` does, and its archive
    /// flag. Returns its peer; nothing if there was no such conversation.
    pub fn delete_conversation(&mut self, peer: &str) -> rusqlite::Result<Option<String>> {
        let tx = self.db.transaction()?;
        let peer = conversation_peer(&tx, peer)?;
        let deleted = tx.execute(
            "DELETE FROM messages WHERE peer = ?1 AND blocked = 0",
            [&peer],
        )?;
        tx.execute("DELETE FROM archived WHERE peer = ?1", [&peer])?;
        tx.commit()?;
        Ok((deleted > 0).then_some(peer))
    }

    /// Archives a conversation or brings it back. Returns its peer; nothing
    /// if there is no such conversation.
    pub fn set_archived(&self, peer: &str, archived: bool) -> rusqlite::Result<Option<String>> {
        let peer = conversation_peer(&self.db, peer)?;
        let exists: bool = self.db.query_row(
            "SELECT EXISTS (SELECT 1 FROM messages WHERE peer = ?1 AND blocked = 0)",
            [&peer],
            |r| r.get(0),
        )?;
        if !exists {
            return Ok(None);
        }
        let sql = if archived {
            "INSERT OR IGNORE INTO archived (peer) VALUES (?1)"
        } else {
            "DELETE FROM archived WHERE peer = ?1"
        };
        self.db.execute(sql, [&peer])?;
        Ok(Some(peer))
    }

    /// Up to `limit` messages whose text has, for each word of `text`, a word
    /// starting with it; newest first, blocked senders' left out.
    pub fn search(&self, text: &str, limit: u32) -> rusqlite::Result<Vec<Found>> {
        // Each word becomes a quoted prefix, so FTS5 syntax in it stays text.
        let query: Vec<String> = text
            .split_whitespace()
            .map(|w| format!("\"{}\"*", w.replace('"', "\"\"")))
            .collect();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        self.db
            .prepare(
                "SELECT m.peer, m.id, m.outgoing, m.text, m.time, m.state
                 FROM messages_text JOIN messages m ON m.id = messages_text.rowid
                 WHERE messages_text MATCH ?1 AND m.blocked = 0
                 ORDER BY m.time DESC, m.id DESC LIMIT ?2",
            )?
            .query_map(params![query.join(" "), limit], |r| {
                Ok((
                    r.get(0)?,
                    r.get::<_, i64>(1)? as u64,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            })?
            .collect()
    }

    /// Blocks a number or sender name, unless an entry matching it is there.
    pub fn block(&self, number: &str) -> rusqlite::Result<()> {
        if !is_blocked(&self.db, number)? {
            self.db
                .execute("INSERT INTO blocked_numbers (number) VALUES (?1)", [number])?;
        }
        Ok(())
    }

    /// Removes every entry that matches `number`. What senders no longer
    /// blocked sent goes back to their conversations, read.
    pub fn unblock(&mut self, number: &str) -> rusqlite::Result<()> {
        let tx = self.db.transaction()?;
        for blocked in blocked_numbers(&tx)? {
            if same_peer(&blocked, number) {
                tx.execute("DELETE FROM blocked_numbers WHERE number = ?1", [blocked])?;
            }
        }
        let senders: Vec<String> = tx
            .prepare("SELECT DISTINCT peer FROM messages WHERE blocked = 1")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for sender in senders {
            if !is_blocked(&tx, &sender)? {
                let peer = conversation_peer(&tx, &sender)?;
                tx.execute(
                    "UPDATE messages SET blocked = 0, peer = ?2 WHERE blocked = 1 AND peer = ?1",
                    params![sender, peer],
                )?;
            }
        }
        tx.commit()
    }

    /// The block list, as the numbers were given.
    pub fn blocked(&self) -> rusqlite::Result<Vec<String>> {
        blocked_numbers(&self.db)
    }

    /// Up to `limit` messages from blocked senders older than `before`,
    /// newest first.
    pub fn blocked_messages(&self, before: i64, limit: u32) -> rusqlite::Result<Vec<Blocked>> {
        self.db
            .prepare(
                "SELECT id, peer, text, time FROM messages
                 WHERE blocked = 1 AND time < ?1 ORDER BY time DESC, id DESC LIMIT ?2",
            )?
            .query_map(params![before, limit], |r| {
                Ok((r.get::<_, i64>(0)? as u64, r.get(1)?, r.get(2)?, r.get(3)?))
            })?
            .collect()
    }
}

/// The digits of a phone number, without its plus; None for a sender name.
fn phone_digits(peer: &str) -> Option<&str> {
    let digits = peer.strip_prefix('+').unwrap_or(peer);
    (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())).then_some(digits)
}

fn same_number(a: &str, b: &str) -> bool {
    if a.len().min(b.len()) < MIN_DIGITS {
        return a == b;
    }
    let n = a.len().min(b.len()).min(MATCH_DIGITS);
    a[a.len() - n..] == b[b.len() - n..]
}

/// Phone numbers match by `same_number`, sender names whole.
fn same_peer(a: &str, b: &str) -> bool {
    match (phone_digits(a), phone_digits(b)) {
        (Some(a), Some(b)) => same_number(a, b),
        _ => a == b,
    }
}

/// The conversation `peer` belongs to: the peer itself if it has one, else
/// one with the same number, else a new one named `peer`.
fn conversation_peer(db: &Connection, peer: &str) -> rusqlite::Result<String> {
    let peers: Vec<String> = db
        .prepare("SELECT DISTINCT peer FROM messages WHERE blocked = 0")?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    if peers.iter().any(|p| p == peer) {
        return Ok(peer.to_owned());
    }
    Ok(peers
        .into_iter()
        .find(|p| same_peer(p, peer))
        .unwrap_or_else(|| peer.to_owned()))
}

fn blocked_numbers(db: &Connection) -> rusqlite::Result<Vec<String>> {
    db.prepare("SELECT number FROM blocked_numbers ORDER BY number")?
        .query_map([], |r| r.get(0))?
        .collect()
}

fn is_blocked(db: &Connection, sender: &str) -> rusqlite::Result<bool> {
    Ok(blocked_numbers(db)?.iter().any(|b| same_peer(b, sender)))
}

/// Inserts an incoming message: unread into its conversation, which leaves
/// the archive, or, from a blocked sender, read into the blocked folder.
/// Returns its id, and its conversation peer unless the sender is blocked.
fn insert_incoming(
    tx: &rusqlite::Transaction,
    sender: &str,
    text: &str,
    time: i64,
) -> rusqlite::Result<(u64, Option<String>)> {
    let blocked = is_blocked(tx, sender)?;
    let peer = if blocked {
        sender.to_owned()
    } else {
        conversation_peer(tx, sender)?
    };
    tx.execute(
        "INSERT INTO messages (peer, outgoing, text, time, state, read, blocked)
         VALUES (?1, 0, ?2, ?3, 'received', ?4, ?4)",
        params![peer, text, time, blocked],
    )?;
    let id = tx.last_insert_rowid() as u64;
    if blocked {
        return Ok((id, None));
    }
    tx.execute("DELETE FROM archived WHERE peer = ?1", [&peer])?;
    Ok((id, Some(peer)))
}

/// Joins the waiting parts of one multipart message, in order. Returns the
/// message's id and conversation peer, unless the sender is blocked.
fn join_parts(
    tx: &rusqlite::Transaction,
    sender: &str,
    reference: u16,
    total: u8,
) -> rusqlite::Result<Option<(u64, String)>> {
    let parts: Vec<(String, i64)> = tx
        .prepare(
            "SELECT text, time FROM received
             WHERE peer = ?1 AND reference = ?2 AND total = ?3 AND message IS NULL
             GROUP BY sequence ORDER BY sequence",
        )?
        .query_map(params![sender, reference, total], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let text: String = parts.iter().map(|(t, _)| t.as_str()).collect();
    let time = parts.iter().map(|&(_, t)| t).min().unwrap_or_default();
    let (id, peer) = insert_incoming(tx, sender, &text, time)?;
    tx.execute(
        "UPDATE received SET message = ?1
         WHERE peer = ?2 AND reference = ?3 AND total = ?4 AND message IS NULL",
        params![id as i64, sender, reference, total],
    )?;
    Ok(peer.map(|peer| (id, peer)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sms::pdu::Part;

    fn deliver(text: &str, part: Option<(u8, u8)>) -> Deliver {
        Deliver {
            sender: "+3900".into(),
            time: Some(100),
            text: text.into(),
            part: part.map(|(sequence, total)| Part {
                reference: 7,
                total,
                sequence,
            }),
        }
    }

    #[test]
    fn stores_once_and_joins_parts() {
        let mut s = Store::in_memory().unwrap();
        let (id, peer) = s.receive(b"a", &deliver("hi", None), 200).unwrap().unwrap();
        assert_eq!(peer, "+3900");
        assert_eq!(s.receive(b"a", &deliver("hi", None), 201).unwrap(), None);

        assert_eq!(
            s.receive(b"p2", &deliver(" world", Some((2, 2))), 300)
                .unwrap(),
            None
        );
        let (joined, _) = s
            .receive(b"p1", &deliver("hello", Some((1, 2))), 301)
            .unwrap()
            .unwrap();
        let all = s.messages("+3900", i64::MAX, 10).unwrap();
        assert_eq!(all.len(), 2);
        assert!(all.iter().any(|m| m.0 == joined && m.2 == "hello world"));
        assert!(all.iter().any(|m| m.0 == id && m.2 == "hi"));
        assert_eq!(s.conversations(false).unwrap()[0].4, 2);
        s.mark_read("+3900").unwrap();
        assert_eq!(s.conversations(false).unwrap()[0].4, 0);
    }

    #[test]
    fn shows_incomplete_parts_after_a_day() {
        let mut s = Store::in_memory().unwrap();
        assert_eq!(
            s.receive(b"p1", &deliver("half", Some((1, 2))), 0).unwrap(),
            None
        );
        assert!(s.flush_stale_parts(60).unwrap().is_empty());
        assert_eq!(s.flush_stale_parts(PART_WAIT + 1).unwrap().len(), 1);
        assert_eq!(s.messages("+3900", i64::MAX, 10).unwrap()[0].2, "half");
    }

    #[test]
    fn one_conversation_per_number() {
        let mut s = Store::in_memory().unwrap();
        s.add_outgoing("393331234567", "to self", 1).unwrap();
        let mut m = deliver("back", None);
        m.sender = "+393331234567".into();
        let (_, peer) = s.receive(b"x", &m, 2).unwrap().unwrap();
        assert_eq!(peer, "393331234567");
        s.add_outgoing("3331234567", "typed", 3).unwrap();
        s.add_outgoing("Iliad", "name", 4).unwrap();
        s.add_outgoing("+447700900123", "abroad", 5).unwrap();
        assert_eq!(s.conversations(false).unwrap().len(), 3);
        assert_eq!(s.messages("+393331234567", i64::MAX, 9).unwrap().len(), 3);
        assert!(!same_number("12345", "912345"));
    }

    /// The schema and rows of a store from before versions were counted.
    const UNVERSIONED: &str = "
        CREATE TABLE messages (
            id INTEGER PRIMARY KEY, peer TEXT NOT NULL, outgoing INTEGER NOT NULL,
            text TEXT NOT NULL, time INTEGER NOT NULL, state TEXT NOT NULL,
            read INTEGER NOT NULL
        );
        CREATE INDEX messages_by_peer ON messages (peer, time);
        CREATE TABLE received (
            pdu BLOB PRIMARY KEY, arrived INTEGER NOT NULL, peer TEXT NOT NULL,
            reference INTEGER, total INTEGER, sequence INTEGER, text TEXT NOT NULL,
            time INTEGER NOT NULL, message INTEGER
        );
        INSERT INTO messages VALUES (1, '393331234567', 1, 'ciao a te', 10, 'sent', 1);
        INSERT INTO messages VALUES (2, '393331234567', 0, 'ciao di nuovo', 10, 'received', 0);
        INSERT INTO received VALUES (x'00', 10, '+393331234567', NULL, NULL, NULL,
                                     'ciao di nuovo', 10, 2);";

    fn version(s: &Store) -> usize {
        s.db.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap() as usize
    }

    #[test]
    fn migrates_an_unversioned_store() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(UNVERSIONED).unwrap();
        let mut s = Store::init(db).unwrap();
        assert_eq!(version(&s), MIGRATIONS.len());
        let conversations = s.conversations(false).unwrap();
        assert_eq!(conversations.len(), 1);
        assert_eq!(conversations[0].4, 1);
        assert_eq!(s.messages("+393331234567", i64::MAX, 10).unwrap().len(), 2);
        // Old messages are searchable, and the store still knows the old PDU.
        assert_eq!(s.search("nuov", 10).unwrap()[0].1, 2);
        let m = Deliver {
            sender: "+393331234567".into(),
            time: Some(10),
            text: "ciao di nuovo".into(),
            part: None,
        };
        assert_eq!(s.receive(&[0], &m, 20).unwrap(), None);
        // Opening it again takes no step twice.
        let s = Store::init(s.db).unwrap();
        assert_eq!(version(&s), MIGRATIONS.len());
    }

    #[test]
    fn a_new_store_takes_every_step() {
        let s = Store::in_memory().unwrap();
        assert_eq!(version(&s), MIGRATIONS.len());
        assert!(s.conversations(false).unwrap().is_empty());
    }

    #[test]
    fn blocked_senders_go_to_their_own_folder() {
        let mut s = Store::in_memory().unwrap();
        s.add_outgoing("3331234567", "before", 1).unwrap();
        s.block("3331234567").unwrap();
        s.block("+393331234567").unwrap();
        assert_eq!(s.blocked().unwrap(), ["3331234567"]);
        let mut m = deliver("spam", None);
        m.sender = "+393331234567".into();
        assert_eq!(s.receive(b"b1", &m, 2).unwrap(), None);
        // Left out of the conversation, its unread count and search.
        let conversations = s.conversations(false).unwrap();
        assert_eq!(
            (conversations[0].1.as_str(), conversations[0].4),
            ("before", 0)
        );
        assert_eq!(s.messages("3331234567", i64::MAX, 9).unwrap().len(), 1);
        assert!(s.search("spam", 9).unwrap().is_empty());
        let blocked = s.blocked_messages(i64::MAX, 9).unwrap();
        assert_eq!(
            (blocked[0].1.as_str(), blocked[0].2.as_str()),
            ("+393331234567", "spam")
        );
        // Unblocking matches like conversations do, and brings the blocked
        // messages back into the conversation, read.
        s.unblock("+39 333").unwrap();
        assert_eq!(s.blocked().unwrap().len(), 1);
        s.unblock("00393331234567").unwrap();
        assert!(s.blocked().unwrap().is_empty());
        assert!(s.blocked_messages(i64::MAX, 9).unwrap().is_empty());
        let conversations = s.conversations(false).unwrap();
        assert_eq!(
            (conversations[0].1.as_str(), conversations[0].4),
            ("spam", 0)
        );
        assert_eq!(s.messages("3331234567", i64::MAX, 9).unwrap().len(), 2);
        m.text = "hello".into();
        let (_, peer) = s.receive(b"b2", &m, 3).unwrap().unwrap();
        assert_eq!(peer, "3331234567");
    }

    #[test]
    fn new_messages_leave_the_archive() {
        let mut s = Store::in_memory().unwrap();
        s.receive(b"a1", &deliver("one", None), 1).unwrap();
        assert_eq!(
            s.set_archived("+3900", true).unwrap().as_deref(),
            Some("+3900")
        );
        assert_eq!(s.set_archived("+3911", true).unwrap(), None);
        assert!(s.conversations(false).unwrap().is_empty());
        assert_eq!(s.conversations(true).unwrap()[0].0, "+3900");
        s.receive(b"a2", &deliver("two", None), 2).unwrap();
        assert_eq!(s.conversations(false).unwrap().len(), 1);
        assert!(s.conversations(true).unwrap().is_empty());
        s.set_archived("+3900", true).unwrap();
        s.add_outgoing("+3900", "reply", 3).unwrap();
        assert!(s.conversations(true).unwrap().is_empty());
    }

    #[test]
    fn deletes_and_searches_conversations() {
        let mut s = Store::in_memory().unwrap();
        s.add_outgoing("393331234567", "Città di Roma", 1).unwrap();
        s.add_outgoing("393331234567", "il codice è 1234", 2)
            .unwrap();
        s.add_outgoing("+447700900123", "citta \"quoted\" OR", 3)
            .unwrap();
        let found = s.search("citta", 9).unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].0, "+447700900123");
        assert_eq!(s.search("CITT rom", 9).unwrap()[0].3, "Città di Roma");
        assert_eq!(s.search("\"quoted\" OR", 9).unwrap().len(), 1);
        assert!(s.search("  ", 9).unwrap().is_empty());
        assert!(s.search("!!", 9).unwrap().is_empty());
        assert_eq!(s.search("citta", 1).unwrap().len(), 1);

        assert_eq!(s.mark_read("3331234567").unwrap(), "393331234567");
        assert_eq!(
            s.delete_conversation("+393331234567").unwrap().as_deref(),
            Some("393331234567")
        );
        assert_eq!(s.delete_conversation("393331234567").unwrap(), None);
        assert_eq!(s.search("citta", 9).unwrap().len(), 1);
        assert_eq!(s.conversations(false).unwrap().len(), 1);
    }

    #[test]
    fn delivery_reports_settle_the_state() {
        let mut s = Store::in_memory().unwrap();
        let two = s.add_outgoing("393331234567", "long", 1).unwrap();
        s.await_report(two, 5).unwrap();
        s.await_report(two, 6).unwrap();
        s.set_state(two, "sent").unwrap();
        assert_eq!(s.report("+393331234567", 5, "delivered").unwrap(), None);
        // The same reference to someone else, or a report seen before.
        assert_eq!(s.report("+447700900123", 6, "delivered").unwrap(), None);
        assert_eq!(s.report("+393331234567", 5, "delivered").unwrap(), None);
        assert_eq!(
            s.report("+393331234567", 6, "delivered").unwrap(),
            Some((two, "delivered".into()))
        );
        assert_eq!(
            s.messages("393331234567", i64::MAX, 1).unwrap()[0].4,
            "delivered"
        );

        // References wrap: the newest waiting part takes the report.
        let old = s.add_outgoing("393331234567", "old", 2).unwrap();
        s.await_report(old, 7).unwrap();
        s.set_state(old, "sent").unwrap();
        let new = s.add_outgoing("393331234567", "new", 3).unwrap();
        s.await_report(new, 7).unwrap();
        s.set_state(new, "sent").unwrap();
        assert_eq!(
            s.report("393331234567", 7, "expired").unwrap(),
            Some((new, "expired".into()))
        );

        // A message that failed keeps its state.
        let failed = s.add_outgoing("393331234567", "half", 4).unwrap();
        s.await_report(failed, 8).unwrap();
        s.set_state(failed, "failed").unwrap();
        assert_eq!(s.report("393331234567", 8, "delivered").unwrap(), None);
        assert!(s.delete(failed).unwrap());
        let left: i64 =
            s.db.query_row("SELECT COUNT(*) FROM reports", [], |r| r.get(0))
                .unwrap();
        assert_eq!(left, 4);
    }

    #[test]
    fn outgoing_states() {
        let mut s = Store::in_memory().unwrap();
        let id = s.add_outgoing("+3901", "yo", 5).unwrap();
        assert_eq!(s.fail_unsent().unwrap(), [id]);
        assert_eq!(s.messages("+3901", i64::MAX, 1).unwrap()[0].4, "failed");
        assert!(s.delete(id).unwrap());
    }
}
