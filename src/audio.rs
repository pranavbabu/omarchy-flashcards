//! Pronunciation audio: the card's own file, a cached text-to-speech download, or live speech.

use crate::db::{self, Card};
use crate::{CliError, Result};
use sha1::{Digest, Sha1};
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sha1_hex(data: &[u8]) -> String {
    hex(&Sha1::digest(data))
}

pub fn expand_user(path: &str) -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    match path {
        "~" => PathBuf::from(home),
        _ => match path.strip_prefix("~/") {
            Some(rest) => PathBuf::from(home).join(rest),
            None => PathBuf::from(path),
        },
    }
}

pub fn audio_path(card: &Card, lang: &str) -> PathBuf {
    if !card.audio.is_empty() {
        return expand_user(&card.audio);
    }
    let key = &sha1_hex(format!("{lang}\0{}", card.front).as_bytes())[..20];
    db::audio_dir().join(format!("{lang}-{key}.mp3"))
}

fn fetch_tts(text: &str, lang: &str, dest: &Path) -> Result<()> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .build()
        .into();
    let spoken: String = text.chars().take(200).collect();
    let mut response = agent
        .get("https://translate.google.com/translate_tts")
        .header("User-Agent", "Mozilla/5.0")
        .query("ie", "UTF-8")
        .query("client", "tw-ob")
        .query("tl", lang)
        .query("q", &spoken)
        .call()
        .map_err(|e| CliError(e.to_string()))?;
    let mut data = Vec::new();
    response.body_mut().as_reader().read_to_end(&mut data)?;
    if data.len() < 200 {
        return Err(CliError("text-to-speech returned no audio".into()));
    }
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = PathBuf::from(format!("{}.part", dest.display()));
    std::fs::write(&tmp, data)?;
    std::fs::rename(tmp, dest)?;
    Ok(())
}

pub fn which(program: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join(program))
        .find(|p| p.is_file())
}

/// Start a program detached from this process, with no input or output.
fn spawn_detached(program: &Path, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .is_ok()
}

pub fn play(path: &Path) -> bool {
    let file = path.to_string_lossy();
    if let Some(mpv) = which("mpv") {
        return spawn_detached(&mpv, &["--no-video", "--really-quiet", &file]);
    }
    if let Some(ffplay) = which("ffplay") {
        return spawn_detached(
            &ffplay,
            &["-nodisp", "-autoexit", "-loglevel", "quiet", &file],
        );
    }
    false
}

pub fn speak_live(lang: &str, text: &str) -> Result<()> {
    let espeak = which("espeak-ng")
        .ok_or_else(|| CliError("no audio: offline and espeak-ng is not installed".into()))?;
    let voice = match lang {
        "no" | "nb" | "nn" => "nb",
        other => other,
    };
    spawn_detached(&espeak, &["-v", voice, text]);
    Ok(())
}

/// A playable file for the card, or `None` when only live speech is possible.
pub fn ensure_audio(card: &Card, lang: &str) -> Result<Option<PathBuf>> {
    let path = audio_path(card, lang);
    if path.is_file() {
        return Ok(Some(path));
    }
    if !card.audio.is_empty() {
        return Err(CliError(format!(
            "audio file not found: {}",
            path.display()
        )));
    }
    Ok(fetch_tts(&card.front, lang, &path).ok().map(|_| path))
}
