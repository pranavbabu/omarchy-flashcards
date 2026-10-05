import QtQuick
import Quickshell
import Quickshell.Io

// Talks to the `flashcards` backend. Commands run one at a time through a
// queue so an answer and the refresh that follows it never interleave.
Item {
    id: root

    property var settings: ({})

    property var card: null
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
    readonly property string deck: String(setting("deck", "Norwegian"))
    readonly property string lang: String(setting("lang", "no"))
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

    function run(args, done) {
        queue = queue.concat([{ args: args, done: done || null }]);
        pump();
    }

    function pump() {
        if (cli.running || queue.length === 0) return;
        _current = queue[0];
        queue = queue.slice(1);
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
        // A pending refresh already covers this one.
        for (var i = 0; i < queue.length; i++) if (queue[i].args[0] === "next") return;
        run(["next", "--deck", deck, "--new-per-day", String(newPerDay)], function(out) {
            if (!out || out.ok === false) return;
            card = out.card;
            counts = out.counts;
            due = out.due;
            total = out.total;
            nextDueIn = out.next_due_in;
            newWaiting = out.new_waiting;
            loaded = true;
        });
    }

    function answer(rating) {
        if (!card) return;
        run(["answer", String(card.id), String(rating)]);
        card = null;
        refresh();
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

    Timer {
        interval: 60000
        repeat: true
        running: true
        triggeredOnStart: true
        onTriggered: root.refresh()
    }

    Timer {
        id: statusTimer
        interval: 2500
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
