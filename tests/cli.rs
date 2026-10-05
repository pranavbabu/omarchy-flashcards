//! Black-box tests: run the real binary against a throwaway data folder and check its JSON.

use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, UNIX_EPOCH};

const T0: i64 = 1_800_000_000;
const DAY: i64 = 86_400;

struct Env {
    dir: tempfile::TempDir,
    now: i64,
}

impl Env {
    fn new() -> Env {
        Env {
            dir: tempfile::tempdir().unwrap(),
            now: T0,
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn write(&self, name: &str, content: &[u8]) -> String {
        fs::write(self.path(name), content).unwrap();
        self.path(name).to_string_lossy().into_owned()
    }

    fn exec(&self, args: &[&str]) -> (bool, Value) {
        let out = Command::new(env!("CARGO_BIN_EXE_flashcards"))
            .env("FLASHCARDS_HOME", self.path("home"))
            .args(["--now", &self.now.to_string()])
            .args(args)
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        let json = serde_json::from_str(&text).unwrap_or_else(|_| {
            panic!("not JSON: {text} {}", String::from_utf8_lossy(&out.stderr))
        });
        (out.status.success(), json)
    }

    fn run(&self, args: &[&str]) -> Value {
        let (ok, out) = self.exec(args);
        assert!(ok, "{args:?} failed: {out}");
        out
    }

    fn fail(&self, args: &[&str]) -> String {
        let (ok, out) = self.exec(args);
        assert!(!ok, "{args:?} should have failed: {out}");
        out["error"].as_str().unwrap().to_string()
    }

    fn first_id(&self) -> String {
        self.run(&["next"])["card"]["id"].to_string()
    }

    /// Write a legacy-layout `.apkg`: one note type (id 7) with the given field names.
    fn apkg(
        &self,
        name: &str,
        fields: &[&str],
        notes: &[Vec<&str>],
        media: &[(&str, &[u8])],
    ) -> String {
        let blob = self.collection(fields, notes);
        self.zip(
            name,
            "collection.anki2",
            &blob,
            &serde_json::to_vec(&media_map(media)).unwrap(),
            media,
        )
    }

    fn collection(&self, fields: &[&str], notes: &[Vec<&str>]) -> Vec<u8> {
        let path = self.path("collection.tmp");
        let _ = fs::remove_file(&path);
        let col = rusqlite::Connection::open(&path).unwrap();
        col.execute_batch("CREATE TABLE notes (id INTEGER PRIMARY KEY, mid INTEGER, flds TEXT); CREATE TABLE col (models TEXT)").unwrap();
        let models =
            json!({"7": {"flds": fields.iter().map(|f| json!({"name": f})).collect::<Vec<_>>()}});
        col.execute("INSERT INTO col VALUES (?)", [models.to_string()])
            .unwrap();
        for (i, note) in notes.iter().enumerate() {
            col.execute(
                "INSERT INTO notes VALUES (?, 7, ?)",
                rusqlite::params![i as i64 + 1, note.join("\x1f")],
            )
            .unwrap();
        }
        drop(col);
        fs::read(path).unwrap()
    }

    fn zip(
        &self,
        name: &str,
        collection_name: &str,
        collection: &[u8],
        media_list: &[u8],
        media: &[(&str, &[u8])],
    ) -> String {
        use std::io::Write;
        let path = self.path(name);
        let mut zip = zip::ZipWriter::new(fs::File::create(&path).unwrap());
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file(collection_name, options).unwrap();
        zip.write_all(collection).unwrap();
        zip.start_file("media", options).unwrap();
        zip.write_all(media_list).unwrap();
        for (i, (_, data)) in media.iter().enumerate() {
            zip.start_file(i.to_string(), options).unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap();
        path.to_string_lossy().into_owned()
    }
}

fn media_map(media: &[(&str, &[u8])]) -> Value {
    Value::Object(
        media
            .iter()
            .enumerate()
            .map(|(i, (name, _))| (i.to_string(), json!(name)))
            .collect(),
    )
}

fn fronts(out: &Value) -> Vec<String> {
    out["cards"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["front"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn empty_database_has_no_card() {
    let e = Env::new();
    let out = e.run(&["next"]);
    assert!(out["card"].is_null());
    assert_eq!(out["due"], 0);
}

#[test]
fn add_then_next_shows_card_with_previews() {
    let e = Env::new();
    e.run(&["add", "hund", "dog", "--note", "Jeg har en hund."]);
    let out = e.run(&["next"]);
    assert_eq!(out["card"]["front"], "hund");
    assert_eq!(out["card"]["lang"], "no");
    assert_eq!(
        out["card"]["preview"],
        json!({"1": "1m", "2": "6m", "3": "10m", "4": "4d"})
    );
    assert_eq!(out["counts"], json!({"new": 1, "learning": 0, "review": 0}));
}

#[test]
fn duplicate_front_is_rejected() {
    let e = Env::new();
    e.run(&["add", "hund", "dog"]);
    assert!(e.fail(&["add", "hund", "dog again"]).contains("already"));
}

#[test]
fn fail_brings_card_back_after_a_minute_and_pass_graduates_it() {
    let mut e = Env::new();
    e.run(&["add", "katt", "cat"]);
    let id = e.first_id();
    e.run(&["answer", &id, "1"]);
    // Nothing else exists, so the card shows again inside the learn-ahead window.
    assert_eq!(e.first_id(), id);
    e.now += 61;
    assert_eq!(e.run(&["next"])["counts"]["learning"], 1);
    e.run(&["answer", &id, "3"]);
    e.now += 601;
    e.run(&["answer", &id, "3"]);
    let out = e.run(&["next"]);
    assert!(out["card"].is_null());
    assert_eq!(out["next_due_in"], DAY);
    e.now += DAY;
    assert_eq!(e.run(&["next"])["counts"]["review"], 1);
}

#[test]
fn new_cards_per_day_limit() {
    let mut e = Env::new();
    for i in 0..5 {
        e.run(&["add", &format!("ord{i}"), &format!("word{i}")]);
    }
    let new = |e: &Env| e.run(&["next", "--new-per-day", "2"])["counts"]["new"].clone();
    assert_eq!(new(&e), 2);
    let id = e.run(&["next", "--new-per-day", "2"])["card"]["id"].to_string();
    e.run(&["answer", &id, "4"]);
    assert_eq!(new(&e), 1);
    e.now += DAY;
    assert_eq!(new(&e), 2);
}

#[test]
fn next_returns_a_stack_in_study_order() {
    let mut e = Env::new();
    for i in 0..30 {
        e.run(&["add", &format!("ord{i:02}"), &format!("word{i}")]);
    }
    let out = e.run(&["next", "--count", "20", "--new-per-day", "25"]);
    assert_eq!(
        fronts(&out),
        (0..20).map(|i| format!("ord{i:02}")).collect::<Vec<_>>()
    );
    assert_eq!(out["card"]["front"], "ord00");
    e.run(&["answer", &out["cards"][0]["id"].to_string(), "1"]);
    e.now += 61;
    let out = e.run(&["next", "--count", "20", "--new-per-day", "25"]);
    assert_eq!(out["cards"][0]["front"], "ord00"); // the failed card comes back first
    let ids: std::collections::HashSet<String> = out["cards"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].to_string())
        .collect();
    assert_eq!(
        (out["cards"].as_array().unwrap().len(), ids.len()),
        (20, 20)
    );
}

#[test]
fn deck_management() {
    let e = Env::new();
    e.run(&["add", "hund", "dog", "--deck", "Norwegian"]);
    e.run(&["add", "hund", "Hund", "--deck", "German", "--lang", "de"]);
    let decks = e.run(&["decks"]);
    let rows: Vec<(String, i64, i64)> = decks["decks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            (
                d["name"].as_str().unwrap().into(),
                d["total"].as_i64().unwrap(),
                d["new"].as_i64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        vec![
            ("German".to_string(), 1, 1),
            ("Norwegian".to_string(), 1, 1)
        ]
    );
    assert!(decks["active"].is_null());
    e.run(&["use-deck", "German"]);
    assert_eq!(e.run(&["decks"])["active"], "German");
    assert!(e.fail(&["use-deck", "Nope"]).contains("no deck"));
    assert_eq!(e.run(&["next", "--deck", "German"])["card"]["lang"], "de");
    assert_eq!(e.run(&["delete-deck", "German"])["deleted"], 1);
    let out = e.run(&["decks"]);
    assert_eq!(out["decks"].as_array().unwrap().len(), 1);
    assert!(out["active"].is_null());
}

#[test]
fn new_card_limit_is_per_deck() {
    let e = Env::new();
    for deck in ["A", "B"] {
        for i in 0..3 {
            e.run(&["add", &format!("{deck}{i}"), "x", "--deck", deck]);
        }
    }
    let id = e.run(&["next", "--deck", "A", "--new-per-day", "1"])["card"]["id"].to_string();
    e.run(&["answer", &id, "4"]);
    assert_eq!(
        e.run(&["next", "--deck", "A", "--new-per-day", "1"])["counts"]["new"],
        0
    );
    assert_eq!(
        e.run(&["next", "--deck", "B", "--new-per-day", "1"])["counts"]["new"],
        1
    );
    let all = e.run(&["next", "--new-per-day", "1", "--count", "10"]);
    assert_eq!(
        (all["counts"]["new"].clone(), fronts(&all)),
        (json!(1), vec!["B0".to_string()])
    );
}

#[test]
fn rename_deck_moves_cards_and_active_deck() {
    let e = Env::new();
    e.run(&["add", "hund", "dog", "--deck", "A"]);
    e.run(&["add", "katt", "cat", "--deck", "B"]);
    e.run(&["use-deck", "A"]);
    e.run(&["rename-deck", "A", "Animals"]);
    let out = e.run(&["decks"]);
    let names: Vec<&str> = out["decks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        (names, out["active"].clone()),
        (vec!["Animals", "B"], json!("Animals"))
    );
    assert_eq!(
        e.run(&["next", "--deck", "Animals"])["card"]["front"],
        "hund"
    );
    assert!(
        e.fail(&["rename-deck", "B", "Animals"])
            .contains("already exists")
    );
}

#[test]
fn find_files_lists_importable_files_newest_first() {
    let e = Env::new();
    for name in ["a.tsv", "b.apkg", "ignore.pdf"] {
        e.write(name, b"");
    }
    let old = fs::File::options()
        .write(true)
        .open(e.path("a.tsv"))
        .unwrap();
    old.set_modified(UNIX_EPOCH + Duration::from_secs(1_000_000_000))
        .unwrap();
    let out = e.run(&["find-files", "--dir", e.dir.path().to_str().unwrap()]);
    let names: Vec<&str> = out["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["b.apkg", "a.tsv"]);
}

#[test]
fn limit_reached_reports_waiting_new_cards() {
    let e = Env::new();
    for i in 0..3 {
        e.run(&["add", &format!("ord{i}"), &format!("word{i}")]);
    }
    let out = e.run(&["next", "--new-per-day", "0"]);
    assert_eq!(
        (
            out["due"].clone(),
            out["new_waiting"].clone(),
            out["card"].is_null()
        ),
        (json!(0), json!(3), true)
    );
}

#[test]
fn undo_restores_the_card() {
    let e = Env::new();
    e.run(&["add", "hus", "house"]);
    let id = e.first_id();
    e.run(&["answer", &id, "4"]);
    assert_eq!(e.run(&["stats"])["review"], 1);
    e.run(&["undo"]);
    assert_eq!(e.run(&["stats"])["new"], 1);
    assert_eq!(e.fail(&["undo"]), "nothing to undo");
}

#[test]
fn import_tsv_skips_blank_comment_and_duplicate_lines() {
    let e = Env::new();
    let path = e.write(
        "words.tsv",
        "#separator:tab\nhei\thello\tHei, hvordan går det?\n\nhei\thi\nskje\tspoon\n\tnothing\n"
            .as_bytes(),
    );
    let out = e.run(&["import", &path, "--no-prefetch"]);
    assert_eq!(
        (out["added"].clone(), out["skipped"].clone()),
        (json!(2), json!(2))
    );
    assert_eq!(
        e.run(&["list", "hello"])["cards"][0]["note"],
        "Hei, hvordan går det?"
    );
}

#[test]
fn import_csv_semicolon() {
    let e = Env::new();
    let path = e.write(
        "w.csv",
        "takk;thanks\nvær så god;you're welcome\n".as_bytes(),
    );
    assert_eq!(e.run(&["import", &path, "--no-prefetch"])["added"], 2);
}

#[test]
fn own_audio_file_is_used_instead_of_speech() {
    let e = Env::new();
    let mp3 = e.write("x.mp3", &[b'x'; 300]);
    e.run(&["add", "bil", "car", "--audio", &mp3]);
    let out = e.run(&["audio", &e.first_id()]);
    assert_eq!(
        (out["path"].clone(), out["engine"].clone()),
        (json!(mp3), json!("file"))
    );
}

#[test]
fn audio_missing_file_reports_error() {
    let e = Env::new();
    e.run(&["add", "bil", "car", "--audio", "/nonexistent/x.mp3"]);
    assert!(e.fail(&["audio", &e.first_id()]).contains("not found"));
}

#[test]
fn import_apkg_keeps_text_html_and_audio() {
    let e = Env::new();
    let path = e.apkg(
        "d.apkg",
        &["Front", "Back", "Example"],
        &[
            vec![
                "<b>hund</b>[sound:hund.mp3]",
                "dog<br>(animal)",
                "Jeg har en <i>hund</i>.",
            ],
            vec!["katt", "cat", ""],
            vec!["", "empty front", ""],
        ],
        &[("hund.mp3", &[b'A'; 300])],
    );
    let out = e.run(&["import", &path, "--no-prefetch"]);
    assert_eq!(
        (out["added"].clone(), out["skipped"].clone()),
        (json!(2), json!(1))
    );
    let card = e.run(&["list", "dog"])["cards"][0].clone();
    assert_eq!(
        (
            card["front"].clone(),
            card["back"].clone(),
            card["note"].clone()
        ),
        (
            json!("hund"),
            json!("dog (animal)"),
            json!("Jeg har en hund.")
        )
    );
    assert_eq!(
        fs::read(card["audio"].as_str().unwrap()).unwrap(),
        vec![b'A'; 300]
    );
    assert_eq!(e.run(&["audio", &card["id"].to_string()])["engine"], "file");
}

#[test]
fn import_apkg_images_and_modes() {
    let e = Env::new();
    let png: Vec<u8> = b"\x89PNG\r\n\x1a\n"
        .iter()
        .copied()
        .chain([b'P'; 100])
        .collect();
    let path = e.apkg(
        "i.apkg",
        &["Front", "Back", "Example"],
        &[
            vec!["<img src=\"pic1.png\">", "hund", ""],
            vec!["katt<br><img src='pic1.png'>", "cat", ""],
            vec!["bil", "car<img src=pic1.png>", ""],
            vec!["<img src=\"nope.png\">", "lost", ""],
        ],
        &[("pic1.png", &png)],
    );
    let out = e.run(&["import", &path, "--no-prefetch"]);
    assert_eq!(
        (out["added"].clone(), out["skipped"].clone()),
        (json!(3), json!(1))
    );
    let listed = e.run(&["list"]);
    let card = |front: &str| {
        listed["cards"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["front"] == front)
            .unwrap()
            .clone()
    };
    assert_eq!(
        (
            card("hund")["back"].clone(),
            card("hund")["image_mode"].clone()
        ),
        (json!(""), json!("prompt"))
    );
    assert_eq!(card("katt")["image_mode"], "front");
    assert_eq!(
        (
            card("bil")["back"].clone(),
            card("bil")["image_mode"].clone()
        ),
        (json!("car"), json!("answer"))
    );
    assert_eq!(
        fs::read(card("hund")["image"].as_str().unwrap()).unwrap(),
        png
    );
    assert_eq!(e.run(&["next"])["card"]["image_mode"], "prompt");
}

#[test]
fn add_card_with_image_and_missing_image() {
    let e = Env::new();
    let img = e.write("p.png", b"x");
    e.run(&["add", "sol", "sun", "--image", &img]);
    let card = e.run(&["next"])["card"].clone();
    assert_eq!(
        (card["image"].clone(), card["image_mode"].clone()),
        (json!(img), json!("front"))
    );
    assert!(
        e.fail(&["add", "a", "b", "--image", "/nope.png"])
            .contains("image not found")
    );
}

#[test]
fn import_apkg_zstd_collection() {
    use ruzstd::encoding::{CompressionLevel, compress_to_vec};
    let e = Env::new();
    let blob = compress_to_vec(
        &e.collection(&["Front", "Back", "Example"], &[vec!["hus", "house", ""]])[..],
        CompressionLevel::Fastest,
    );
    let media = compress_to_vec(&b""[..], CompressionLevel::Fastest);
    let path = e.zip("z.apkg", "collection.anki21b", &blob, &media, &[]);
    assert_eq!(e.run(&["import", &path, "--no-prefetch"])["added"], 1);
}

#[test]
fn import_apkg_maps_fields_by_name_and_adds_article() {
    let e = Env::new();
    let fields = [
        "Rank",
        "Norwegian word",
        "Word class",
        "Article",
        "Audio, word",
        "English translation",
        "Example sentences",
    ];
    let note = vec![
        "1",
        "bil",
        "noun",
        "en",
        "[sound:b.mp3]",
        "car",
        "Bilen er rød.",
    ];
    let path = e.apkg("m.apkg", &fields, &[note], &[("b.mp3", &[b'B'; 300])]);
    e.run(&["import", &path, "--no-prefetch"]);
    let card = e.run(&["list", "car"])["cards"][0].clone();
    assert_eq!(
        (
            card["front"].clone(),
            card["back"].clone(),
            card["note"].clone()
        ),
        (json!("en bil"), json!("car (noun)"), json!("Bilen er rød."))
    );
    assert!(card["audio"].as_str().unwrap().ends_with("b.mp3"));
    e.run(&["import", &path, "--no-prefetch", "--front", "Rank"]);
    assert_eq!(e.run(&["list", "1"])["cards"][0]["front"], "1");
    assert!(
        e.fail(&["import", &path, "--front", "Nope"])
            .contains("no field")
    );
}

#[test]
fn skip_easy_suspends_frequent_words_and_sentences_made_of_them() {
    let e = Env::new();
    let notes = vec![
        vec!["1", "være", "be"],
        vec!["2", "og", "and"],
        vec!["900", "hus", "house"],
        vec!["2500", "kompleks", "complex"],
    ];
    let path = e.apkg(
        "f.apkg",
        &["Frequency index", "Norwegian word", "English translation"],
        &notes,
        &[],
    );
    e.run(&["import", &path, "--no-prefetch"]);
    e.run(&["add", "hus og være", "house and be"]);
    e.run(&["add", "hus og kompleks", "house and complex"]);
    assert_eq!(
        e.run(&["skip-easy", "--max-rank", "1000", "--dry-run"])["suspended"],
        4
    );
    assert_eq!(e.run(&["stats"])["new"], 6);
    e.run(&["skip-easy", "--max-rank", "1000"]);
    let listed = e.run(&["list"]);
    let mut left: Vec<&str> = listed["cards"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["suspended"] == 0)
        .map(|c| c["front"].as_str().unwrap())
        .collect();
    left.sort();
    assert_eq!(left, vec!["hus og kompleks", "kompleks"]);
    assert_eq!(e.run(&["next"])["counts"]["new"], 2);
    assert_eq!(e.run(&["unsuspend"])["restored"], 4);
}

#[test]
fn import_garbage_apkg_reports_error() {
    let e = Env::new();
    let path = e.write("bad.apkg", b"nope");
    assert!(e.fail(&["import", &path]).contains("not a valid"));
}

#[test]
fn starter_deck_imports_cleanly() {
    let e = Env::new();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("decks/norwegian-starter.tsv");
    let out = e.run(&["import", path.to_str().unwrap(), "--no-prefetch"]);
    assert!(out["added"].as_i64().unwrap() >= 100);
    assert_eq!(out["skipped"], 0);
}

#[test]
fn database_from_an_older_version_is_migrated() {
    let e = Env::new();
    fs::create_dir_all(e.path("home")).unwrap();
    let old = rusqlite::Connection::open(e.path("home/cards.db")).unwrap();
    old.execute_batch(
        "CREATE TABLE decks (name TEXT PRIMARY KEY, lang TEXT NOT NULL);
         CREATE TABLE cards (id INTEGER PRIMARY KEY AUTOINCREMENT, deck TEXT NOT NULL, front TEXT NOT NULL, back TEXT NOT NULL,
           note TEXT NOT NULL DEFAULT '', audio TEXT NOT NULL DEFAULT '', state INTEGER NOT NULL DEFAULT 0, step INTEGER NOT NULL DEFAULT 0,
           due INTEGER NOT NULL DEFAULT 0, interval INTEGER NOT NULL DEFAULT 0, ease INTEGER NOT NULL DEFAULT 2500, reps INTEGER NOT NULL DEFAULT 0,
           lapses INTEGER NOT NULL DEFAULT 0, suspended INTEGER NOT NULL DEFAULT 0, created INTEGER NOT NULL, UNIQUE (deck, front));
         INSERT INTO decks VALUES ('Norwegian', 'no');
         INSERT INTO cards (deck, front, back, created) VALUES ('Norwegian', 'hei', 'hello', 1);",
    )
    .unwrap();
    drop(old);
    let card = e.run(&["next"])["card"].clone();
    assert_eq!(
        (
            card["front"].clone(),
            card["rank"].clone(),
            card["image"].clone()
        ),
        (json!("hei"), Value::Null, json!(""))
    );
}
