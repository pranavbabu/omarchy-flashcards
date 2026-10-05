import importlib.machinery
import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SCRIPT = os.path.join(ROOT, "flashcards")
loader = importlib.machinery.SourceFileLoader("flashcards_mod", SCRIPT)
spec = importlib.util.spec_from_loader("flashcards_mod", loader)
fc = importlib.util.module_from_spec(spec)
loader.exec_module(fc)

T0 = 1_800_000_000
NEW_CARD = dict(id=1, state=0, step=0, due=0, interval=0, ease=2500, reps=0, lapses=0)


class Scheduler(unittest.TestCase):
    def test_new_card_again_stays_in_learning_for_one_minute(self):
        c = fc.schedule(NEW_CARD, fc.AGAIN, T0)
        self.assertEqual((c["state"], c["step"], c["due"]), (fc.LEARNING, 0, T0 + 60))

    def test_good_walks_learning_steps_then_graduates_to_one_day(self):
        c = fc.schedule(NEW_CARD, fc.GOOD, T0)
        self.assertEqual((c["state"], c["step"], c["due"]), (fc.LEARNING, 1, T0 + 600))
        c = fc.schedule(c, fc.GOOD, T0 + 600)
        self.assertEqual((c["state"], c["interval"], c["due"]), (fc.REVIEW, 1, T0 + 600 + fc.DAY))

    def test_easy_graduates_new_card_at_four_days(self):
        c = fc.schedule(NEW_CARD, fc.EASY, T0)
        self.assertEqual((c["state"], c["interval"]), (fc.REVIEW, 4))

    def test_review_passing_intervals_strictly_increase(self):
        card = dict(NEW_CARD, state=fc.REVIEW, interval=10, ease=2500)
        days = [fc.schedule(card, r, T0)["interval"] for r in (fc.HARD, fc.GOOD, fc.EASY)]
        self.assertEqual(days, [12, 25, 32])

    def test_review_fail_is_a_lapse_that_lowers_ease_and_relearns(self):
        card = dict(NEW_CARD, state=fc.REVIEW, interval=10, ease=2500)
        c = fc.schedule(card, fc.AGAIN, T0)
        self.assertEqual((c["state"], c["lapses"], c["ease"], c["due"]), (fc.RELEARNING, 1, 2300, T0 + 600))
        c = fc.schedule(c, fc.GOOD, T0 + 600)
        self.assertEqual((c["state"], c["interval"]), (fc.REVIEW, 5))

    def test_ease_never_drops_below_floor(self):
        card = dict(NEW_CARD, state=fc.REVIEW, interval=3, ease=1350)
        self.assertEqual(fc.schedule(card, fc.AGAIN, T0)["ease"], fc.MIN_EASE)

    def test_interval_labels(self):
        self.assertEqual([fc.fmt_interval(s) for s in (30, 600, 7200, 4 * fc.DAY, 45 * fc.DAY)],
                         ["<1m", "10m", "2h", "4d", "1.5mo"])


class Cli(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.env = dict(os.environ, FLASHCARDS_HOME=self.tmp.name)
        self.now = T0

    def tearDown(self):
        self.tmp.cleanup()

    def run_cli(self, *args, ok=True):
        p = subprocess.run([sys.executable, SCRIPT, "--now", str(self.now), *args],
                           capture_output=True, text=True, env=self.env)
        self.assertEqual(p.returncode == 0, ok, p.stdout + p.stderr)
        return json.loads(p.stdout)

    def test_empty_database_has_no_card(self):
        out = self.run_cli("next")
        self.assertIsNone(out["card"])
        self.assertEqual(out["due"], 0)

    def test_add_then_next_shows_card_with_previews(self):
        self.run_cli("add", "hund", "dog", "--note", "Jeg har en hund.")
        out = self.run_cli("next")
        self.assertEqual(out["card"]["front"], "hund")
        self.assertEqual(out["card"]["lang"], "no")
        self.assertEqual(out["card"]["preview"], {"1": "1m", "2": "6m", "3": "10m", "4": "4d"})
        self.assertEqual(out["counts"], {"new": 1, "learning": 0, "review": 0})

    def test_duplicate_front_is_rejected(self):
        self.run_cli("add", "hund", "dog")
        out = self.run_cli("add", "hund", "dog again", ok=False)
        self.assertIn("already", out["error"])

    def test_fail_brings_card_back_after_a_minute_and_pass_graduates_it(self):
        self.run_cli("add", "katt", "cat")
        cid = self.run_cli("next")["card"]["id"]
        self.run_cli("answer", str(cid), "1")
        # Nothing else exists, so the card shows again inside the learn-ahead window.
        self.assertEqual(self.run_cli("next")["card"]["id"], cid)
        self.now += 61
        self.assertEqual(self.run_cli("next")["counts"]["learning"], 1)
        self.run_cli("answer", str(cid), "3")
        self.now += 601
        self.run_cli("answer", str(cid), "3")
        out = self.run_cli("next")
        self.assertIsNone(out["card"])
        self.assertEqual(out["next_due_in"], fc.DAY)
        self.now += fc.DAY
        self.assertEqual(self.run_cli("next")["counts"]["review"], 1)

    def test_new_cards_per_day_limit(self):
        for i in range(5):
            self.run_cli("add", "ord%d" % i, "word%d" % i)
        self.assertEqual(self.run_cli("next", "--new-per-day", "2")["counts"]["new"], 2)
        cid = self.run_cli("next", "--new-per-day", "2")["card"]["id"]
        self.run_cli("answer", str(cid), "4")
        self.assertEqual(self.run_cli("next", "--new-per-day", "2")["counts"]["new"], 1)
        self.now += fc.DAY
        self.assertEqual(self.run_cli("next", "--new-per-day", "2")["counts"]["new"], 2)

    def test_next_returns_a_stack_in_study_order(self):
        for i in range(30):
            self.run_cli("add", "ord%02d" % i, "word%d" % i)
        out = self.run_cli("next", "--count", "20", "--new-per-day", "25")
        self.assertEqual([c["front"] for c in out["cards"]], ["ord%02d" % i for i in range(20)])
        self.assertEqual(out["card"]["front"], "ord00")
        self.run_cli("answer", str(out["cards"][0]["id"]), "1")
        self.now += 61
        out = self.run_cli("next", "--count", "20", "--new-per-day", "25")
        self.assertEqual(out["cards"][0]["front"], "ord00")  # the failed card comes back first
        self.assertEqual(len(out["cards"]), 20)
        self.assertEqual(len({c["id"] for c in out["cards"]}), 20)

    def test_deck_management(self):
        self.run_cli("add", "hund", "dog", "--deck", "Norwegian")
        self.run_cli("add", "hund", "Hund", "--deck", "German", "--lang", "de")
        decks = self.run_cli("decks")
        self.assertEqual([(d["name"], d["total"], d["new"]) for d in decks["decks"]], [("German", 1, 1), ("Norwegian", 1, 1)])
        self.assertIsNone(decks["active"])
        self.run_cli("use-deck", "German")
        self.assertEqual(self.run_cli("decks")["active"], "German")
        self.assertIn("no deck", self.run_cli("use-deck", "Nope", ok=False)["error"])
        self.assertEqual(self.run_cli("next", "--deck", "German")["card"]["lang"], "de")
        self.assertEqual(self.run_cli("delete-deck", "German")["deleted"], 1)
        out = self.run_cli("decks")
        self.assertEqual(([d["name"] for d in out["decks"]], out["active"]), (["Norwegian"], None))

    def test_new_card_limit_is_per_deck(self):
        for deck in ("A", "B"):
            for i in range(3):
                self.run_cli("add", "%s%d" % (deck, i), "x", "--deck", deck)
        cid = self.run_cli("next", "--deck", "A", "--new-per-day", "1")["card"]["id"]
        self.run_cli("answer", str(cid), "4")
        self.assertEqual(self.run_cli("next", "--deck", "A", "--new-per-day", "1")["counts"]["new"], 0)
        self.assertEqual(self.run_cli("next", "--deck", "B", "--new-per-day", "1")["counts"]["new"], 1)
        allq = self.run_cli("next", "--new-per-day", "1", "--count", "10")
        self.assertEqual((allq["counts"]["new"], [c["front"] for c in allq["cards"]]), (1, ["B0"]))

    def test_rename_deck_moves_cards_and_active_deck(self):
        self.run_cli("add", "hund", "dog", "--deck", "A")
        self.run_cli("add", "katt", "cat", "--deck", "B")
        self.run_cli("use-deck", "A")
        self.run_cli("rename-deck", "A", "Animals")
        out = self.run_cli("decks")
        self.assertEqual(([d["name"] for d in out["decks"]], out["active"]), (["Animals", "B"], "Animals"))
        self.assertEqual(self.run_cli("next", "--deck", "Animals")["card"]["front"], "hund")
        self.assertIn("already exists", self.run_cli("rename-deck", "B", "Animals", ok=False)["error"])

    def test_find_files_lists_importable_files_newest_first(self):
        for name in ("a.tsv", "b.apkg", "ignore.pdf"):
            open(os.path.join(self.tmp.name, name), "w").close()
        os.utime(os.path.join(self.tmp.name, "a.tsv"), (1_000_000_000, 1_000_000_000))
        names = [f["name"] for f in self.run_cli("find-files", "--dir", self.tmp.name)["files"]]
        self.assertEqual(names, ["b.apkg", "a.tsv"])

    def test_limit_reached_reports_waiting_new_cards(self):
        for i in range(3):
            self.run_cli("add", "ord%d" % i, "word%d" % i)
        out = self.run_cli("next", "--new-per-day", "0")
        self.assertEqual((out["due"], out["new_waiting"], out["card"]), (0, 3, None))

    def test_undo_restores_the_card(self):
        self.run_cli("add", "hus", "house")
        cid = self.run_cli("next")["card"]["id"]
        self.run_cli("answer", str(cid), "4")
        self.assertEqual(self.run_cli("stats")["review"], 1)
        self.run_cli("undo")
        self.assertEqual(self.run_cli("stats")["new"], 1)

    def test_import_tsv_skips_blank_comment_and_duplicate_lines(self):
        path = os.path.join(self.tmp.name, "words.tsv")
        with open(path, "w", encoding="utf-8") as fh:
            fh.write("#separator:tab\nhei\thello\tHei, hvordan går det?\n\nhei\thi\nskje\tspoon\n\tnothing\n")
        out = self.run_cli("import", path, "--no-prefetch")
        self.assertEqual((out["added"], out["skipped"]), (2, 2))
        self.assertEqual(self.run_cli("list", "hello")["cards"][0]["note"], "Hei, hvordan går det?")

    def test_import_csv_semicolon(self):
        path = os.path.join(self.tmp.name, "w.csv")
        with open(path, "w", encoding="utf-8") as fh:
            fh.write("takk;thanks\nvær så god;you're welcome\n")
        self.assertEqual(self.run_cli("import", path, "--no-prefetch")["added"], 2)

    def test_own_audio_file_is_used_instead_of_speech(self):
        wav = os.path.join(self.tmp.name, "x.mp3")
        open(wav, "wb").write(b"x" * 300)
        self.run_cli("add", "bil", "car", "--audio", wav)
        cid = self.run_cli("next")["card"]["id"]
        out = self.run_cli("audio", str(cid))
        self.assertEqual((out["path"], out["engine"]), (wav, "file"))

    def test_audio_missing_file_reports_error(self):
        self.run_cli("add", "bil", "car", "--audio", "/nonexistent/x.mp3")
        cid = self.run_cli("next")["card"]["id"]
        self.assertIn("not found", self.run_cli("audio", str(cid), ok=False)["error"])

    def make_apkg(self, name, notes, media, legacy=True):
        import sqlite3, zipfile
        dbp = os.path.join(self.tmp.name, "col.db")
        if os.path.exists(dbp):
            os.remove(dbp)
        con = sqlite3.connect(dbp)
        con.execute("CREATE TABLE notes (id INTEGER PRIMARY KEY, mid INTEGER, flds TEXT)")
        con.execute("CREATE TABLE col (models TEXT)")
        con.execute("INSERT INTO col VALUES (?)", (json.dumps({"7": {"flds": [{"name": n} for n in ("Front", "Back", "Example")]}}),))
        for i, flds in enumerate(notes):
            con.execute("INSERT INTO notes VALUES (?, 7, ?)", (i + 1, "\x1f".join(flds)))
        con.commit()
        con.close()
        path = os.path.join(self.tmp.name, name)
        with zipfile.ZipFile(path, "w") as z:
            z.write(dbp, "collection.anki2" if legacy else "collection.anki21b")
            z.writestr("media", json.dumps({str(i): n for i, n in enumerate(media)}))
            for i, data in enumerate(media.values()):
                z.writestr(str(i), data)
        return path

    def test_import_apkg_keeps_text_html_and_audio(self):
        path = self.make_apkg("d.apkg", [
            ["<b>hund</b>[sound:hund.mp3]", "dog<br>(animal)", "Jeg har en <i>hund</i>."],
            ["katt", "cat", ""],
            ["", "empty front", ""],
        ], {"hund.mp3": b"A" * 300})
        out = self.run_cli("import", path, "--no-prefetch")
        self.assertEqual((out["added"], out["skipped"]), (2, 1))
        card = self.run_cli("list", "dog")["cards"][0]
        self.assertEqual((card["front"], card["back"], card["note"]), ("hund", "dog (animal)", "Jeg har en hund."))
        self.assertEqual(open(card["audio"], "rb").read(), b"A" * 300)
        self.assertEqual(self.run_cli("audio", str(card["id"]))["engine"], "file")

    def test_import_apkg_zstd_collection(self):
        from compression import zstd
        import zipfile
        legacy = self.make_apkg("n.apkg", [["hus", "house", ""]], {})
        with zipfile.ZipFile(legacy) as z:
            blob = zstd.compress(z.read("collection.anki2"))
        path = os.path.join(self.tmp.name, "z.apkg")
        with zipfile.ZipFile(path, "w") as z:
            z.writestr("collection.anki21b", blob)
            z.writestr("media", zstd.compress(b""))
        self.assertEqual(self.run_cli("import", path, "--no-prefetch")["added"], 1)

    def test_import_apkg_maps_fields_by_name_and_adds_article(self):
        import sqlite3, zipfile
        path = self.make_apkg("m.apkg", [["x"]], {})
        dbp = os.path.join(self.tmp.name, "col.db")
        con = sqlite3.connect(dbp)
        names = ["Rank", "Norwegian word", "Word class", "Article", "Audio, word", "English translation", "Example sentences"]
        con.execute("UPDATE col SET models = ?", (json.dumps({"7": {"flds": [{"name": n} for n in names]}}),))
        con.execute("DELETE FROM notes")
        con.execute("INSERT INTO notes VALUES (1, 7, ?)", ("\x1f".join(["1", "bil", "noun", "en", "[sound:b.mp3]", "car", "Bilen er rød."]),))
        con.commit()
        con.close()
        with zipfile.ZipFile(path, "w") as z:
            z.write(dbp, "collection.anki2")
            z.writestr("media", json.dumps({"0": "b.mp3"}))
            z.writestr("0", b"B" * 300)
        self.run_cli("import", path, "--no-prefetch")
        card = self.run_cli("list", "car")["cards"][0]
        self.assertEqual((card["front"], card["back"], card["note"]), ("en bil", "car (noun)", "Bilen er rød."))
        self.assertTrue(card["audio"].endswith("b.mp3"))
        self.run_cli("import", path, "--no-prefetch", "--front", "Rank", ok=True)
        self.assertEqual(self.run_cli("list", "1")["cards"][0]["front"], "1")
        self.assertIn("no field", self.run_cli("import", path, "--front", "Nope", ok=False)["error"])

    def test_skip_easy_suspends_frequent_words_and_sentences_made_of_them(self):
        import sqlite3, zipfile
        path = self.make_apkg("f.apkg", [["x"]], {})
        dbp = os.path.join(self.tmp.name, "col.db")
        con = sqlite3.connect(dbp)
        names = ["Frequency index", "Norwegian word", "English translation"]
        con.execute("UPDATE col SET models = ?", (json.dumps({"7": {"flds": [{"name": n} for n in names]}}),))
        con.execute("DELETE FROM notes")
        for i, (rank, word, en) in enumerate([(1, "være", "be"), (2, "og", "and"), (900, "hus", "house"), (2500, "kompleks", "complex")]):
            con.execute("INSERT INTO notes VALUES (?, 7, ?)", (i + 1, "\x1f".join([str(rank), word, en])))
        con.commit()
        con.close()
        with zipfile.ZipFile(path, "w") as z:
            z.write(dbp, "collection.anki2")
        self.run_cli("import", path, "--no-prefetch")
        self.run_cli("add", "hus og være", "house and be")
        self.run_cli("add", "hus og kompleks", "house and complex")
        dry = self.run_cli("skip-easy", "--max-rank", "1000", "--dry-run")
        self.assertEqual(dry["suspended"], 4)
        self.assertEqual(self.run_cli("stats")["new"], 6)
        self.run_cli("skip-easy", "--max-rank", "1000")
        left = {c["front"] for c in self.run_cli("list")["cards"] if not c["suspended"]}
        self.assertEqual(left, {"kompleks", "hus og kompleks"})
        self.assertEqual(self.run_cli("next")["counts"]["new"], 2)
        self.assertEqual(self.run_cli("unsuspend")["restored"], 4)

    def test_new_anki_media_list_is_parsed(self):
        from compression import zstd
        def entry(name):
            n = name.encode()
            body = b"\x0a" + bytes([len(n)]) + n + b"\x10\x05"
            return b"\x0a" + bytes([len(body)]) + body
        raw = zstd.compress(entry("a.mp3") + entry("b.mp3"))
        self.assertEqual(fc._media_names(raw), {"0": "a.mp3", "1": "b.mp3"})

    def test_import_garbage_apkg_reports_error(self):
        path = os.path.join(self.tmp.name, "bad.apkg")
        open(path, "w").write("nope")
        self.assertIn("not a valid", self.run_cli("import", path, ok=False)["error"])

    def test_starter_deck_imports_cleanly(self):
        path = os.path.join(ROOT, "decks", "norwegian-starter.tsv")
        out = self.run_cli("import", path, "--no-prefetch")
        self.assertGreaterEqual(out["added"], 100)
        self.assertEqual(out["skipped"], 0)


if __name__ == "__main__":
    unittest.main()
