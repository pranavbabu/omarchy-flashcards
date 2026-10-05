import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Quickshell
import Quickshell.Io
import qs.Commons
import qs.Ui

Panel {
  id: root
  moduleName: "pranavbabu.flashcards"
  ipcTarget: "pranavbabu.flashcards"
  manageIpc: false

  // Glyphs are built from codepoints so no private-use characters live in this file.
  readonly property string glyphIcon: String.fromCodePoint(0xF05CA)
  readonly property string glyphSound: String.fromCodePoint(0xF057E)
  readonly property string glyphAdd: String.fromCodePoint(0xF0415)
  readonly property string glyphUndo: String.fromCodePoint(0xF054C)
  readonly property string glyphTrash: String.fromCodePoint(0xF01B4)
  property string confirmDelete: ""

  readonly property color foreground: bar ? bar.foreground : Color.foreground
  readonly property color urgent: bar ? bar.urgent : Color.urgent
  readonly property color dim: Qt.darker(foreground, 1.55)
  readonly property string fontFamily: bar ? bar.fontFamily : Style.font.family

  property string mode: "review"
  property bool revealed: false
  readonly property var card: svc.card
  readonly property int cardId: card ? card.id : -1
  readonly property string barLabelMode: String(setting("barLabel", "Word"))
  readonly property int popupEveryMinutes: Math.max(0, parseInt(String(setting("popupEveryMinutes", 0)), 10) || 0)
  readonly property var ratings: [
    { value: 1, label: "Fail", key: "1" },
    { value: 2, label: "Hard", key: "2" },
    { value: 3, label: "Pass", key: "3" },
    { value: 4, label: "Easy", key: "4" }
  ]

  function barText() {
    var label = glyphIcon;
    if (svc.due === 0 || barLabelMode === "Icon") return label;
    if (barLabelMode === "Count") return label + " " + svc.due;
    var word = card ? String(card.front) : "";
    if (word.length > 18) word = word.substring(0, 17) + "…";
    return label + " " + word;
  }

  readonly property bool pictureQuestion: card !== null && card.image_mode === "prompt"
  readonly property bool imageShown: card !== null && card.image !== "" && (card.image_mode !== "answer" || revealed)

  // Picture-only cards hide the word, so their audio waits for the reveal.
  function autoplayNow() {
    if (opened && mode === "review" && cardId >= 0 && !pictureQuestion && Boolean(setting("autoplay", true))) svc.play();
  }

  function reveal() {
    revealed = true;
    if (pictureQuestion && Boolean(setting("autoplay", true))) svc.play();
  }

  function rate(value) {
    if (!card) return;
    svc.answer(value);
    revealed = false;
  }

  function activate() {
    if (mode !== "review" || !card) return;
    if (!revealed) reveal();
    else rate(3);
  }

  function submitCard() {
    var front = frontField.text.trim();
    var back = backField.text.trim();
    if (front === "" || back === "") return;
    svc.addCard(front, back, noteField.text.trim(), function(ok) {
      if (!ok) return;
      frontField.text = "";
      backField.text = "";
      noteField.text = "";
      frontField.forceActiveFocus();
    });
  }

  function startImport() {
    var path = pathField.text.trim();
    var deckName = deckField.text.trim();
    if (path === "" || deckName === "" || svc.busy) return;
    svc.importFile(path, deckName, function(ok) { if (ok) pathField.text = ""; });
  }

  function showMode(next) {
    mode = next;
    confirmDelete = "";
    if (next === "add") Qt.callLater(function() { frontField.forceActiveFocus(); });
    else Qt.callLater(function() { keyCatcher.forceActiveFocus(); });
    if (next === "decks") { svc.loadDecks(); svc.findFiles(); deckField.text = svc.deck; }
    if (next === "review") { revealed = false; svc.refresh(); }
  }

  onCardIdChanged: {
    revealed = false;
    autoplayNow();
  }

  onOpenedChanged: if (opened) {
    mode = "review";
    revealed = false;
    svc.refresh();
    Qt.callLater(function() { keyCatcher.forceActiveFocus(); });
    autoplayNow();
  }

  implicitWidth: button.implicitWidth
  implicitHeight: button.implicitHeight

  Service {
    id: svc
    settings: root.settings
  }

  Timer {
    interval: Math.max(1, root.popupEveryMinutes) * 60000
    repeat: true
    running: root.popupEveryMinutes > 0
    onTriggered: if (svc.due > 0 && !root.opened) root.open()
  }

  IpcHandler {
    target: root.ipcTarget
    function open(): void { root.open() }
    function close(): void { root.close() }
    function toggle(): void { root.toggle() }
    function refresh(): string { svc.refresh(); return "ok" }
    function status(): string { return svc.due + " due, " + svc.total + " total" }
  }

  WidgetButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    text: root.barText()
    dimmed: svc.due === 0
    active: root.opened
    onPressed: function(buttonCode) {
      if (buttonCode === Qt.RightButton) svc.play();
      else root.toggle();
    }
  }

  KeyboardPanel {
    id: panel
    anchorItem: button
    owner: root
    bar: root.bar
    open: root.opened
    focusTarget: keyCatcher
    contentWidth: panel.fittedContentWidth(Style.space(380))
    contentHeight: panel.fittedContentHeight(column.implicitHeight, Style.space(560))

    PanelKeyCatcher {
      id: keyCatcher
      anchors.fill: parent
      blocked: (root.mode === "add" || root.mode === "decks") && (frontField.activeFocus || backField.activeFocus || noteField.activeFocus || pathField.activeFocus || deckField.activeFocus)
      onActivateRequested: root.activate()
      onCloseRequested: root.close()
      onTextKey: function(t) {
        var key = t.toLowerCase();
        if (key === "1" || key === "f") root.rate(1);
        else if (key === "2") root.rate(2);
        else if (key === "3" || key === "p") root.rate(3);
        else if (key === "4" || key === "e") root.rate(4);
        else if (key === "s") svc.play();
        else if (key === "u") svc.undo();
        else if (key === "a") root.showMode("add");
        else if (key === "d") root.showMode("decks");
        else if (t === "[") svc.cycleDeck(-1);
        else if (t === "]") svc.cycleDeck(1);
        else if (key === "r") root.showMode("review");
      }

      Column {
        id: column
        width: parent.width
        spacing: Style.space(12)

        RowLayout {
          width: parent.width
          spacing: Style.space(8)

          Button {
            iconText: String.fromCodePoint(0xF0141)
            tooltipText: "Previous deck  [ [ ]"
            foreground: root.foreground
            fontFamily: root.fontFamily
            onClicked: svc.cycleDeck(-1)
          }
          Button {
            text: svc.studyTitle
            tooltipText: "Next deck  [ ] ]"
            foreground: root.foreground
            fontFamily: root.fontFamily
            fontSize: Style.font.title
            onClicked: svc.cycleDeck(1)
          }
          Button {
            iconText: String.fromCodePoint(0xF0142)
            tooltipText: "Next deck  [ ] ]"
            foreground: root.foreground
            fontFamily: root.fontFamily
            onClicked: svc.cycleDeck(1)
          }
          Item { Layout.fillWidth: true }
          Text {
            text: svc.counts["new"] + " new"
            color: svc.counts["new"] > 0 ? root.foreground : root.dim
            font.family: root.fontFamily
            font.pixelSize: Style.font.bodySmall
          }
          Text {
            text: svc.counts["learning"] + " learning"
            color: svc.counts["learning"] > 0 ? root.urgent : root.dim
            font.family: root.fontFamily
            font.pixelSize: Style.font.bodySmall
          }
          Text {
            text: svc.counts["review"] + " review"
            color: svc.counts["review"] > 0 ? root.foreground : root.dim
            font.family: root.fontFamily
            font.pixelSize: Style.font.bodySmall
          }
        }

        RowLayout {
          width: parent.width
          spacing: Style.space(6)
          Button {
            text: "Review"
            bordered: true
            selected: root.mode === "review"
            foreground: root.foreground
            fontFamily: root.fontFamily
            Layout.fillWidth: true
            onClicked: root.showMode("review")
          }
          Button {
            text: "Decks"
            bordered: true
            selected: root.mode === "decks"
            foreground: root.foreground
            fontFamily: root.fontFamily
            Layout.fillWidth: true
            onClicked: root.showMode("decks")
          }
          Button {
            text: "Add  [a]"
            iconText: root.glyphAdd
            bordered: true
            selected: root.mode === "add"
            foreground: root.foreground
            fontFamily: root.fontFamily
            Layout.fillWidth: true
            onClicked: root.showMode("add")
          }
        }

        PanelSeparator { foreground: root.foreground }

        // --- decks ---
        Column {
          visible: root.mode === "decks"
          width: parent.width
          spacing: Style.space(8)

          PanelSectionHeader { text: "DECKS"; foreground: root.foreground; fontFamily: root.fontFamily }

          Repeater {
            model: [{ name: "*", total: 0, new: 0, learning: 0, review: 0 }].concat(svc.decks)
            Column {
              id: deckRow
              required property var modelData
              width: column.width
              spacing: Style.space(4)
              readonly property bool all: modelData.name === "*"
              readonly property int dueCount: modelData.new + modelData.learning + modelData.review
              RowLayout {
                width: parent.width
                spacing: Style.space(6)
                Button {
                  Layout.fillWidth: true
                  bordered: true
                  leftAlign: true
                  selected: svc.studyDeck === (deckRow.all ? "" : deckRow.modelData.name)
                  text: (deckRow.all ? "All decks" : deckRow.modelData.name) + (deckRow.all ? "" : "   " + deckRow.modelData.total + " cards, " + deckRow.dueCount + " due")
                  foreground: root.foreground
                  fontFamily: root.fontFamily
                  onClicked: { svc.useDeck(deckRow.modelData.name); root.showMode("review"); }
                }
                PanelActionButton {
                  visible: !deckRow.all
                  iconText: root.glyphTrash
                  tooltipText: "Delete deck"
                  foreground: root.foreground
                  hoverColor: root.urgent
                  fontFamily: root.fontFamily
                  onClicked: root.confirmDelete = deckRow.modelData.name
                }
              }
              RowLayout {
                visible: root.confirmDelete === deckRow.modelData.name
                width: parent.width
                spacing: Style.space(6)
                Text {
                  Layout.fillWidth: true
                  text: "Delete " + deckRow.modelData.total + " cards and their history?"
                  color: root.urgent
                  font.family: root.fontFamily
                  font.pixelSize: Style.font.bodySmall
                  wrapMode: Text.WordWrap
                }
                Button { text: "Delete"; bordered: true; foreground: root.urgent; fontFamily: root.fontFamily
                  onClicked: { svc.deleteDeck(deckRow.modelData.name); root.confirmDelete = ""; } }
                Button { text: "Keep"; bordered: true; foreground: root.foreground; fontFamily: root.fontFamily
                  onClicked: root.confirmDelete = "" }
              }
            }
          }

          PanelSeparator { foreground: root.foreground }
          PanelSectionHeader { text: "IMPORT"; foreground: root.foreground; fontFamily: root.fontFamily }

          Text {
            visible: svc.files.length > 0
            width: parent.width
            text: "In Downloads (click to choose):"
            color: root.dim
            font.family: root.fontFamily
            font.pixelSize: Style.font.caption
          }
          Repeater {
            model: svc.files
            Button {
              required property var modelData
              width: column.width
              bordered: true
              leftAlign: true
              text: modelData.name + "  (" + modelData.mb + " MB)"
              foreground: root.foreground
              fontFamily: root.fontFamily
              onClicked: { pathField.text = modelData.path; deckField.text = modelData.name.replace(/\.[^.]+$/, "").replace(/_+/g, " "); }
            }
          }
          TextField {
            id: pathField
            width: parent.width
            placeholderText: "Path to an Anki .apkg or a .tsv/.csv file"
            foreground: root.foreground
            font.family: root.fontFamily
            KeyNavigation.tab: deckField
            onAccepted: deckField.forceActiveFocus()
            Keys.onEscapePressed: root.showMode("review")
          }
          TextField {
            id: deckField
            width: parent.width
            placeholderText: "Import into deck (new name creates a deck)"
            foreground: root.foreground
            font.family: root.fontFamily
            KeyNavigation.tab: pathField
            onAccepted: root.startImport()
            Keys.onEscapePressed: root.showMode("review")
          }
          Button {
            width: parent.width
            text: "Import"
            bordered: true
            enabled: !svc.busy && pathField.text.trim() !== "" && deckField.text.trim() !== ""
            foreground: root.foreground
            fontFamily: root.fontFamily
            onClicked: root.startImport()
          }
        }

        // --- review ---
        Column {
          visible: root.mode === "review" && root.card !== null
          width: parent.width
          spacing: Style.space(10)

          Item {
            visible: !root.pictureQuestion || root.revealed
            width: parent.width
            height: visible ? frontRow.implicitHeight : 0
            RowLayout {
              id: frontRow
              anchors.horizontalCenter: parent.horizontalCenter
              width: Math.min(parent.width, implicitWidth)
              spacing: Style.space(10)
              Text {
                Layout.fillWidth: true
                text: root.card ? root.card.front : ""
                color: root.foreground
                font.family: root.fontFamily
                font.pixelSize: Style.font.displayLarge
                font.bold: true
                horizontalAlignment: Text.AlignHCenter
                wrapMode: Text.WordWrap
              }
              PanelActionButton {
                iconText: root.glyphSound
                tooltipText: "Play pronunciation  [s]"
                foreground: root.foreground
                fontFamily: root.fontFamily
                focusable: false
                onClicked: svc.play()
              }
            }
          }

          Image {
            visible: root.imageShown && status !== Image.Error
            width: parent.width
            height: visible ? Style.space(170) : 0
            source: root.card && root.card.image !== "" ? "file://" + encodeURI(root.card.image) : ""
            fillMode: Image.PreserveAspectFit
            sourceSize.width: 800
            asynchronous: true
          }

          Text {
            visible: root.revealed && root.card && root.card.back !== ""
            width: parent.width
            text: root.card ? root.card.back : ""
            color: root.foreground
            font.family: root.fontFamily
            font.pixelSize: Style.font.heading
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.WordWrap
          }

          Text {
            visible: root.revealed && root.card && root.card.note !== ""
            width: parent.width
            text: root.card ? root.card.note : ""
            color: root.dim
            font.family: root.fontFamily
            font.pixelSize: Style.font.bodySmall
            font.italic: true
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.WordWrap
          }

          Button {
            visible: !root.revealed
            width: parent.width
            text: "Show answer  [space]"
            bordered: true
            foreground: root.foreground
            fontFamily: root.fontFamily
            onClicked: root.reveal()
          }

          RowLayout {
            width: parent.width
            spacing: Style.space(6)
            Repeater {
              model: root.ratings
              Button {
                required property var modelData
                Layout.fillWidth: true
                Layout.preferredWidth: 1
                bordered: true
                text: modelData.label + "  [" + modelData.key + "]\n" + (root.card ? root.card.preview[String(modelData.value)] : "")
                foreground: modelData.value === 1 ? root.urgent : root.foreground
                fontFamily: root.fontFamily
                onClicked: root.rate(modelData.value)
              }
            }
          }
        }

        Column {
          visible: root.mode === "review" && root.card === null && svc.loaded && !svc.busy
          width: parent.width
          spacing: Style.space(10)

          Text {
            width: parent.width
            text: svc.total === 0 ? "No words yet." : (svc.newWaiting > 0 && svc.counts["learning"] === 0 && svc.counts["review"] === 0 ? "Today's new-card limit is reached." : "Nothing due right now.")
            color: root.foreground
            font.family: root.fontFamily
            font.pixelSize: Style.font.heading
            horizontalAlignment: Text.AlignHCenter
          }
          Text {
            visible: svc.total > 0 && svc.dueText() !== ""
            width: parent.width
            text: "Next card in " + svc.dueText() + "."
            color: root.dim
            font.family: root.fontFamily
            font.pixelSize: Style.font.body
            horizontalAlignment: Text.AlignHCenter
          }
          Button {
            visible: svc.newWaiting > 0 && svc.due === 0
            width: parent.width
            text: "Study 10 more new cards  (" + svc.newWaiting + " waiting)"
            bordered: true
            foreground: root.foreground
            fontFamily: root.fontFamily
            onClicked: svc.studyMore()
          }
          Button {
            visible: svc.total === 0
            width: parent.width
            text: "Import starter Norwegian words"
            bordered: true
            foreground: root.foreground
            fontFamily: root.fontFamily
            onClicked: svc.importStarter()
          }
        }

        // --- add ---
        Column {
          visible: root.mode === "add"
          width: parent.width
          spacing: Style.space(8)

          Text {
            width: parent.width
            text: "Adds to deck: " + svc.deck
            color: root.dim
            font.family: root.fontFamily
            font.pixelSize: Style.font.bodySmall
          }
          TextField {
            id: frontField
            width: parent.width
            placeholderText: "Word (" + svc.lang + ")"
            foreground: root.foreground
            font.family: root.fontFamily
            KeyNavigation.tab: backField
            onAccepted: backField.forceActiveFocus()
            Keys.onEscapePressed: root.showMode("review")
          }
          TextField {
            id: backField
            width: parent.width
            placeholderText: "Translation"
            foreground: root.foreground
            font.family: root.fontFamily
            KeyNavigation.tab: noteField
            onAccepted: noteField.forceActiveFocus()
            Keys.onEscapePressed: root.showMode("review")
          }
          TextField {
            id: noteField
            width: parent.width
            placeholderText: "Example sentence (optional)"
            foreground: root.foreground
            font.family: root.fontFamily
            KeyNavigation.tab: frontField
            onAccepted: root.submitCard()
            Keys.onEscapePressed: root.showMode("review")
          }
          Button {
            width: parent.width
            text: "Add card  [enter]"
            iconText: root.glyphAdd
            bordered: true
            foreground: root.foreground
            fontFamily: root.fontFamily
            onClicked: root.submitCard()
          }
          Text {
            width: parent.width
            text: "Pronunciation audio is fetched automatically. Bulk import: flashcards import words.tsv"
            color: root.dim
            font.family: root.fontFamily
            font.pixelSize: Style.font.caption
            wrapMode: Text.WordWrap
          }
        }

        Text {
          visible: svc.status !== "" || svc.error !== ""
          width: parent.width
          text: svc.error !== "" ? svc.error : svc.status
          color: svc.error !== "" ? root.urgent : root.dim
          font.family: root.fontFamily
          font.pixelSize: Style.font.bodySmall
          wrapMode: Text.WordWrap
        }

        Text {
          visible: root.mode === "review"
          width: parent.width
          text: "space show/pass  1-4 rate (f fail, p pass, e easy)  s sound  u undo  a add  d decks  [ ] switch deck  esc close"
          color: root.dim
          font.family: root.fontFamily
          font.pixelSize: Style.font.caption
          horizontalAlignment: Text.AlignHCenter
        }
      }
    }
  }
}
