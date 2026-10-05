//! Spaced-repetition flashcards: Anki-style scheduler, SQLite store, pronunciation audio.
//!
//! Every command prints one JSON document on stdout, so the Quickshell widget and the
//! tests read the same output. Data lives in `$FLASHCARDS_HOME`
//! (default `~/.local/share/omarchy-flashcards`).

mod anki;
mod audio;
mod commands;
mod db;
mod scheduler;

use anki::FieldSpec;
use clap::{Parser, Subcommand};
use serde_json::{Value, json};
use std::process::{Command, ExitCode, Stdio};

/// A user-facing failure, printed as `{"ok": false, "error": ...}` with exit status 1.
#[derive(Debug)]
pub struct CliError(pub String);

pub type Result<T> = std::result::Result<T, CliError>;

impl<E: std::error::Error> From<E> for CliError {
    fn from(e: E) -> Self {
        CliError(e.to_string())
    }
}

#[derive(Parser)]
#[command(name = "flashcards", version, about)]
struct Cli {
    /// Override the current Unix time (for tests).
    #[arg(long, global = true, hide = true)]
    now: Option<i64>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Next cards due, with counts and answer previews
    Next {
        #[arg(long)]
        deck: Option<String>,
        #[arg(long, default_value_t = db::DEFAULT_NEW_PER_DAY)]
        new_per_day: i64,
        /// How many upcoming cards to return
        #[arg(long, default_value_t = 1)]
        count: i64,
    },
    /// Record an answer: 1 again, 2 hard, 3 good, 4 easy
    Answer {
        id: i64,
        #[arg(value_parser = clap::value_parser!(i64).range(1..=4))]
        rating: i64,
    },
    /// Undo the last answer
    Undo,
    /// Add one card
    Add {
        front: String,
        back: String,
        #[arg(long, default_value = "")]
        note: String,
        /// Path to your own audio file
        #[arg(long, default_value = "")]
        audio: String,
        /// Path to a picture shown with the card
        #[arg(long, default_value = "")]
        image: String,
        #[arg(long, default_value = "Norwegian")]
        deck: String,
        /// Speech language code, e.g. no, sv, de
        #[arg(long, default_value = "no")]
        lang: String,
    },
    /// Import an Anki .apkg, or a TSV/CSV file: front, back, note, audio
    Import {
        file: String,
        #[arg(long, default_value = "Norwegian")]
        deck: String,
        #[arg(long, default_value = "no")]
        lang: String,
        #[arg(long)]
        no_prefetch: bool,
        /// Anki field for the front: a field name or its 1-based number
        #[arg(long)]
        front: Option<String>,
        /// Anki field for the back: a field name or its 1-based number
        #[arg(long)]
        back: Option<String>,
        /// Anki field for the note: a field name or its 1-based number
        #[arg(long)]
        note: Option<String>,
    },
    /// Search cards
    List {
        #[arg(default_value = "")]
        query: String,
        #[arg(long, default_value_t = 50)]
        limit: i64,
    },
    /// Delete one card
    Delete {
        id: i64,
    },
    /// Suspend new cards that only use the most frequent words
    SkipEasy {
        /// Words up to this frequency rank count as easy
        #[arg(long, default_value_t = 800)]
        max_rank: i64,
        #[arg(long)]
        dry_run: bool,
    },
    /// Bring back every suspended card
    Unsuspend,
    /// List decks with counts and the active deck
    Decks {
        #[arg(long, default_value_t = db::DEFAULT_NEW_PER_DAY)]
        new_per_day: i64,
    },
    /// Choose the deck to study; '*' studies every deck
    UseDeck {
        name: String,
    },
    /// Delete a deck and all of its cards
    DeleteDeck {
        name: String,
    },
    RenameDeck {
        old: String,
        new: String,
    },
    /// List importable files in a folder
    FindFiles {
        #[arg(long, default_value = "~/Downloads")]
        dir: String,
    },
    Stats,
    /// Fetch pronunciation audio for a card, optionally play it
    Audio {
        id: i64,
        #[arg(long)]
        play: bool,
    },
    /// Download audio for every card
    Prefetch,
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Commands that fetch audio over the network must not hold a database lock while they wait.
fn needs_transaction(cmd: &Cmd) -> bool {
    !matches!(cmd, Cmd::Audio { .. } | Cmd::Prefetch)
}

fn dispatch(db: &rusqlite::Connection, cmd: &Cmd, now: i64) -> Result<Value> {
    match cmd {
        Cmd::Next {
            deck,
            new_per_day,
            count,
        } => commands::next(db, now, deck, *new_per_day, *count),
        Cmd::Answer { id, rating } => commands::answer(db, now, *id, *rating),
        Cmd::Undo => commands::undo(db),
        Cmd::Add {
            front,
            back,
            note,
            audio,
            image,
            deck,
            lang,
        } => commands::add(db, now, front, back, note, audio, image, deck, lang),
        Cmd::Import {
            file,
            deck,
            lang,
            front,
            back,
            note,
            ..
        } => {
            let spec = FieldSpec {
                front: front.clone(),
                back: back.clone(),
                note: note.clone(),
            };
            commands::import(db, now, file, deck, lang, &spec)
        }
        Cmd::List { query, limit } => commands::list(db, query, *limit),
        Cmd::Delete { id } => commands::delete(db, *id),
        Cmd::SkipEasy { max_rank, dry_run } => commands::skip_easy(db, *max_rank, *dry_run),
        Cmd::Unsuspend => commands::unsuspend(db),
        Cmd::Decks { new_per_day } => commands::decks(db, now, *new_per_day),
        Cmd::UseDeck { name } => commands::use_deck(db, name),
        Cmd::DeleteDeck { name } => commands::delete_deck(db, name),
        Cmd::RenameDeck { old, new } => commands::rename_deck(db, old, new),
        Cmd::FindFiles { dir } => commands::find_files(dir),
        Cmd::Stats => commands::stats(db, now),
        Cmd::Audio { id, play } => commands::audio_cmd(db, *id, *play),
        Cmd::Prefetch => commands::prefetch(db),
    }
}

fn run(cmd: &Cmd, now: i64) -> Result<Value> {
    let mut conn = db::connect()?;
    let out = if needs_transaction(cmd) {
        let tx = conn.transaction()?;
        let out = dispatch(&tx, cmd, now)?;
        tx.commit()?;
        out
    } else {
        dispatch(&conn, cmd, now)?
    };
    // After the import is saved, download any missing audio in the background.
    if let Cmd::Import {
        no_prefetch: false, ..
    } = cmd
        && out["added"].as_i64().unwrap_or(0) > 0
        && let Ok(exe) = std::env::current_exe()
    {
        use std::os::unix::process::CommandExt;
        let _ = Command::new(exe)
            .arg("prefetch")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn();
    }
    Ok(out)
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli.cmd, cli.now.unwrap_or_else(unix_now)) {
        Ok(out) => {
            println!("{out}");
            ExitCode::SUCCESS
        }
        Err(CliError(message)) => {
            println!("{}", json!({"ok": false, "error": message}));
            ExitCode::from(1)
        }
    }
}
