# Omarchy Flashcards

Anki-style spaced-repetition word trainer for the Omarchy bar. Norwegian by
default, any language works. The bar shows your next word. Click it, see the
answer, hear the pronunciation, then pass or fail the card.

<img width="627" height="541" alt="image" src="https://github.com/user-attachments/assets/37aaa9cc-628f-4848-b2cb-d76a91bc3007" />

## Install

Requires Omarchy with shell plugins, a Rust toolchain (`cargo`, to build the
backend once), `curl` (to download pronunciation audio), and `mpv` (or `ffplay`) for audio.

```bash
git clone https://github.com/pranavbabu/omarchy-flashcards ~/.config/omarchy/plugins/pranavbabu.flashcards
cd ~/.config/omarchy/plugins/pranavbabu.flashcards
cargo build --release
omarchy-shell shell rescanPlugins
omarchy plugin enable pranavbabu.flashcards
```

The first build downloads crates and takes about a minute (Rust 1.88 or newer).
Until it is done, the panel shows "Backend not built". If you installed from the
marketplace or with `omarchy plugin add`, run `cargo build --release` inside
`~/.config/omarchy/plugins/pranavbabu.flashcards` once.

The folder name must stay `pranavbabu.flashcards`. If the widget does not show,
run `omarchy restart shell`. Enabling the plugin adds one entry to
`~/.config/omarchy/shell.json`. Nothing else in your configuration changes.

## Remove

```bash
omarchy plugin remove pranavbabu.flashcards
```

This removes the plugin and its `shell.json` entry. Your cards and audio stay
in `~/.local/share/omarchy-flashcards/`. Delete that folder to remove them.

## Use

1. Click the word in the bar, or run `omarchy-shell pranavbabu.flashcards toggle`.
2. Empty deck: press **Import starter Norwegian words** (191 words).
3. Study with the keyboard:

| Key | Action |
|-----|--------|
| `space` | show answer, then pass |
| `1` `2` `3` `4` | fail, hard, pass, easy (works before showing the answer) |
| `f` `p` `e` | fail, pass, easy |
| `s` | play pronunciation |
| `u` | undo last answer |
| `a` | add a word |
| `[` / `]` | previous / next deck (All decks, then each deck) |
| `d` / `r` | decks tab / review tab |
| `esc` | close |

Scheduling follows Anki: 1 and 10 minute learning steps, then days, weeks and
months. New cards are limited to 20 a day. The panel offers 10 more when the
limit is reached.

## Decks and import

- Keep each source in its own deck. Switch decks while practising with the
  arrows next to the deck name in the panel header, or with `[` and `]`.
  Each deck has its own daily limit of new cards.
- **Decks** tab: choose the deck to study (or all decks), delete a deck, and
  import a file. Files in `~/Downloads` are listed; click one, name the deck,
  press Import.
- The panel keeps the next 20 cards in memory, so the next card appears at
  once. Answers are saved in the background.

## Add words

- In the panel: press `a`.
- Anki deck: `./flashcards import deck.apkg` (text, example sentences and
  audio are imported; review history is not). Old and new Anki export formats
  both work.
- Text file: `./flashcards import words.tsv` with columns
  `front`, `back`, `note`, `audio`.
- Skip easy words: `./flashcards skip-easy` hides new cards that only use the
  800 most frequent words (needs a frequency deck). `--max-rank N` changes
  the cutoff, `unsuspend` undoes it.

Run `./flashcards` from the plugin folder, or add it to your `PATH`.

## Audio and privacy

Pronunciation audio comes from Google Translate's speech service, so word
text is sent to Google once per card and cached in
`~/.local/share/omarchy-flashcards/audio/`. Decks with their own audio, and
`espeak-ng` as an offline fallback, avoid this. Your cards live in
`~/.local/share/omarchy-flashcards/cards.db`.

## Settings

Set in the widget entry of `~/.config/omarchy/shell.json`: `deck`, `lang`
(speech language code, for example `no`, `sv`, `de`), `barLabel` (`Word`,
`Count`, `Icon`), `newPerDay`, `queueSize` (cards kept ready, default 20), `autoplay`, `popupEveryMinutes`.

## Dependencies and license

`cargo` and `rustc` to build the backend (all Rust dependencies are fetched
by `cargo`, including a bundled SQLite), `curl` for audio downloads, `mpv` or `ffplay` for playback,
`espeak-ng` optional. MIT license, see `LICENSE`.

## Tests

```bash
cargo test --release
```
