[English](README.md) · [简体中文](README.zh-CN.md) · [Español](README.es.md) · [Português (BR)](README.pt-BR.md) · [Русский](README.ru.md) · [日本語](README.ja.md) · [Français](README.fr.md) · **Deutsch** · [Türkçe](README.tr.md)

> Dies ist eine Übersetzung der englischen README. Bei Abweichungen ist die
> [englische Fassung](README.md) maßgeblich.

# maskrun

Befehle mit Secrets aus dem Schlüsselbund deines Betriebssystems ausführen —
und die Werte aus dem Kontext deines KI-Coding-Agents heraushalten.

```bash
maskrun put myapp-database-url        # im Schlüsselbund gespeichert, nie auf der Platte
maskrun run -- npm run dev            # in den Kindprozess injiziert, in der Ausgabe maskiert
```

```
$ maskrun run -- node -e 'console.log(process.env.DATABASE_URL)'
<masked:DATABASE_URL>
```

Eine Binary, keine Laufzeitabhängigkeiten, kein Daemon. Linux, macOS und
Windows.

---

## Das Problem

Secrets aus `.env`-Dateien in den Schlüsselbund zu verschieben, ist die
leichte Hälfte. Die schwere Hälfte zeigt sich, sobald ein KI-Agent deine Shell
bedient.

Ein Secret in einen Kindprozess zu injizieren, hindert den Agent daran, *den
Tresor zu lesen*. Gegen den Wert, der *aus dem Prozess zurückkommt*, tut es
nichts:

- ein Dev-Server gibt beim Start seinen Connection String aus
- `curl -v` gibt den `Authorization`-Header wieder
- ein Stacktrace trägt den DSN mit sich
- `psql` zitiert die URL, zu der es keine Verbindung aufbauen konnte

Jedes davon bringt das Secret ins Transkript, wo es nun Teil des Gesprächs ist,
des Scrollbacks und all dessen, wo dieses Transkript gespeichert wird.

maskrun schließt diesen Weg und die daneben.

## Installation

**Linux / macOS** — lädt eine vorgebaute Binary, prüft ihre Checksumme, kein
Compiler nötig:

```bash
curl -fsSL https://raw.githubusercontent.com/furkanakyol/maskrun/main/install.sh | sh
```

**Windows (PowerShell)**

```powershell
irm https://raw.githubusercontent.com/furkanakyol/maskrun/main/install.ps1 | iex
```

**Mit einer Rust-Toolchain:**

```bash
cargo install maskrun
```

**Aus dem Quellcode:**

```bash
git clone https://github.com/furkanakyol/maskrun
cd maskrun && cargo build --release
# Binary unter target/release/maskrun
```

## Schnellstart

```bash
# 1. eine vorhandene .env in den Schlüsselbund holen — erst schauen, dann springen
maskrun import .env --dry-run
maskrun import .env

# 2. prüfen, dass jeder Name aus dem Manifest tatsächlich vorhanden ist
maskrun status

# 3. deine App ohne .env auf der Platte starten
maskrun run -- npm run dev

# erst jetzt die .env löschen
```

`import` schreibt ein `.maskrun`-Manifest neben deinen Code:

```
DATABASE_URL=myapp-database-url
JWT_SECRET=myapp-jwt-secret
```

**Diese Datei enthält Namen, keine Werte. Committe sie.** Sie ist das
`.env.example`, das tatsächlich etwas tut: `maskrun status` sagt einem neuen
Teammitglied genau, welche Secrets ihm fehlen.

## Wie es funktioniert

Drei Mechanismen für drei verschiedene Bedürfnisse.

| Bedarf | Mechanismus | Was der Agent sieht |
|---|---|---|
| Ein Dev-Server, eine Migration oder ein CLI, das `DATABASE_URL` braucht | `maskrun run` injiziert in den Kindprozess | maskierte Ausgabe (`<masked:VAR>`) |
| Der Wert selbst — einen Key rotieren, in ein Dashboard einfügen | du, in deinem eigenen Terminal | nichts; die Bremse verweigert |
| Wissen, *welche* Secrets existieren | `maskrun list`, `maskrun status` | nur Namen, nie Werte |

### Injektion, kein Lesen

Die Werte leben im Schlüsselbund. `maskrun run` löst das Manifest auf, übergibt
die Werte an genau einen Kindprozess und legt nichts auf der Platte ab. Fehlt
ein Secret, verweigert es den Start, statt deine App mit einer halben Umgebung
hochzufahren.

### Ausgabemaskierung

`run` und `exec` leiten stdout und stderr des Kindprozesses durch einen Filter,
der Secret-Werte durch `<masked:VAR>` ersetzt.

- Werte erreichen den Filter über **seine Umgebung**, nie über argv — argv ist
  über `/proc/<pid>/cmdline` lesbar.
- Maskiert den Rohwert sowie seine Base64-, URL-kodierte und
  Backslash-escapte Schreibweise.
- Erwischt einen Wert, der **über zwei Schreibvorgänge verteilt** ist, sodass
  ein Secret an einer Flush-Grenze nicht durchrutscht.
- Reicht vollständige Zeilen sofort durch, damit die Ausgabe eines Dev-Servers
  nicht gepuffert wird, während du zusiehst.
- Verweigert es, Werte unter 6 Zeichen zu maskieren, und sagt das auch: `5432`
  zu maskieren würde jede unbeteiligte Zahl in der Ausgabe verfälschen.
- Exit-Code, stdin und Ctrl-C erreichen den Kindprozess unverändert.

Standardmäßig aktiv in einer KI-Agent-Sitzung und immer dann, wenn stdout kein
Terminal ist; aus in deinem eigenen interaktiven Terminal, damit Farben
erhalten bleiben. `--raw` erzwingt aus, `--mask` erzwingt an.

### Die Agent-Bremse

```bash
maskrun install-guard        # registriert einen PreToolUse-Hook für Claude Code
```

Der Sinn eines Hooks ist, dass **das Harness ihn durchsetzt, nicht das Modell**.
Eine in einen Prompt geschriebene Regel wird vom Modell durchgesetzt, also kann
sich das Modell aus ihr herausreden. Hier geht das nicht.

Er verweigert die Befehle, die einen Wert ins Transkript brächten:

- `maskrun get`, `maskrun import`
- `secret-tool lookup/search`, `security find-generic-password`
- `--raw`, `MASKRUN_MASK=0`, `MASKRUN_ALLOW_READ=1` (Maskierung abschalten)
- `/proc/<pid>/environ`
- ein nacktes `env` / `printenv`, `Get-ChildItem Env:`
- das Lesen einer `.env` / `.envrc`, ob über Bash oder über die Dateiwerkzeuge
  des Agents

Und er lässt normale Arbeit in Ruhe: `maskrun run`, `env VAR=x cmd`,
`cat .env.example`, `cat .maskrun`, `printenv PATH`, `ls -la .env`, `rm .env`.

`maskrun install-guard` fügt sich in deine vorhandene `settings.json` ein, legt
vorher eine Sicherung an, ist idempotent, verweigert den Zugriff auf eine
Datei, die kein gültiges JSON ist, und `--remove` macht es rückgängig. Andere
Harnesses: `maskrun hook` als Pre-Tool-Hook ausführen, der den Tool-Aufruf als
JSON auf stdin bekommt — siehe
[integrations/claude-code](integrations/claude-code/).

Das CLI setzt dieselben Verweigerungen selbst durch, sodass ein Agent in einem
Harness ohne Hook-Unterstützung trotzdem kein `maskrun get` ausführen kann.

### Einen Agent ohne Hook-Mechanismus anleiten

```bash
maskrun install-rules        # schreibt einen kurzen Block in AGENTS.md/CLAUDE.md/.cursor-Regeln
```

`install-guard` funktioniert nur dort, wo das Harness für dich einen
PreToolUse-Hook ausführt. Anderswo setzt nichts irgendetwas durch — also
schreibt `install-rules` einen kurzen, markierten Block in diejenige von
`AGENTS.md`, `CLAUDE.md` oder `.cursor/rules/`, die im Projekt bereits
existiert (und legt `AGENTS.md` an, wenn keine existiert), und erklärt dem
Agent, wie maskrun hier zu benutzen ist. **Das ist Anleitung, keine
Durchsetzung**: Ein Modell kann sie ignorieren, so wie es jede andere Anweisung
ignorieren kann. Es ist ein Rückfallweg für Harnesses, die `install-guard`
nicht erreicht, kein Ersatz dafür.

Der Block wird von den Markierungen
`<!-- maskrun:start -->`/`<!-- maskrun:end -->` eingefasst, sichert die Datei
vorher, ist idempotent (ein zweiter Lauf aktualisiert ihn an Ort und Stelle,
statt ihn zu duplizieren), und `--remove` nimmt ihn wieder heraus, ohne den
Rest der Datei anzurühren. Liegt ein `.maskrun`-Manifest vor, listet der Block
die tatsächlichen Variablennamen auf, die das Projekt erwartet; `--file <path>`
zielt direkt auf eine Datei und überspringt die Suche.

<a id="interactive-view"></a>

### Die interaktive Ansicht

```bash
maskrun            # in deinem eigenen Terminal, ohne Argumente
```

Ein echtes Terminal mit sonst nichts auf der Kommandozeile öffnet eine mit den
Pfeiltasten bedienbare Ansicht: links die Plattformen, rechts die Secrets
dieser Plattform und die Details des markierten (Plattform, Notiz, Wert).

```
↑↓ move   → enter   ← back   e edit   d delete   v reveal   y copy   q quit
```

Werte sind maskiert (`••••••••••••••••`), bis du `v` drückst, und ein Wert wird
immer erst in diesem Moment aus dem Schlüsselbund gelesen — durch die Liste zu
navigieren rührt ihn nie an. Zu einem anderen Secret zu wechseln maskiert
automatisch wieder. `e` bearbeitet Plattform, Notiz und Wert an Ort und Stelle
(leer lassen behält den aktuellen); `d` verlangt eine Bestätigung mit dem Namen
(`delete 'name'? [y/N]`) und nur ein buchstäbliches `y`/`Y` fährt fort — jede
andere Taste, Enter eingeschlossen, bricht ab. `y` kopiert den Wert in deine
Zwischenablage.

Sie läuft in einem [alternativen
Bildschirmpuffer](https://en.wikipedia.org/wiki/Terminal_emulator#Alternate_screen_buffer):
nichts, was sie zeichnet — auch kein aufgedeckter Wert — landet je im
Scrollback deines Terminals, anders als die heutige Ausgabe von `maskrun get`.
Das ist eine echte Verbesserung, unabhängig von allem Weiteren unten.

Wie jeder andere Weg, der Werte berührt, **öffnet sie sich in einer
KI-Agent-Sitzung nie** — ob die Ausgabe gepipet ist oder nicht, eine erkannte
Agent-Sitzung bekommt immer dieselbe Übersicht nur mit Namen, die ein nacktes
`maskrun` nicht-interaktiv ausgibt. Sie verweigert sich außerdem auf einem
Terminal kleiner als 60x15 und sagt dir den Grund, bevor sie auf diese
Übersicht zurückfällt.

#### Kopieren in die Zwischenablage

`y` kopiert das aktuelle Secret in deine Zwischenablage — einen Wert
aufzudecken, den du danach nirgends einfügen kannst, verschiebt das Problem
nur: Du würdest ihn abtippen oder mit der Maus markieren, beides schlechter.
Drei eingebaute Grenzen:

- **Wird nach 45 Sekunden automatisch geleert.** Die TUI zeigt einen laufenden
  Countdown, sobald etwas kopiert wurde; `c` leert sofort, statt zu warten, und
  die TUI zu verlassen, während noch etwas kopiert ist, leert ebenfalls. Beim
  Leeren wird wiederhergestellt, was vor dem Kopieren in der Zwischenablage
  war, oder sie wird geleert, wenn dort nichts war.
- **Auf jeder Plattform geht ein Nicht-aufzeichnen-Hinweis raus**, über
  arboards `exclude_from_history`: unter Linux der MIME-Typ
  `x-kde-passwordManagerHint` von KDE, unter macOS die Community-Konvention
  `org.nspasteboard.ConcealedType` und unter Windows das native
  Zwischenablage-Format `CanIncludeInClipboardHistory`. Hier gegen ein echtes
  Klipper verifiziert: Eine normale Kopie taucht in seiner Historie auf, eine
  markierte nicht, und Klipper meldet die markierte nicht einmal als
  *aktuellen* Inhalt der Zwischenablage. Jedes davon ist ein Hinweis, den ein
  bestimmtes Werkzeug zu beachten wählt, nichts, was maskrun erzwingt — siehe
  unten.
- Sie **öffnet sich in einer Agent-Sitzung nie**, dasselbe Tor wie für den Rest
  der TUI.

**Was das nicht behebt:** Jedes Zwischenablage-Historie-Werkzeug, das nicht auf
diesen Hinweis achtet (das von GNOME, die meisten Nicht-KDE-Wayland-Setups und
— beim Bauen dieser Funktion direkt beobachtet — sogar KDEs eigenes Klipper,
wenn es nicht den Transportweg der Zwischenablage beobachtet, den dein
Compositor gerade benutzt), zeichnet den Wert weiterhin dauerhaft auf, und die
45-Sekunden-Automatik ändert nichts an dieser Kopie, sobald sie in einer
Historiendatei steht. Behandle das Kopieren in die Zwischenablage wie
`/proc/<pid>/environ` und wie einen Menschen, der einen unmaskierten Wert von
Hand einfügt, beides unten: eine echte Grenze, kein gelöstes Problem.

## Was das nicht ist

**maskrun ist keine Sicherheitsgrenze.** Es ist Härtung gegen Unfälle, und es
sollte dir — oder von dir — nicht als mehr verkauft werden.

- **Ein Schlüsselbund löst Speicherung, nicht Zugriff.** Jeder Prozess, der
  unter deinem Benutzer läuft, kann `secret-tool lookup` oder
  `security find-generic-password` aufrufen, dein Agent eingeschlossen. Die
  Bremse erhöht die Kosten, es aus Versehen zu tun; unmöglich macht sie es
  nicht. Für dich gilt dasselbe: Wenn ein Mensch einen unmaskierten Wert von
  Hand ins Transkript einfügt, kann kein Werkzeug hinter diesem Tastendruck ihn
  noch abfangen.
- **Code, den die Bremse nicht lesen kann.** Heredoc-Inhalte und Skriptdateien
  werden als Daten behandelt, nicht als Befehle — absichtlich, weil ihr Parsen
  Fehlalarme erzeugt hat. Ein Skript, das `.env` selbst aus seinem eigenen
  Quelltext heraus liest, kommt also durch. Ein Mustervergleich erwischt
  beliebigen Code nie.
- **Prozessumgebung.** Während `maskrun run` läuft, ist die Umgebung seines
  Kindprozesses über `/proc/<pid>/environ` lesbar. Die Bremse blockiert diesen
  Weg direkt, aber Umgebungsinjektion hat diese Form von Natur aus.
- **Das Kopieren in die Zwischenablage (`y` in der interaktiven Ansicht) macht
  die Automatik nicht rückgängig.** Der 45-Sekunden-Timeout leert *die
  Zwischenablage*; gegen ein Historie-Werkzeug, das den Wert schon vor Ablauf
  dauerhaft aufgezeichnet hat, tut er nichts. maskrun markiert die Kopie, damit
  KDEs Klipper sie überspringt — real und verifiziert; nicht jedes
  historienführende Werkzeug beachtet diese Markierung, und von GNOMEs
  Zwischenablage-Historie, den meisten Nicht-KDE-Wayland-Setups und dem
  Windows-Zwischenablageverlauf ist nicht bekannt, dass sie es tun. Dasselbe
  Regal wie `/proc/<pid>/environ` oben und der Mensch, der unten einen
  unmaskierten Wert von Hand einfügt: eine klar benannte Grenze, kein still
  gelöstes Problem.
- **`ps` während eines Schreibvorgangs — geschlossen.** Unter macOS erreichte
  der Wert `security` früher als CLI-Argument und war kurz über `ps` in der
  Prozessliste sichtbar. Dieser Weg ist weg: maskrun ruft jetzt direkt die
  generic-password-API von Security.framework auf, der Wert wird also nie zu
  einem argv, das das Betriebssystem irgendwem zeigen müsste.
- **Abgleich pro Pipeline-Segment, kein Shell-Parser.** Die Bremse bewertet
  jedes Segment einer Pipeline für sich, und genau das lässt
  `sed 's/maskrun get/x/' notes.md` durch — dieser Text ruft `maskrun get` nie
  auf, er erwähnt es nur. Derselbe Geltungsbereich bedeutet, dass die Bremse
  einem Befehl nicht durch Indirektion folgt: `echo 'maskrun get x' | sh` liest
  sich als ein `echo`, nicht als der Befehl, den `sh` am Ende ausführt.

Wenn du eine echte Grenze willst, muss die Shell des Agents irgendwo laufen, wo
sie den Schlüsselbund überhaupt nicht erreicht — ohne D-Bus-Session-Socket
unter Linux, in einem eigenen Benutzerkonto oder in einem Container. Dann
funktioniert maskrun aus deinem Terminal und nicht aus dem des Agents. Der
Schadensradius wird besser beim Anbieter verwaltet: ein eigener Key pro
Werkzeug, Ausgabenlimits, Rotation und kurzlebige Anmeldedaten, wo es sie gibt.

## Backends

| Plattform | Backend-Name | Speicher |
|---|---|---|
| Linux | `secret-service` | Secret Service von libsecret, direkt über D-Bus (Crate `dbus-secret-service`) — gnome-keyring, KWallet, KeePassXC |
| macOS | `keychain` | Login-Keychain über die generic-password-API von Security.framework (Crate `security-framework`) |
| Windows | `dpapi` | Windows Credential Manager (Crate `windows`, `Win32_Security_Credentials`) — der Name stammt aus Kompatibilitätsgründen für Overrides noch vom alten DPAPI-Datei-Backend; der Speicher darunter sind keine DPAPI-Dateien mehr |

Frühere Versionen riefen für jede Operation unter Linux `secret-tool` und unter
macOS `security` als Unterprozess auf und implementierten unter Windows die
DPAPI-Dateispeicherung von Hand. Alle drei gehen für `put`/`get`/`delete` jetzt
über eine Bibliotheksanbindung an die Plattform-API statt über einen
Unterprozess oder handgeschriebene Kryptografie — unter Linux ist keine
`secret-tool`-Installation nötig, und unter macOS wird der Secret-Wert nicht
mehr zu einem CLI-Argument. (Unter macOS ruft `list` weiterhin
`security dump-keychain` auf, um Namen aufzuzählen — dieser Aufruf nimmt keinen
Secret-Wert als Argument, es wird also nichts Neues offengelegt;
`security-framework` hat keinen auf den Service beschränkten Aufzählungsaufruf,
der ihn ersetzen könnte.)

Der Linux-Speicher behält das Schema bei, das
`secret-tool lookup service maskrun name <name>` erwartet (Attribute `service`
+ `name`), sodass ein von maskrun gespeichertes Secret weiterhin mit dem
Standard-CLI lesbar ist, falls du von Hand nachsehen willst — maskrun selbst
hängt nur nicht mehr davon ab, dass dieses CLI installiert ist.

### Wo Plattform-Label und Notiz liegen

| Plattform | Wo |
|---|---|
| Linux | Zwei zusätzliche Secret-Service-Attribute, `platform` und `note`, neben den bestehenden `service`/`name` — `secret-tool lookup` gleicht nur mit den Attributen ab, die du ihm gibst, also fahren diese mit, ohne irgendetwas zu beeinträchtigen, das bereits `service`+`name` liest. |
| macOS | Die Felder `kSecAttrDescription` (Plattform) und `kSecAttrComment` (Notiz) des generic password, gesetzt und gelesen über die Attributsuche und die reinen Attribut-Update-APIs von `security-framework` — der gespeicherte Wert selbst wird bei einer Neubeschriftung nie angerührt. |
| Windows | Beides kodiert in das eine Feld `CREDENTIALW.Comment`, das Credential Manager anbietet (`platform=<p><US>note=<n>`, `<US>` = U+001F): Hier gibt es keinen separaten Attributspeicher, und eine Neubeschriftung muss den gesamten Eintrag (Wert eingeschlossen) neu schreiben, weil `CredWriteW` keinen Teil-Update-Aufruf hat. |

Erkennung überschreiben mit `MASKRUN_BACKEND=secret-service|keychain|dpapi`.

### Was tatsächlich verifiziert ist

Alle drei Backends werden in der CI gegen einen echten Schlüsselbund
ausgeführt: Secret Service unter Linux, Keychain unter macOS, Credential
Manager unter Windows. Die Schlüsselbund-Tests schreiben und lesen ein echtes
Secret zurück, statt das Backend zu mocken, und ein Job ganz ohne installierten
Schlüsselbund beweist, dass die Bremse trotzdem antwortet.

Beim Plattform-Label / der Notiz ist es dieselbe Geschichte: unter Linux gegen
einen echten Secret Service ausgeführt (inklusive der
`secret-tool`/Attributschema-Kompatibilitätsprüfung), für macOS und Windows
über Ziele hinweg typgeprüft wie der restliche Plattformcode, aber tatsächlich
gegen einen echten Keychain oder Credential Manager erst ausgeführt, wenn jene
CI-Jobs laufen.

Der allererste CI-Lauf, der je startete, fand drei echte Fehler im
Plattformcode, alle in Pfaden, die ein Linux-Build nie typprüft, weil sie
`cfg`-gated sind: zwei falsch typisierte Win32-Argumente, ein
`CredEnumerateW`-Aufruf, der sowohl einen Filter als auch das
Alle-Anmeldedaten-Flag übergab (zusammen ungültig), und ein Vergleich von
`ERROR_NOT_FOUND` gegen den rohen Win32-Code statt gegen das `HRESULT`, das
tatsächlich ankommt — wodurch ein fehlendes Secret und ein leerer Speicher
beide als roher Fehler auftauchten. Der Lint-Job prüft Windows- und macOS-Ziele
jetzt von Linux aus quer, damit diese Klasse von Fehlern nie wieder einen
Plattform-Runner erreicht.

Was *nicht* abgedeckt ist: Die Tests zur Sperrbehandlung sind Opt-in
(`MASKRUN_LOCK_TESTS=1`), weil das Starten eines Wegwerf-`gnome-keyring-daemon`
auf einem echten Desktop den Benutzer auffordert, einen Schlüsselbund
anzulegen, und den Test überdauert.

## Befehle

```
maskrun put <name>                    store a secret (prompts; not echoed)
maskrun put                           fully interactive: asks name, platform, note, value
maskrun put <name> --stdin            store from stdin (for scripts)
maskrun put <name> --for X --note Y   tag it with a platform and a note while storing
maskrun label <name> --for X          tag (or retag) an existing secret's platform
maskrun label <name> --for ""         clear its platform
maskrun get <name>                    print it (refused in an agent session)
maskrun list                          grouped by platform, with notes, never values
maskrun list --plain                  flat, sorted names only, one per line (for scripts)
maskrun rm <name>                     delete (no name: pick from a numbered list)
maskrun status                        check the manifest against the keyring
maskrun run [--raw|--mask] -- CMD     run with the manifest injected
maskrun exec VAR=name -- CMD          run with explicit pairs, no manifest
maskrun -- CMD                        shorthand for run
maskrun import <.env> [--dry-run]     move a .env into the keyring
maskrun completions <fish|bash|zsh>   print a shell completion script
maskrun install-guard [--remove]      register the agent guard
maskrun hook                          the guard itself (reads JSON on stdin)
maskrun install-rules [--remove]      tell agents how to use maskrun here (guidance, not enforcement)
```

`import` überspringt die Variablen `VITE_`, `NEXT_PUBLIC_`, `PUBLIC_`,
`REACT_APP_`, `NUXT_PUBLIC_`, `EXPO_PUBLIC_` und `GATSBY_`: Sie werden in dein
Client-Bundle kompiliert und an jeden Besucher ausgeliefert, sind also
Konfiguration und keine Secrets, und sie zu verschieben bringt nichts. `--all`
setzt das außer Kraft.

### Plattform-Labels und Notizen

Sobald du mehr als eine Handvoll Secrets hast, reicht ein flacher Name nicht
mehr, um sich zu merken, wofür jedes da ist — besonders wenn eine Plattform
mehr als einen Key hat und der Unterschied im Geltungsbereich liegt, nicht im
Namen. `--for` versieht ein Secret mit einer Plattform bzw. einem Dienst;
`--note` fügt eine kurze Freitextbeschreibung hinzu:

```
$ maskrun put gh-release-token --for github --note "fine-grained, contents+plan"
stored: gh-release-token

$ maskrun label vault-token-github --for github --note "fine-grained, repo+plan"
labelled: vault-token-github

$ maskrun list
github
  gh-release-token       fine-grained, contents+plan
  vault-token-github     fine-grained, repo+plan
(no platform)
  pos-terminal-electron-api-url
```

Beide Flags sind optional und gelten für Secrets, die bisher keines von beiden
haben. `label` beschriftet ein bestehendes Secret nachträglich neu; bei beiden
Befehlen leert `--for ""` (oder `--note ""`) das jeweilige Feld, und ein ganz
weggelassenes Flag lässt den bestehenden Wert unangetastet — ein `put` über den
Wert eines Secrets verwirft sein Label nie stillschweigend. **Plattform und
Notiz sind keine Secrets**: Sie werden unmaskiert gespeichert, sind in einer
KI-Agent-Sitzung sichtbar und werden von `list` und `status` angezeigt.
Schreibe keinen Secret-Wert in `--note`; es ist auf 200 Zeichen begrenzt und
darf keinen Zeilenumbruch enthalten.

`maskrun` ohne Argumente öffnet in einem echten Terminal [die interaktive
Ansicht](#interactive-view); gepipet (`maskrun | cat`), nicht-interaktiv oder
in einer Agent-Sitzung gibt es stattdessen eine kurze Übersicht aus: den
Manifest-Status und die oben gezeigte gruppierte Secret-Liste, damit du die
Befehlsoberfläche nicht im Kopf behalten musst.

### Shell-Vervollständigung

```bash
maskrun completions fish > ~/.config/fish/completions/maskrun.fish
maskrun completions bash > ~/.local/share/bash-completion/completions/maskrun
maskrun completions zsh > ~/.zfunc/_maskrun   # dann `fpath+=~/.zfunc` vor compinit
```

Über Flags und Unterbefehle hinaus vervollständigen `maskrun get`/`rm`/`label`
echte Secret-Namen — fish tut das von Haus aus (über genau das
`maskrun list --plain`, für das dieses Flag existiert); bash/zsh bekommen nur
die statischen Vervollständigungen.

## Umgebungsvariablen

| Variable | Wirkung |
|---|---|
| `MASKRUN_MASK` | `1` immer maskieren, `0` nie maskieren |
| `MASKRUN_AGENT` | `1` dies als Agent-Sitzung behandeln, `0` als Mensch |
| `MASKRUN_BACKEND` | ein Backend erzwingen |
| `MASKRUN_ALLOW_READ` | `1` aktiviert `get`/`import` in einer Agent-Sitzung wieder |

Agent-Sitzungen werden über `CLAUDECODE`, `CLAUDE_CODE_ENTRYPOINT`, `AI_AGENT`,
`AIDER_CHAT`, `CURSOR_AGENT`, `OPENAI_CODEX`, `GEMINI_CLI` und `REPLIT_AGENT`
erkannt. Für alles andere setze `MASKRUN_AGENT=1` im Harness.

## Plattformunterstützung

- **Linux** — glibc 2.35 oder neuer (die Release-Binaries werden auf Ubuntu
  22.04 gebaut). In der CI gegen einen laufenden Secret Service getestet.
- **macOS** — Intel und Apple Silicon. In der CI gegen einen echten Keychain
  getestet.
- **Windows** — x86_64. In der CI gegen den echten Credential Manager getestet.

## Vorarbeiten

[`envchain`](https://github.com/sorah/envchain) hat Secrets schon vor Jahren in
den Keychain gelegt und in die Umgebung injiziert und ist der direkte Vorfahr
der `run`-Hälfte dieses Werkzeugs. [`direnv`](https://direnv.net/) verwaltet
Umgebungen pro Verzeichnis, [`sops`](https://github.com/getsops/sops) und
[`dotenvx`](https://github.com/dotenvx/dotenvx) verschlüsseln Secrets im
Repository, und [`aws-vault`](https://github.com/99designs/aws-vault) erledigt
den Keychain-Tanz für einen Anbieter.

Was maskrun hinzufügt, ist die dem Agent zugewandte Hälfte: die Ausgabe des
Kindprozesses maskieren und eine vom Harness durchgesetzte Bremse für die
Befehle, die einen Wert preisgeben würden. Wenn du nicht mit einem KI-Agent
arbeitest, reicht dir `envchain` vielleicht.

## Entwicklung

```bash
cargo test -- --test-threads=1   # einthreadig: Backends teilen sich echten Schlüsselbund-Zustand
cargo clippy --all-targets -- -D warnings
cargo fmt --check
make test                        # derselbe Testlauf
make lint                        # fmt --check + clippy
```

Schlüsselbund-Tests überspringen sich selbst, wenn kein Backend erreichbar ist,
damit die Bremsen-Tests in einem nackten Container trotzdem laufen.

Secret-Werte in der Testsuite werden zufällig erzeugt und nie ausgegeben —
Assertions prüfen auf Abwesenheit oder Anwesenheit, nie auf Gleichheit mit
einem geloggten Wert.

Beiträge sind willkommen. Ein Backend hinzuzufügen heißt, `put`/`get`/`delete`/
`list` des `Backend`-Traits zu implementieren; ein Harness hinzuzufügen heißt
ein Eintrag unter `integrations/`.

## Lizenz

Copyright (C) 2026 Furkan Akyol.

maskrun ist freie Software: Du darfst es unter den Bedingungen der GNU General
Public License, Version 3 oder einer späteren Version, wie von der Free
Software Foundation veröffentlicht, weiterverbreiten und verändern. Es kommt
ohne jede Gewährleistung. Die vollständigen Bedingungen stehen in
[LICENSE](LICENSE).

Eine praktische Konsequenz: Wenn du ein verändertes maskrun verbreitest — als
Quellcode, als Binary oder innerhalb eines Produkts — musst du deinen
veränderten Quellcode demjenigen, an den du es verbreitest, unter derselben
Lizenz zugänglich machen.
