//! SQLite store. The schema and migrations are identical to the Python version,
//! so an existing `cards.db` opens unchanged.

use crate::scheduler::{self, LEARNING, NEW, RELEARNING, REVIEW};
use crate::{CliError, Result};
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, Row, params, params_from_iter};
use serde_json::{Value as Json, json};
use std::path::PathBuf;

pub const DEFAULT_NEW_PER_DAY: i64 = 20;
const LEARN_AHEAD: i64 = 20 * 60;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS decks (name TEXT PRIMARY KEY, lang TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS cards (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  deck TEXT NOT NULL,
  front TEXT NOT NULL,
  back TEXT NOT NULL,
  note TEXT NOT NULL DEFAULT '',
  audio TEXT NOT NULL DEFAULT '',
  state INTEGER NOT NULL DEFAULT 0,
  step INTEGER NOT NULL DEFAULT 0,
  due INTEGER NOT NULL DEFAULT 0,
  interval INTEGER NOT NULL DEFAULT 0,
  ease INTEGER NOT NULL DEFAULT 2500,
  reps INTEGER NOT NULL DEFAULT 0,
  lapses INTEGER NOT NULL DEFAULT 0,
  suspended INTEGER NOT NULL DEFAULT 0,
  created INTEGER NOT NULL,
  rank INTEGER,
  image TEXT NOT NULL DEFAULT '',
  image_mode TEXT NOT NULL DEFAULT '',
  UNIQUE (deck, front)
);
CREATE INDEX IF NOT EXISTS cards_due ON cards (suspended, state, due);
CREATE TABLE IF NOT EXISTS revlog (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  card_id INTEGER NOT NULL,
  ts INTEGER NOT NULL,
  rating INTEGER NOT NULL,
  state_before INTEGER NOT NULL,
  snapshot TEXT NOT NULL
);
";

/// Columns added after the first release; older databases get them on open.
const MIGRATIONS: &[(&str, &str)] = &[
    ("rank", "INTEGER"),
    ("image", "TEXT NOT NULL DEFAULT ''"),
    ("image_mode", "TEXT NOT NULL DEFAULT ''"),
];

pub fn home() -> PathBuf {
    match std::env::var("FLASHCARDS_HOME") {
        Ok(h) if !h.is_empty() => PathBuf::from(h),
        _ => PathBuf::from(std::env::var("HOME").unwrap_or_default())
            .join(".local/share/omarchy-flashcards"),
    }
}

pub fn audio_dir() -> PathBuf {
    home().join("audio")
}

pub fn image_dir() -> PathBuf {
    home().join("images")
}

pub fn connect() -> Result<Connection> {
    std::fs::create_dir_all(home())?;
    let db = Connection::open(home().join("cards.db"))?;
    db.execute_batch(SCHEMA)?;
    let have: Vec<String> = db
        .prepare("PRAGMA table_info(cards)")?
        .query_map([], |r| r.get::<_, String>("name"))?
        .collect::<rusqlite::Result<_>>()?;
    for (column, ddl) in MIGRATIONS {
        if !have.iter().any(|h| h == column) {
            db.execute_batch(&format!("ALTER TABLE cards ADD COLUMN {column} {ddl}"))?;
        }
    }
    Ok(db)
}

#[derive(Clone, Debug, Default)]
pub struct Card {
    pub id: i64,
    pub deck: String,
    pub front: String,
    pub back: String,
    pub note: String,
    pub audio: String,
    pub state: i64,
    pub step: i64,
    pub due: i64,
    pub interval: i64,
    pub ease: i64,
    pub reps: i64,
    pub lapses: i64,
    pub suspended: i64,
    pub created: i64,
    pub rank: Option<i64>,
    pub image: String,
    pub image_mode: String,
}

impl Card {
    pub fn from_row(r: &Row) -> rusqlite::Result<Card> {
        Ok(Card {
            id: r.get("id")?,
            deck: r.get("deck")?,
            front: r.get("front")?,
            back: r.get("back")?,
            note: r.get("note")?,
            audio: r.get("audio")?,
            state: r.get("state")?,
            step: r.get("step")?,
            due: r.get("due")?,
            interval: r.get("interval")?,
            ease: r.get("ease")?,
            reps: r.get("reps")?,
            lapses: r.get("lapses")?,
            suspended: r.get("suspended")?,
            created: r.get("created")?,
            rank: r.get("rank")?,
            image: r.get("image")?,
            image_mode: r.get("image_mode")?,
        })
    }

    pub fn to_json(&self) -> Json {
        json!({
            "id": self.id, "deck": self.deck, "front": self.front, "back": self.back, "note": self.note,
            "audio": self.audio, "state": self.state, "step": self.step, "due": self.due,
            "interval": self.interval, "ease": self.ease, "reps": self.reps, "lapses": self.lapses,
            "suspended": self.suspended, "created": self.created, "rank": self.rank,
            "image": self.image, "image_mode": self.image_mode,
        })
    }
}

pub fn card_by_id(db: &Connection, id: i64) -> Result<Card> {
    db.query_row("SELECT * FROM cards WHERE id = ?", [id], Card::from_row)
        .optional()?
        .ok_or_else(|| CliError(format!("no card with id {id}")))
}

fn cards(db: &Connection, sql: &str, params: Vec<Value>) -> Result<Vec<Card>> {
    let mut stmt = db.prepare(sql)?;
    let rows = stmt.query_map(params_from_iter(params), Card::from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn scalar(db: &Connection, sql: &str, params: Vec<Value>) -> Result<i64> {
    Ok(db.query_row(sql, params_from_iter(params), |r| r.get(0))?)
}

/// Midnight today in the local time zone, as a Unix timestamp.
pub fn day_start(now: i64) -> i64 {
    use chrono::{Local, TimeZone};
    let local = Local
        .timestamp_opt(now, 0)
        .single()
        .unwrap_or_else(|| Local.timestamp_opt(0, 0).unwrap());
    let midnight = local.date_naive().and_hms_opt(0, 0, 0).unwrap();
    Local
        .from_local_datetime(&midnight)
        .earliest()
        .map(|t| t.timestamp())
        .unwrap_or(now - now % 86400)
}

pub fn deck_names(db: &Connection, deck: Option<&str>) -> Result<Vec<String>> {
    match deck {
        Some(d) => Ok(vec![d.to_string()]),
        None => {
            let mut stmt = db.prepare("SELECT name FROM decks ORDER BY name")?;
            let rows = stmt.query_map([], |r| r.get(0))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        }
    }
}

/// New cards still allowed today in one deck; each deck has its own daily limit.
pub fn new_budget(db: &Connection, now: i64, limit: i64, deck: &str) -> Result<i64> {
    let seen = scalar(
        db,
        "SELECT COUNT(*) FROM revlog r JOIN cards c ON c.id = r.card_id \
         WHERE r.state_before = ? AND r.ts >= ? AND c.deck = ?",
        vec![NEW.into(), day_start(now).into(), deck.to_string().into()],
    )?;
    Ok((limit - seen).max(0))
}

pub struct Counts {
    pub new: i64,
    pub learning: i64,
    pub review: i64,
}

fn deck_clause(deck: Option<&str>) -> (&'static str, Vec<Value>) {
    match deck {
        Some(d) => ("AND deck = ?", vec![d.to_string().into()]),
        None => ("", vec![]),
    }
}

pub fn counts(db: &Connection, now: i64, deck: Option<&str>, limit: i64) -> Result<Counts> {
    let (clause, deck_args) = deck_clause(deck);
    let q = |extra: &str, mut args: Vec<Value>| -> Result<i64> {
        let mut all = deck_args.clone();
        all.append(&mut args);
        scalar(
            db,
            &format!("SELECT COUNT(*) FROM cards WHERE suspended = 0 {clause} {extra}"),
            all,
        )
    };
    let mut new = 0;
    for name in deck_names(db, deck)? {
        let total = scalar(
            db,
            "SELECT COUNT(*) FROM cards WHERE suspended = 0 AND state = ? AND deck = ?",
            vec![NEW.into(), name.clone().into()],
        )?;
        new += total.min(new_budget(db, now, limit, &name)?);
    }
    Ok(Counts {
        new,
        learning: q(
            "AND state IN (?, ?) AND due <= ?",
            vec![LEARNING.into(), RELEARNING.into(), now.into()],
        )?,
        review: q(
            "AND state = ? AND due <= ?",
            vec![REVIEW.into(), now.into()],
        )?,
    })
}

/// Up to `count` cards in study order: learning, then reviews, then new, then learn-ahead.
pub fn pick_queue(
    db: &Connection,
    now: i64,
    deck: Option<&str>,
    limit: i64,
    count: i64,
) -> Result<Vec<Card>> {
    let (clause, deck_args) = deck_clause(deck);
    let mut rows: Vec<Card> = Vec::new();
    let take = |rows: &mut Vec<Card>, extra: &str, args: Vec<Value>, cap: i64| -> Result<()> {
        if cap <= 0 {
            return Ok(());
        }
        let mut all = deck_args.clone();
        all.extend(args);
        all.push(cap.into());
        rows.extend(cards(
            db,
            &format!("SELECT * FROM cards WHERE suspended = 0 {clause} {extra} LIMIT ?"),
            all,
        )?);
        Ok(())
    };

    let learning = || vec![LEARNING.into(), RELEARNING.into()];
    let mut args = learning();
    args.push(now.into());
    take(
        &mut rows,
        "AND state IN (?, ?) AND due <= ? ORDER BY due",
        args,
        count,
    )?;
    let remaining = count - rows.len() as i64;
    take(
        &mut rows,
        "AND state = ? AND due <= ? ORDER BY due",
        vec![REVIEW.into(), now.into()],
        remaining,
    )?;
    for name in deck_names(db, deck)? {
        let room = (count - rows.len() as i64).min(new_budget(db, now, limit, &name)?);
        if room > 0 {
            rows.extend(cards(
                db,
                "SELECT * FROM cards WHERE suspended = 0 AND state = ? AND deck = ? ORDER BY id LIMIT ?",
                vec![NEW.into(), name.into(), room.into()],
            )?);
        }
    }
    if rows.is_empty() {
        let mut args = learning();
        args.push((now + LEARN_AHEAD).into());
        take(
            &mut rows,
            "AND state IN (?, ?) AND due <= ? ORDER BY due",
            args,
            count,
        )?;
    }
    Ok(rows)
}

pub fn next_due_in(db: &Connection, now: i64, deck: Option<&str>) -> Result<Option<i64>> {
    let (clause, deck_args) = deck_clause(deck);
    let mut args: Vec<Value> = vec![NEW.into()];
    args.extend(deck_args);
    let due: Option<i64> = db.query_row(
        &format!("SELECT MIN(due) FROM cards WHERE suspended = 0 AND state != ? {clause}"),
        params_from_iter(args),
        |r| r.get(0),
    )?;
    Ok(due.map(|d| (d - now).max(0)))
}

pub fn lang_of(db: &Connection, deck: &str) -> Result<String> {
    Ok(db
        .query_row("SELECT lang FROM decks WHERE name = ?", [deck], |r| {
            r.get(0)
        })
        .optional()?
        .unwrap_or_else(|| "no".to_string()))
}

pub fn get_meta(db: &Connection, key: &str) -> Result<Option<String>> {
    Ok(db
        .query_row("SELECT value FROM meta WHERE key = ?", [key], |r| r.get(0))
        .optional()?)
}

pub fn ensure_deck(db: &Connection, deck: &str, lang: &str) -> Result<()> {
    db.execute(
        "INSERT OR IGNORE INTO decks (name, lang) VALUES (?, ?)",
        params![deck, lang],
    )?;
    if !lang.is_empty() {
        db.execute(
            "UPDATE decks SET lang = ? WHERE name = ?",
            params![lang, deck],
        )?;
    }
    Ok(())
}

pub struct NewCard<'a> {
    pub deck: &'a str,
    pub front: &'a str,
    pub back: &'a str,
    pub note: &'a str,
    pub audio: &'a str,
    pub rank: Option<i64>,
    pub image: &'a str,
    pub image_mode: &'a str,
}

/// Insert a card unless the deck already has that front. On a duplicate, a missing rank is filled in.
/// Returns the new card's id, or `None` for a duplicate.
pub fn insert_card(db: &Connection, c: &NewCard, now: i64) -> Result<Option<i64>> {
    let front = c.front.trim();
    let changed = db.execute(
        "INSERT OR IGNORE INTO cards (deck, front, back, note, audio, ease, created, rank, image, image_mode) \
         VALUES (?,?,?,?,?,?,?,?,?,?)",
        params![c.deck, front, c.back.trim(), c.note.trim(), c.audio.trim(), scheduler::START_EASE, now, c.rank, c.image, c.image_mode],
    )?;
    if changed == 0 {
        if let Some(rank) = c.rank {
            db.execute(
                "UPDATE cards SET rank = ? WHERE deck = ? AND front = ? AND rank IS NULL",
                params![rank, c.deck, front],
            )?;
        }
        return Ok(None);
    }
    Ok(Some(db.last_insert_rowid()))
}
