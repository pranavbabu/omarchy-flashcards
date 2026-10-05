//! One function per subcommand. Each returns the JSON document that is printed on stdout.

use crate::anki::{self, FieldSpec};
use crate::audio;
use crate::db::{self, Card, NewCard};
use crate::scheduler::{self, AGAIN, EASY, GOOD, HARD, NEW, REVIEW};
use crate::{CliError, Result};
use regex::Regex;
use rusqlite::types::Value;
use rusqlite::{Connection, params};
use serde_json::{Value as Json, json};
use std::collections::HashSet;
use std::path::Path;

fn deck_arg(deck: &Option<String>) -> Option<&str> {
    deck.as_deref().filter(|d| !d.is_empty())
}

pub fn next(
    db: &Connection,
    now: i64,
    deck: &Option<String>,
    new_per_day: i64,
    count: i64,
) -> Result<Json> {
    let deck = deck_arg(deck);
    let rows = db::pick_queue(db, now, deck, new_per_day, count.max(1))?;
    let counts = db::counts(db, now, deck, new_per_day)?;
    let mut cards = Vec::new();
    for row in &rows {
        let mut card = row.to_json();
        let preview: serde_json::Map<String, Json> = [AGAIN, HARD, GOOD, EASY]
            .iter()
            .map(|&r| {
                (
                    r.to_string(),
                    Json::from(scheduler::fmt_interval(
                        scheduler::schedule(row, r, now).due - now,
                    )),
                )
            })
            .collect();
        card["lang"] = db::lang_of(db, &row.deck)?.into();
        card["preview"] = Json::Object(preview);
        cards.push(card);
    }
    let (clause, mut args) = match deck {
        Some(d) => ("AND deck = ?", vec![Value::from(d.to_string())]),
        None => ("", vec![]),
    };
    args.insert(0, NEW.into());
    let new_waiting = db::scalar(
        db,
        &format!("SELECT COUNT(*) FROM cards WHERE suspended = 0 AND state = ? {clause}"),
        args,
    )?;
    Ok(json!({
        "card": cards.first().cloned().unwrap_or(Json::Null),
        "cards": cards,
        "counts": {"new": counts.new, "learning": counts.learning, "review": counts.review},
        "total": db::scalar(db, "SELECT COUNT(*) FROM cards", vec![])?,
        "next_due_in": db::next_due_in(db, now, deck)?,
        "new_waiting": new_waiting,
        "due": counts.new + counts.learning + counts.review,
    }))
}

pub fn answer(db: &Connection, now: i64, id: i64, rating: i64) -> Result<Json> {
    let card = db::card_by_id(db, id)?;
    let new = scheduler::schedule(&card, rating, now);
    db.execute(
        "INSERT INTO revlog (card_id, ts, rating, state_before, snapshot) VALUES (?,?,?,?,?)",
        params![card.id, now, rating, card.state, card.to_json().to_string()],
    )?;
    save_schedule(db, &new)?;
    Ok(json!({"ok": true, "id": card.id, "due_in": new.due - now, "interval": new.interval}))
}

fn save_schedule(db: &Connection, c: &Card) -> Result<()> {
    db.execute(
        "UPDATE cards SET state=?, step=?, due=?, interval=?, ease=?, reps=?, lapses=? WHERE id=?",
        params![
            c.state, c.step, c.due, c.interval, c.ease, c.reps, c.lapses, c.id
        ],
    )?;
    Ok(())
}

pub fn undo(db: &Connection) -> Result<Json> {
    let row: Option<(i64, String)> = db
        .query_row(
            "SELECT id, snapshot FROM revlog ORDER BY id DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    let (log_id, snapshot) = row.ok_or_else(|| CliError("nothing to undo".into()))?;
    let s: Json = serde_json::from_str(&snapshot).map_err(|e| CliError(e.to_string()))?;
    let get = |key: &str| s[key].as_i64().unwrap_or(0);
    save_schedule(
        db,
        &Card {
            id: get("id"),
            state: get("state"),
            step: get("step"),
            due: get("due"),
            interval: get("interval"),
            ease: get("ease"),
            reps: get("reps"),
            lapses: get("lapses"),
            ..Card::default()
        },
    )?;
    db.execute("DELETE FROM revlog WHERE id = ?", [log_id])?;
    Ok(json!({"ok": true, "id": get("id")}))
}

#[allow(clippy::too_many_arguments)]
pub fn add(
    db: &Connection,
    now: i64,
    front: &str,
    back: &str,
    note: &str,
    audio_file: &str,
    image: &str,
    deck: &str,
    lang: &str,
) -> Result<Json> {
    if front.trim().is_empty() || back.trim().is_empty() {
        return Err(CliError("front and back must not be empty".into()));
    }
    db::ensure_deck(db, deck, lang)?;
    let image = audio::expand_user(image).to_string_lossy().into_owned();
    if !image.is_empty() && !Path::new(&image).is_file() {
        return Err(CliError(format!("image not found: {image}")));
    }
    let mode = if image.is_empty() { "" } else { "front" };
    let card = NewCard {
        deck,
        front,
        back,
        note,
        audio: audio_file,
        rank: None,
        image: &image,
        image_mode: mode,
    };
    match db::insert_card(db, &card, now)? {
        Some(id) => Ok(json!({"ok": true, "id": id})),
        None => Err(CliError(format!(
            "'{}' is already in deck {deck}",
            front.trim()
        ))),
    }
}

fn sanitize(name: &str) -> String {
    static UNSAFE: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| Regex::new(r"[^\w.\-]").unwrap());
    UNSAFE.replace_all(name, "_").into_owned()
}

fn import_apkg(
    db: &Connection,
    now: i64,
    file: &str,
    deck: &str,
    lang: &str,
    spec: &FieldSpec,
) -> Result<Json> {
    db::ensure_deck(db, deck, lang)?;
    let (mut added, mut skipped) = (0, 0);
    anki::read_apkg(file, spec, |n| {
        let mut image = String::new();
        if let Some(bytes) = &n.image_bytes {
            let name = format!(
                "{}-{}",
                &audio::sha1_hex(bytes)[..12],
                sanitize(n.image.as_deref().unwrap_or("image"))
            );
            std::fs::create_dir_all(db::image_dir())?;
            let path = db::image_dir().join(name);
            if !path.is_file() {
                std::fs::write(&path, bytes)?;
            }
            image = path.to_string_lossy().into_owned();
        }
        let mut audio_file = String::new();
        if let (Some(bytes), Some(sound)) = (&n.media, &n.sound) {
            let dir = db::audio_dir().join("anki");
            std::fs::create_dir_all(&dir)?;
            let path = dir.join(sanitize(sound));
            std::fs::write(&path, bytes)?;
            audio_file = path.to_string_lossy().into_owned();
        }
        let usable = !n.front.is_empty() && (!n.back.is_empty() || n.image_mode == "prompt");
        let card = NewCard {
            deck,
            front: &n.front,
            back: &n.back,
            note: &n.note,
            audio: &audio_file,
            rank: n.rank,
            image: &image,
            image_mode: if image.is_empty() { "" } else { &n.image_mode },
        };
        if usable && db::insert_card(db, &card, now)?.is_some() {
            added += 1;
        } else {
            skipped += 1;
        }
        Ok(())
    })?;
    Ok(json!({"ok": true, "added": added, "skipped": skipped}))
}

pub fn import(
    db: &Connection,
    now: i64,
    file: &str,
    deck: &str,
    lang: &str,
    spec: &FieldSpec,
) -> Result<Json> {
    if !Path::new(file).is_file() {
        return Err(CliError(format!("file not found: {file}")));
    }
    if file.to_lowercase().ends_with(".apkg") {
        return import_apkg(db, now, file, deck, lang, spec);
    }
    let bytes = std::fs::read(file)?;
    let text = String::from_utf8(bytes).map_err(|e| CliError(e.to_string()))?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let lines: Vec<&str> = text
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .collect();
    if lines.is_empty() {
        return Ok(json!({"ok": true, "added": 0, "skipped": 0}));
    }
    let first = lines[0];
    let delimiter = if first.contains('\t') {
        b'\t'
    } else if first.matches(';').count() > first.matches(',').count() {
        b';'
    } else {
        b','
    };
    db::ensure_deck(db, deck, lang)?;
    let joined = lines.join("\n");
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .delimiter(delimiter)
        .from_reader(joined.as_bytes());
    let (mut added, mut skipped) = (0, 0);
    for record in reader.records() {
        let record = record.map_err(|e| CliError(e.to_string()))?;
        let col = |i: usize| record.get(i).unwrap_or("").trim();
        if col(0).is_empty() || col(1).is_empty() {
            skipped += 1;
            continue;
        }
        let card = NewCard {
            deck,
            front: col(0),
            back: col(1),
            note: col(2),
            audio: col(3),
            rank: None,
            image: "",
            image_mode: "",
        };
        if db::insert_card(db, &card, now)?.is_some() {
            added += 1;
        } else {
            skipped += 1;
        }
    }
    Ok(json!({"ok": true, "added": added, "skipped": skipped}))
}

fn tokens(text: &str) -> Vec<String> {
    static WORD: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| Regex::new(r"[\p{L}\p{M}]+").unwrap());
    let lower = text.to_lowercase();
    WORD.find_iter(&lower)
        .map(|m| m.as_str().to_string())
        .collect()
}

/// Suspend new cards made only of the `max_rank` most frequent words (a proxy for CEFR A1).
pub fn skip_easy(db: &Connection, max_rank: i64, dry_run: bool) -> Result<Json> {
    let mut stmt = db.prepare("SELECT front FROM cards WHERE rank IS NOT NULL AND rank <= ?")?;
    let ranked: Vec<String> = stmt
        .query_map([max_rank], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    if ranked.is_empty() {
        return Err(CliError(
            "no ranked cards: import a frequency deck first".into(),
        ));
    }
    let easy: HashSet<String> = ranked.iter().flat_map(|f| tokens(f)).collect();
    let mut stmt =
        db.prepare("SELECT id, front, rank FROM cards WHERE state = ? AND suspended = 0")?;
    let rows: Vec<(i64, String, Option<i64>)> = stmt
        .query_map([NEW], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let victims: Vec<&(i64, String, Option<i64>)> = rows
        .iter()
        .filter(|(_, front, rank)| match rank {
            Some(r) => *r <= max_rank,
            None => {
                let words = tokens(front);
                !words.is_empty() && words.iter().all(|w| easy.contains(w))
            }
        })
        .collect();
    if !dry_run {
        for (id, _, _) in &victims {
            db.execute("UPDATE cards SET suspended = 1 WHERE id = ?", [id])?;
        }
    }
    let step = (victims.len() / 25).max(1);
    let sample: Vec<&str> = victims
        .iter()
        .step_by(step)
        .take(25)
        .map(|v| v.1.as_str())
        .collect();
    Ok(json!({"ok": true, "dry_run": dry_run, "suspended": victims.len(), "sample": sample}))
}

pub fn unsuspend(db: &Connection) -> Result<Json> {
    let restored = db.execute("UPDATE cards SET suspended = 0 WHERE suspended = 1", [])?;
    Ok(json!({"ok": true, "restored": restored}))
}

pub fn decks(db: &Connection, now: i64, new_per_day: i64) -> Result<Json> {
    let mut stmt = db.prepare("SELECT name, lang FROM decks ORDER BY name")?;
    let rows: Vec<(String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut out = Vec::new();
    for (name, lang) in rows {
        let c = db::counts(db, now, Some(&name), new_per_day)?;
        let (total, suspended): (i64, i64) = db.query_row(
            "SELECT COUNT(*), COALESCE(SUM(suspended), 0) FROM cards WHERE deck = ?",
            [&name],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        out.push(json!({"name": name, "lang": lang, "total": total, "suspended": suspended, "new": c.new, "learning": c.learning, "review": c.review}));
    }
    Ok(json!({"decks": out, "active": db::get_meta(db, "deck")?}))
}

fn deck_exists(db: &Connection, name: &str) -> Result<bool> {
    Ok(db::scalar(
        db,
        "SELECT COUNT(*) FROM decks WHERE name = ?",
        vec![name.to_string().into()],
    )? > 0)
}

pub fn use_deck(db: &Connection, name: &str) -> Result<Json> {
    if name != "*" && !deck_exists(db, name)? {
        return Err(CliError(format!("no deck named {name}")));
    }
    db.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES ('deck', ?)",
        [name],
    )?;
    Ok(json!({"ok": true}))
}

pub fn delete_deck(db: &Connection, name: &str) -> Result<Json> {
    let mut stmt = db.prepare("SELECT id FROM cards WHERE deck = ?")?;
    let ids: Vec<i64> = stmt
        .query_map([name], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    if ids.is_empty() && !deck_exists(db, name)? {
        return Err(CliError(format!("no deck named {name}")));
    }
    for id in &ids {
        db.execute("DELETE FROM revlog WHERE card_id = ?", [id])?;
    }
    db.execute("DELETE FROM cards WHERE deck = ?", [name])?;
    db.execute("DELETE FROM decks WHERE name = ?", [name])?;
    if db::get_meta(db, "deck")?.as_deref() == Some(name) {
        db.execute("DELETE FROM meta WHERE key = 'deck'", [])?;
    }
    Ok(json!({"ok": true, "deleted": ids.len()}))
}

pub fn rename_deck(db: &Connection, old: &str, new: &str) -> Result<Json> {
    if !deck_exists(db, old)? {
        return Err(CliError(format!("no deck named {old}")));
    }
    if deck_exists(db, new)? {
        return Err(CliError(format!("a deck named {new} already exists")));
    }
    db.execute("UPDATE decks SET name = ? WHERE name = ?", [new, old])?;
    db.execute("UPDATE cards SET deck = ? WHERE deck = ?", [new, old])?;
    db.execute(
        "UPDATE meta SET value = ? WHERE key = 'deck' AND value = ?",
        [new, old],
    )?;
    Ok(json!({"ok": true}))
}

pub fn find_files(dir: &str) -> Result<Json> {
    let dir = audio::expand_user(dir);
    let mut found: Vec<(std::time::SystemTime, std::path::PathBuf, u64)> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let importable = [".apkg", ".tsv", ".csv", ".txt"]
                .iter()
                .any(|ext| name.ends_with(ext))
                && !name.starts_with('.');
            if let (true, Ok(meta)) = (importable, entry.metadata())
                && meta.is_file()
            {
                found.push((
                    meta.modified().unwrap_or(std::time::UNIX_EPOCH),
                    path,
                    meta.len(),
                ));
            }
        }
    }
    found.sort_by_key(|f| std::cmp::Reverse(f.0));
    let files: Vec<Json> = found
        .iter()
        .take(8)
        .map(|(_, path, size)| {
            let mb = (*size as f64 / 1e6 * 10.0).round_ties_even() / 10.0;
            json!({"name": path.file_name().map(|n| n.to_string_lossy().into_owned()), "path": path, "mb": mb})
        })
        .collect();
    Ok(json!({"files": files}))
}

pub fn list(db: &Connection, query: &str, limit: i64) -> Result<Json> {
    let pattern = format!("%{query}%");
    let mut stmt = db.prepare(
        "SELECT * FROM cards WHERE (front LIKE ?1 OR back LIKE ?1) ORDER BY id DESC LIMIT ?2",
    )?;
    let cards: Vec<Json> = stmt
        .query_map(params![pattern, limit], Card::from_row)?
        .map(|c| c.map(|c| c.to_json()))
        .collect::<rusqlite::Result<_>>()?;
    Ok(json!({"cards": cards}))
}

pub fn delete(db: &Connection, id: i64) -> Result<Json> {
    if db.execute("DELETE FROM cards WHERE id = ?", [id])? == 0 {
        return Err(CliError(format!("no card with id {id}")));
    }
    Ok(json!({"ok": true}))
}

pub fn stats(db: &Connection, now: i64) -> Result<Json> {
    let by_state = |state: i64| {
        db::scalar(
            db,
            "SELECT COUNT(*) FROM cards WHERE suspended = 0 AND state = ?",
            vec![state.into()],
        )
    };
    let (new, learning, relearning, review) = (
        by_state(NEW)?,
        by_state(1)?,
        by_state(3)?,
        by_state(REVIEW)?,
    );
    let suspended = db::scalar(db, "SELECT COUNT(*) FROM cards WHERE suspended = 1", vec![])?;
    let (reviewed, passed): (i64, i64) = db.query_row(
        "SELECT COUNT(*), COALESCE(SUM(rating > 1), 0) FROM revlog WHERE ts >= ?",
        [db::day_start(now)],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let mature = db::scalar(
        db,
        "SELECT COUNT(*) FROM cards WHERE state = ? AND interval >= 21",
        vec![REVIEW.into()],
    )?;
    Ok(json!({
        "total": new + learning + relearning + review + suspended, "suspended": suspended, "new": new,
        "learning": learning + relearning, "review": review, "mature": mature,
        "reviewed_today": reviewed, "passed_today": passed,
    }))
}

pub fn audio_cmd(db: &Connection, id: i64, play: bool) -> Result<Json> {
    let card = db::card_by_id(db, id)?;
    let lang = db::lang_of(db, &card.deck)?;
    match audio::ensure_audio(&card, &lang)? {
        None => {
            if play {
                audio::speak_live(&lang, &card.front)?;
            } else if audio::which("espeak-ng").is_none() {
                return Err(CliError(
                    "no audio: offline and espeak-ng is not installed".into(),
                ));
            }
            Ok(json!({"ok": true, "path": Json::Null, "engine": "espeak-ng"}))
        }
        Some(path) => {
            if play && !audio::play(&path) {
                return Err(CliError("no audio player found (install mpv)".into()));
            }
            Ok(
                json!({"ok": true, "path": path, "engine": if card.audio.is_empty() { "tts" } else { "file" }}),
            )
        }
    }
}

pub fn prefetch(db: &Connection) -> Result<Json> {
    let mut stmt = db.prepare("SELECT * FROM cards WHERE audio = ''")?;
    let cards: Vec<Card> = stmt
        .query_map([], Card::from_row)?
        .collect::<rusqlite::Result<_>>()?;
    let mut fetched = 0;
    for card in cards {
        let lang = db::lang_of(db, &card.deck)?;
        if !audio::audio_path(&card, &lang).is_file()
            && audio::ensure_audio(&card, &lang)?.is_some()
        {
            fetched += 1;
        }
    }
    Ok(json!({"ok": true, "fetched": fetched}))
}
