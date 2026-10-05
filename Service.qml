import QtQuick
import Quickshell
import Quickshell.Io

// Talks to the `flashcards` backend. Commands run one at a time through a
// queue so an answer and the refresh that follows it never interleave.
Item {
    id: root

    property var settings: ({})

    // Upcoming cards, in study order. The first one is on screen; answering pops it at
    // once and the answer is saved in the background.
    property var stack: []
    readonly property var card: stack.length > 0 ? stack[0] : null
    property int answerSeq: 0
    property var answered: []
    property var counts: ({ "new": 0, "learning": 0, "review": 0 })
    property int due: 0
    property int total: 0
    property var nextDueIn: null
    property int newWaiting: 0
    property int extraNew: 0
    property string error: ""
    property string status: ""
    property bool loaded: false
    readonly property bool busy: cli.running || queue.length > 0
    property var decks: []
    property var files: []
    // null until the backend answers; "*" means every deck.
    property var activeDeck: null
    readonly property string configuredDeck: String(setting("deck", "Norwegian"))
    // The deck new cards are added to and imported into.
    readonly property string deck: activeDeck !== null && activeDeck !== "*" ? activeDeck : configuredDeck
    // The deck being studied; "" studies every deck.
    readonly property string studyDeck: activeDeck === null ? configuredDeck : (activeDeck === "*" ? "" : activeDeck)
    readonly property string studyTitle: studyDeck === "" ? "All decks" : studyDeck
    readonly property string lang: {
        for (var i = 0; i < decks.length; i++) if (decks[i].name === deck) return decks[i].lang;
        return String(setting("lang", "no"));
    }
    onStudyDeckChanged: { stack = []; refresh(); }
    readonly property int queueSize: intSetting("queueSize", 20, 1, 100)
    readonly property int newPerDay: intSetting("newPerDay", 20, 0, 500) + extraNew
    property var queue: []
    property var _current: null

    function setting(name, fallback) {
        var value = settings ? settings[name] : undefined;
        return value === undefined || value === null ? fallback : value;
    }

    function intSetting(name, fallback, min, max) {
        var n = parseInt(String(setting(name, fallback)), 10);
        if (!isFinite(n)) n = fallback;
        return Math.max(min, Math.min(max, n));
    }

    function path(name) {
        return decodeURIComponent(Qt.resolvedUrl(name).toString().replace(/^file:\/\//, ""));
    }

    function run(args, done, started, seq) {
        queue = queue.concat([{ args: args, done: done || null, started: started || null, seq: seq }]);
        pump();
    }

    function pump() {
        if (cli.running || queue.length === 0) return;
        _current = queue[0];
        queue = queue.slice(1);
        if (_current.started) _current.started();
        cli.command = ["python3", path("flashcards")].concat(_current.args);
        cli.running = true;
    }

    function finish(stdout, stderr) {
        var job = _current;
        _current = null;
        var parsed = null;
        try { parsed = JSON.parse(stdout); } catch (e) { parsed = null; }
        if (parsed === null) {
            error = String(stderr || stdout || "Backend failed").trim().split("\n").slice(-1)[0];
        } else if (parsed.ok === false) {
            error = String(parsed.error || "Failed");
        } else {
            error = "";
        }
        if (job && job.done) job.done(parsed);
        pump();
    }

    function refresh() {
        // A pending refresh queued after the latest answer already covers this one.
        for (var i = 0; i < queue.length; i++) if (queue[i].args[0] === "next" && queue[i].seq === answerSeq) return;
        // Answers queued before this job are saved when it runs. Answers made later are not,
        // so applyQueue must drop those cards from the result.
        var seq = answerSeq;
        var args = ["next", "--new-per-day", String(newPerDay), "--count", String(queueSize)];
        if (studyDeck !== "") args = args.concat(["--deck", studyDeck]);
        run(args, function(out) {
            if (!out || out.ok === false) return;
            applyQueue(out.cards || [], seq);
            counts = out.counts;
            due = out.due;
            total = out.total;
            nextDueIn = out.next_due_in;
            newWaiting = out.new_waiting;
            loaded = true;
        }, null, seq);
    }

    // Keep the card on screen first, drop cards answered while this request ran, and
    // never reorder what the user is looking at.
    function applyQueue(cards, startSeq) {
        var skip = {};
        for (var i = 0; i < answered.length; i++) if (answered[i].seq > startSeq) skip[answered[i].id] = true;
        var current = stack.length > 0 && !skip[stack[0].id] ? stack[0] : null;
        var fresh = cards.filter(function(c) { return !skip[c.id] && (!current || c.id !== current.id); });
        stack = current ? [current].concat(fresh) : fresh;
        answered = answered.filter(function(a) { return a.seq > startSeq; });
    }

    function answer(rating) {
        if (!card) return;
        answerSeq += 1;
        answered = answered.concat([{ id: card.id, seq: answerSeq }]);
        run(["answer", String(card.id), String(rating)]);
        stack = stack.slice(1);
        refresh();
    }

    function loadDecks() {
        run(["decks", "--new-per-day", String(newPerDay)], function(out) {
            if (!out || out.ok === false) return;
            decks = out.decks;
            activeDeck = out.active;
        });
    }

    function findFiles() {
        run(["find-files"], function(out) { if (out && out.files) files = out.files; });
    }

    function useDeck(name) {
        run(["use-deck", name], function(out) { if (out && out.ok) loadDecks(); });
    }

    function deleteDeck(name) {
        run(["delete-deck", name], function(out) {
            if (out && out.ok) { flash("Deleted " + name + " (" + out.deleted + " cards)"); loadDecks(); refresh(); }
        });
    }

    function importFile(path, deckName, done) {
        flash("Importing… large decks take a few seconds");
        run(["import", path, "--deck", deckName, "--lang", lang], function(out) {
            if (out && out.ok) flash("Imported " + out.added + " cards, skipped " + out.skipped + " duplicates");
            loadDecks();
            refresh();
            if (done) done(out && out.ok === true);
        });
    }

    function flash(text) {
        status = text;
        statusTimer.restart();
    }

    function studyMore() {
        extraNew += 10;
        refresh();
    }

    function undo() {
        run(["undo"], function(out) { status = out && out.ok ? "Undid the last answer" : ""; statusTimer.restart(); });
        refresh();
    }

    function addCard(front, back, note, done) {
        run(["add", front, back, "--note", note, "--deck", deck, "--lang", lang], function(out) {
            if (out && out.ok) { status = "Added " + front; statusTimer.restart(); refresh(); }
            if (done) done(out && out.ok === true);
        });
    }

    function importStarter() {
        status = "Importing…";
        run(["import", path("decks/norwegian-starter.tsv"), "--deck", deck, "--lang", lang], function(out) {
            status = out && out.ok ? "Imported " + out.added + " words" : "";
            statusTimer.restart();
            refresh();
        });
    }

    function play() {
        if (!card || audio.running) return;
        audio.command = ["python3", path("flashcards"), "audio", String(card.id), "--play"];
        audio.running = true;
    }

    function dueText() {
        if (nextDueIn === null) return "";
        var s = nextDueIn;
        if (s < 3600) return Math.max(1, Math.round(s / 60)) + " min";
        if (s < 86400) return Math.round(s / 3600) + " h";
        return Math.round(s / 86400) + " d";
    }

    Component.onCompleted: loadDecks()

    Timer {
        interval: 60000
        repeat: true
        running: true
        triggeredOnStart: true
        onTriggered: root.refresh()
    }

    Timer {
        id: statusTimer
        interval: 6000
        onTriggered: root.status = ""
    }

    Process {
        id: cli
        stdout: StdioCollector { id: cliOut; waitForEnd: true }
        stderr: StdioCollector { id: cliErr; waitForEnd: true }
        onExited: root.finish(cliOut.text, cliErr.text)
    }

    Process {
        id: audio
        stdout: StdioCollector { id: audioOut; waitForEnd: true }
        onExited: {
            var parsed = null;
            try { parsed = JSON.parse(audioOut.text); } catch (e) {}
            if (parsed && parsed.ok === false) root.error = parsed.error;
        }
    }
}
