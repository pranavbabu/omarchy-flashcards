//! Reading Anki `.apkg` packages: the zip, the collection database (old and new
//! layouts), the media list (JSON or zstd + protobuf), and field mapping.

use crate::{CliError, Result};
use regex::Regex;
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::sync::LazyLock;

/// User overrides for which Anki field becomes the front, back and note.
#[derive(Default, Clone)]
pub struct FieldSpec {
    pub front: Option<String>,
    pub back: Option<String>,
    pub note: Option<String>,
}

pub struct ApkgNote {
    pub front: String,
    pub back: String,
    pub note: String,
    pub sound: Option<String>,
    pub media: Option<Vec<u8>>,
    pub rank: Option<i64>,
    pub image: Option<String>,
    pub image_bytes: Option<Vec<u8>>,
    pub image_mode: String,
}

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid regex")
}

static SOUND: LazyLock<Regex> = LazyLock::new(|| re(r"\[sound:[^\]]*\]"));
static SOUND_NAME: LazyLock<Regex> = LazyLock::new(|| re(r"\[sound:([^\]]+)\]"));
static BREAK: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)<\s*(br|/div|/p)\s*/?>"));
static TAG: LazyLock<Regex> = LazyLock::new(|| re(r"<[^>]+>"));
static SPACE: LazyLock<Regex> = LazyLock::new(|| re(r"\s+"));
static IMG: LazyLock<Regex> =
    LazyLock::new(|| re(r#"(?i)<img[^>]*?\ssrc\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))"#));

const NOISE: &[&str] = &[
    "audio",
    "ipa",
    "example",
    "index",
    "class",
    "inflect",
    "dictionary",
    "sound",
    "article",
];

/// Visible text of an Anki field: no sound tags or markup, entities decoded, whitespace collapsed.
pub fn plain(text: &str) -> String {
    let t = SOUND.replace_all(text, "");
    let t = BREAK.replace_all(&t, " ");
    let t = TAG.replace_all(&t, "");
    let t = html_escape::decode_html_entities(&t);
    SPACE.replace_all(&t, " ").trim().to_string()
}

fn images(text: &str) -> Vec<String> {
    IMG.captures_iter(text)
        .filter_map(|c| (1..=3).find_map(|i| c.get(i)))
        .map(|m| html_escape::decode_html_entities(m.as_str()).into_owned())
        .collect()
}

fn unquote(s: &str) -> String {
    percent_encoding::percent_decode_str(s)
        .decode_utf8_lossy()
        .into_owned()
}

pub fn zstd_decompress(data: &[u8]) -> Result<Vec<u8>> {
    let mut decoder = ruzstd::decoding::StreamingDecoder::new(data)
        .map_err(|e| CliError(format!("bad zstd data: {e}")))?;
    let mut out = Vec::new();
    decoder
        .read_to_end(&mut out)
        .map_err(|e| CliError(format!("bad zstd data: {e}")))?;
    Ok(out)
}

fn varint(buf: &[u8], i: &mut usize) -> Option<u64> {
    let (mut n, mut shift) = (0u64, 0u32);
    loop {
        let b = *buf.get(*i)?;
        *i += 1;
        n |= u64::from(b & 0x7F) << shift;
        if b < 0x80 {
            return Some(n);
        }
        shift += 7;
        if shift > 63 {
            return None;
        }
    }
}

/// Pairs of (zip member name, media file name), from the legacy JSON map or the newer
/// zstd-compressed protobuf list, whose entries are numbered by position.
pub fn media_names(raw: &[u8]) -> Result<Vec<(String, String)>> {
    if let Ok(text) = std::str::from_utf8(raw)
        && let Ok(serde_json::Value::Object(map)) = serde_json::from_str(text)
    {
        return Ok(map
            .into_iter()
            .filter_map(|(k, v)| v.as_str().map(|s| (k, s.to_string())))
            .collect());
    }
    let buf = zstd_decompress(raw)?;
    let bad = || CliError("bad media list".into());
    let (mut i, mut names) = (0usize, Vec::new());
    while i < buf.len() {
        varint(&buf, &mut i).ok_or_else(bad)?; // tag
        let size = varint(&buf, &mut i).ok_or_else(bad)? as usize;
        let entry = buf.get(i..i + size).ok_or_else(bad)?;
        i += size;
        let (mut j, mut name) = (0usize, String::new());
        while j < entry.len() {
            let tag = varint(entry, &mut j).ok_or_else(bad)?;
            if tag & 7 == 2 {
                let len = varint(entry, &mut j).ok_or_else(bad)? as usize;
                if tag >> 3 == 1 {
                    name = String::from_utf8_lossy(entry.get(j..j + len).ok_or_else(bad)?)
                        .into_owned();
                }
                j += len;
            } else {
                varint(entry, &mut j).ok_or_else(bad)?;
            }
        }
        names.push((names.len().to_string(), name));
    }
    Ok(names)
}

/// `{model id: [field names in order]}` for both the old (`col.models` JSON) and new (`fields` table) layouts.
fn models(col: &Connection) -> rusqlite::Result<HashMap<i64, Vec<String>>> {
    let has_fields: bool = col.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'fields'",
        [],
        |r| r.get::<_, i64>(0),
    )? > 0;
    let mut out: HashMap<i64, Vec<String>> = HashMap::new();
    if has_fields {
        let mut stmt = col.prepare("SELECT ntid, name FROM fields ORDER BY ntid, ord")?;
        for row in stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))? {
            let (id, name) = row?;
            out.entry(id).or_default().push(name);
        }
        return Ok(out);
    }
    let raw: String = col.query_row("SELECT models FROM col", [], |r| r.get(0))?;
    if let Ok(serde_json::Value::Object(map)) = serde_json::from_str(&raw) {
        for (key, model) in map {
            if let (Ok(id), Some(fields)) = (
                key.parse::<i64>(),
                model.get("flds").and_then(|f| f.as_array()),
            ) {
                out.insert(
                    id,
                    fields
                        .iter()
                        .filter_map(|f| f.get("name")?.as_str().map(String::from))
                        .collect(),
                );
            }
        }
    }
    Ok(out)
}

/// Index of the first field whose lowercase name matches `include` and none of `exclude`.
fn pick(names: &[String], include: &str, exclude: &[&str]) -> Option<usize> {
    let include = re(include);
    let exclude: Vec<Regex> = exclude.iter().map(|x| re(x)).collect();
    names.iter().position(|n| {
        let low = n.to_lowercase();
        include.is_match(&low) && !exclude.iter().any(|x| x.is_match(&low))
    })
}

/// Field index from a user spec (a name or a 1-based number), else from name heuristics.
fn resolve(
    names: &[String],
    spec: &Option<String>,
    include: &str,
    exclude: &[&str],
) -> Result<Option<usize>> {
    match spec.as_deref().filter(|s| !s.is_empty()) {
        None => Ok(pick(names, include, exclude)),
        Some(s) => {
            if s.chars().all(|c| c.is_ascii_digit())
                && let Ok(n) = s.parse::<usize>()
                && (1..=names.len()).contains(&n)
            {
                return Ok(Some(n - 1));
            }
            names
                .iter()
                .position(|n| n.to_lowercase() == s.to_lowercase())
                .map(Some)
                .ok_or_else(|| CliError(format!("no field '{s}'. Fields: {}", names.join(", "))))
        }
    }
}

struct Plan {
    front: usize,
    back: usize,
    note: Option<usize>,
    article: Option<usize>,
    class: Option<usize>,
    audio: Vec<usize>,
    rank: Option<usize>,
}

fn plan(names: &[String], spec: &FieldSpec) -> Result<Plan> {
    let front = resolve(
        names,
        &spec.front,
        "word|front|term|expression|question",
        NOISE,
    )?
    .unwrap_or(0);
    let back = match resolve(
        names,
        &spec.back,
        "translation|meaning|back|english|answer|definition",
        &["general", "audio", "nuance"],
    )? {
        Some(b) => b,
        None => usize::from(names.len() > 1 && front == 0),
    };
    let audio_re = re("audio|sound|pronunc");
    Ok(Plan {
        front,
        back,
        note: resolve(names, &spec.note, "example|sentence", &[])?,
        article: if spec.front.as_deref().is_some_and(|s| !s.is_empty()) {
            None
        } else {
            pick(names, "^article", &[])
        },
        class: pick(names, "word class|part of speech|^pos$", &[]),
        audio: names
            .iter()
            .enumerate()
            .filter(|(_, n)| {
                let low = n.to_lowercase();
                audio_re.is_match(&low) && !low.contains("example") && !low.contains("container")
            })
            .map(|(i, _)| i)
            .collect(),
        rank: pick(names, "frequency|rank", &[]),
    })
}

/// Read every note of an `.apkg` and hand it to `on_note`, one at a time so large decks stay out of memory.
pub fn read_apkg(
    path: &str,
    spec: &FieldSpec,
    mut on_note: impl FnMut(ApkgNote) -> Result<()>,
) -> Result<()> {
    let invalid = || CliError(format!("not a valid Anki package: {path}"));
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path)?).map_err(|_| invalid())?;
    let members: HashSet<String> = zip.file_names().map(String::from).collect();
    let collection = [
        "collection.anki21b",
        "collection.anki21",
        "collection.anki2",
    ]
    .into_iter()
    .find(|n| members.contains(*n))
    .ok_or_else(|| CliError("not an Anki package: no collection found".into()))?;

    let mut read_member = |name: &str| -> Option<Vec<u8>> {
        let mut buf = Vec::new();
        zip.by_name(name).ok()?.read_to_end(&mut buf).ok()?;
        Some(buf)
    };
    let mut blob = read_member(collection).ok_or_else(invalid)?;
    if collection.ends_with("21b") {
        blob = zstd_decompress(&blob).map_err(|_| invalid())?;
    }
    let names = match members.contains("media") {
        true => media_names(&read_member("media").ok_or_else(invalid)?).map_err(|_| invalid())?,
        false => Vec::new(),
    };
    // Last entry wins when two zip members carry the same file name.
    let by_name: HashMap<String, String> = names
        .into_iter()
        .map(|(member, file)| (file, member))
        .collect();

    let tmp = tempfile::NamedTempFile::new()?;
    std::fs::write(tmp.path(), &blob)?;
    let col = Connection::open(tmp.path()).map_err(|_| invalid())?;
    let model_fields = models(&col).map_err(|_| invalid())?;
    let rows: Vec<(i64, String)> = {
        let mut stmt = col
            .prepare("SELECT mid, flds FROM notes ORDER BY id")
            .map_err(|_| invalid())?;
        let mapped = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|_| invalid())?;
        mapped
            .collect::<rusqlite::Result<_>>()
            .map_err(|_| invalid())?
    };
    drop(col);

    let mut plans: HashMap<i64, Plan> = HashMap::new();
    for (mid, flds) in rows {
        let mut fields: Vec<&str> = flds.split('\x1f').collect();
        let names = match model_fields.get(&mid) {
            Some(n) if !n.is_empty() => n.clone(),
            _ => (1..=fields.len()).map(|i| format!("Field {i}")).collect(),
        };
        if let std::collections::hash_map::Entry::Vacant(e) = plans.entry(mid) {
            e.insert(plan(&names, spec)?);
        }
        let p = &plans[&mid];
        fields.extend(std::iter::repeat_n("", names.len()));

        let mut word = plain(fields[p.front]);
        if let Some(a) = p
            .article
            .map(|a| plain(fields[a]))
            .filter(|a| !a.is_empty())
        {
            word = format!("{a} {word}");
        }
        let mut meaning = plain(fields[p.back]);
        if let Some(c) = p.class.map(|c| plain(fields[c])).filter(|c| !c.is_empty())
            && !meaning.is_empty()
        {
            meaning = format!("{meaning} ({c})");
        }
        let sound = p
            .audio
            .iter()
            .find_map(|&i| SOUND_NAME.captures(fields[i]))
            .or_else(|| SOUND_NAME.captures(&flds))
            .map(|c| c[1].to_string());
        let media = sound
            .as_ref()
            .and_then(|s| by_name.get(s))
            .and_then(|member| read_member(member));
        let rank = p
            .rank
            .map(|r| plain(fields[r]))
            .filter(|r| !r.is_empty() && r.chars().all(|c| c.is_ascii_digit()))
            .and_then(|r| r.parse().ok());

        let (mut image, mut image_bytes, mut image_mode) = (None, None, String::new());
        for index in [p.front, p.back].into_iter().chain(0..names.len()) {
            let found = images(fields[index])
                .into_iter()
                .find(|i| by_name.contains_key(i) || by_name.contains_key(&unquote(i)));
            if let Some(found) = found {
                let name = if by_name.contains_key(&found) {
                    found
                } else {
                    unquote(&found)
                };
                image_bytes = read_member(&by_name[&name]);
                image = Some(name);
                image_mode = if index == p.front { "front" } else { "answer" }.to_string();
                break;
            }
        }
        if image.is_some() && word.is_empty() && !meaning.is_empty() {
            word = std::mem::take(&mut meaning);
            image_mode = "prompt".to_string();
        }
        let note = p.note.map(|n| plain(fields[n])).unwrap_or_default();
        on_note(ApkgNote {
            front: word,
            back: meaning,
            note,
            sound,
            media,
            rank,
            image,
            image_bytes,
            image_mode,
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_strips_markup_sounds_and_entities() {
        assert_eq!(plain("<b>hund</b>[sound:a.mp3]"), "hund");
        assert_eq!(plain("dog<br>(animal)"), "dog (animal)");
        assert_eq!(plain("Tom &amp; Jerry&nbsp;&lt;3"), "Tom & Jerry <3");
    }

    #[test]
    fn image_sources_in_all_quote_styles() {
        assert_eq!(
            images(r#"<img src="a b.png"> <IMG class=x src='c.png'> <img src=d.png>"#),
            vec!["a b.png", "c.png", "d.png"]
        );
    }

    #[test]
    fn legacy_media_list_keeps_order() {
        let names = media_names(br#"{"0":"a.mp3","1":"b.mp3"}"#).unwrap();
        assert_eq!(
            names,
            vec![("0".into(), "a.mp3".into()), ("1".into(), "b.mp3".into())]
        );
    }

    #[test]
    fn new_media_list_is_zstd_protobuf_numbered_by_position() {
        fn entry(name: &str) -> Vec<u8> {
            let mut body = vec![0x0a, name.len() as u8];
            body.extend(name.as_bytes());
            body.extend([0x10, 0x05]);
            let mut out = vec![0x0a, body.len() as u8];
            out.extend(body);
            out
        }
        let raw = [entry("a.mp3"), entry("b.mp3")].concat();
        let packed = ruzstd::encoding::compress_to_vec(
            &raw[..],
            ruzstd::encoding::CompressionLevel::Fastest,
        );
        assert_eq!(
            media_names(&packed).unwrap(),
            vec![("0".into(), "a.mp3".into()), ("1".into(), "b.mp3".into())]
        );
    }

    #[test]
    fn empty_zstd_media_list_is_empty() {
        let packed = ruzstd::encoding::compress_to_vec(
            &b""[..],
            ruzstd::encoding::CompressionLevel::Fastest,
        );
        assert!(media_names(&packed).unwrap().is_empty());
    }

    #[test]
    fn field_spec_accepts_names_and_numbers_and_rejects_unknown() {
        let names: Vec<String> = ["Rank", "Norwegian word", "English"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            resolve(&names, &Some("2".into()), "x", &[]).unwrap(),
            Some(1)
        );
        assert_eq!(
            resolve(&names, &Some("english".into()), "x", &[]).unwrap(),
            Some(2)
        );
        assert!(
            resolve(&names, &Some("Nope".into()), "x", &[])
                .unwrap_err()
                .0
                .starts_with("no field 'Nope'")
        );
    }
}
