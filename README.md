# maskrun

Run commands with secrets from your OS keyring — and keep the values out of
your AI coding agent's context.

```bash
maskrun put myapp-database-url        # stored in the keyring, never on disk
maskrun run -- npm run dev            # injected into the child, masked in output
```

```
$ maskrun run -- node -e 'console.log(process.env.DATABASE_URL)'
<masked:DATABASE_URL>
```

One binary, no runtime dependencies, no daemon. Linux, macOS and Windows.

---

## The problem

Moving secrets from `.env` files into the OS keyring is the easy half. The hard
half shows up once an AI agent is driving your shell.

Injecting a secret into a child process stops the agent from *reading the
vault*. It does nothing about the value *coming back out of the process*:

- a dev server prints its connection string at boot
- `curl -v` echoes the `Authorization` header
- a stack trace carries the DSN
- `psql` quotes the URL it failed to connect to

Any one of those puts the secret in the transcript, where it is now part of the
conversation, the scrollback, and wherever that transcript is stored.

maskrun closes that path, and the ones next to it.

## Install

**Linux / macOS** — downloads a prebuilt binary, verifies its checksum, no
compiler needed:

```bash
curl -fsSL https://raw.githubusercontent.com/furkanakyol/maskrun/main/install.sh | sh
```

**Windows (PowerShell)**

```powershell
irm https://raw.githubusercontent.com/furkanakyol/maskrun/main/install.ps1 | iex
```

**From a Rust toolchain:**

```bash
cargo install maskrun
```

**From source:**

```bash
git clone https://github.com/furkanakyol/maskrun
cd maskrun && cargo build --release
# binary at target/release/maskrun
```

## Quick start

```bash
# 1. move an existing .env into the keyring — look before you leap
maskrun import .env --dry-run
maskrun import .env

# 2. check that every name the manifest wants is actually present
maskrun status

# 3. run your app with no .env on disk
maskrun run -- npm run dev

# only now delete the .env
```

`import` writes a `.maskrun` manifest next to your code:

```
DATABASE_URL=myapp-database-url
JWT_SECRET=myapp-jwt-secret
```

**That file holds names, not values. Commit it.** It is `.env.example` that
actually does something: `maskrun status` tells a new teammate exactly which
secrets they are missing.

## How it works

Three mechanisms, for three different needs.

| Need | Mechanism | What the agent sees |
|---|---|---|
| A dev server, migration or CLI that needs `DATABASE_URL` | `maskrun run` injects into the child | masked output (`<masked:VAR>`) |
| The value itself — rotating a key, pasting into a dashboard | you, in your own terminal | nothing; the guard refuses |
| Knowing *which* secrets exist | `maskrun list`, `maskrun status` | names only, never values |

### Injection, not reading

Values live in the keyring. `maskrun run` resolves the manifest, hands the
values to exactly one child process, and stores nothing on disk. If any secret
is missing it refuses to start rather than booting your app with half an
environment.

### Output masking

`run` and `exec` pass the child's stdout and stderr through a filter that
replaces secret values with `<masked:VAR>`.

- Values reach the filter through **its environment**, never argv — argv is
  readable via `/proc/<pid>/cmdline`.
- Masks the raw value plus its base64, URL-encoded and backslash-escaped
  spellings.
- Catches a value **split across two writes**, so a secret straddling a flush
  boundary does not slip through.
- Passes through complete lines immediately, so a dev server's output is not
  buffered while you watch it.
- Refuses to mask values shorter than 6 characters, and says so: masking `5432`
  would corrupt every unrelated number in the output.
- Exit code, stdin and Ctrl-C reach the child unchanged.

On by default in an AI agent session and whenever stdout is not a terminal; off
in your own interactive terminal so colours survive. `--raw` forces it off,
`--mask` forces it on.

### The agent guard

```bash
maskrun install-guard        # registers a PreToolUse hook for Claude Code
```

The point of a hook is that **the harness enforces it, not the model**. A rule
written into a prompt is enforced by the model, so the model can talk itself
out of it. This cannot be talked out of.

It refuses the commands that would put a value in the transcript:

- `maskrun get`, `maskrun import`
- `secret-tool lookup/search`, `security find-generic-password`
- `--raw`, `MASKRUN_MASK=0`, `MASKRUN_ALLOW_READ=1` (turning masking off)
- `/proc/<pid>/environ`
- a bare `env` / `printenv`, `Get-ChildItem Env:`
- reading a `.env` / `.envrc`, whether through Bash or the agent's file tools

And leaves normal work alone: `maskrun run`, `env VAR=x cmd`, `cat
.env.example`, `cat .maskrun`, `printenv PATH`, `ls -la .env`, `rm .env`.

`maskrun install-guard` merges into your existing `settings.json`, backs it up
first, is idempotent, refuses to touch a file that is not valid JSON, and
`--remove` undoes it. Other harnesses: run `maskrun hook` as a pre-tool hook
that receives the tool call as JSON on stdin — see
[integrations/claude-code](integrations/claude-code/).

The CLI enforces the same refusals itself, so an agent running in a harness
with no hook support still cannot `maskrun get`.

### Guiding an agent that has no hook mechanism

```bash
maskrun install-rules        # writes a short block into AGENTS.md/CLAUDE.md/.cursor rules
```

`install-guard` only works where the harness runs a PreToolUse hook for you.
Elsewhere, there is nothing enforcing anything — so `install-rules` writes a
short, marked block into whichever of `AGENTS.md`, `CLAUDE.md` or
`.cursor/rules/` already exist in the project (creating `AGENTS.md` if none
do), telling the agent how to use maskrun here. **This is guidance, not
enforcement**: a model can still ignore it, the way it can ignore any other
instruction. It is a fallback for harnesses `install-guard` cannot reach, not
a substitute for it.

The block is bounded by `<!-- maskrun:start -->`/`<!-- maskrun:end -->`
markers, backs up the file first, is idempotent (a second run updates it in
place instead of duplicating it), and `--remove` takes it back out without
touching the rest of the file. With a `.maskrun` manifest present, the block
lists the actual variable names the project expects; `--file <path>` targets
one file directly, skipping discovery.

### The interactive view

```bash
maskrun            # in your own terminal, no arguments
```

A real terminal with nothing else on the command line opens an arrow-key
view: platforms on the left, that platform's secrets and the highlighted
one's detail (platform, note, value) on the right.

```
↑↓ move   → enter   ← back   e edit   d delete   v reveal   y copy   q quit
```

Values are masked (`••••••••••••••••`) until you press `v`, and a value is
only ever read from the keyring at that moment — moving through the list
never touches it. Moving to a different secret re-masks automatically. `e`
edits the platform, note and value in place (blank keeps the current one);
`d` asks for confirmation by name (`delete 'name'? [y/N]`) and only a literal
`y`/`Y` proceeds — every other key, including Enter, cancels. `y` copies the
value to your clipboard.

It runs on an [alternate screen
buffer](https://en.wikipedia.org/wiki/Terminal_emulator#Alternate_screen_buffer):
nothing it draws — including a revealed value — ever lands in your terminal's
scrollback, unlike `maskrun get`'s output today. This is a genuine
improvement independent of anything else below.

Like every other value-touching path, it **never opens in an AI agent
session** — piped output or not, a detected agent session always gets the
same names-only summary a bare `maskrun` prints non-interactively. It also
declines on a terminal smaller than 60x15, and tells you why before falling
back to that summary.

#### Clipboard copy

`y` copies the current secret to your clipboard, since revealing a value
you can't then paste anywhere just moves the problem — you'd retype it or
select it with the mouse instead, both worse. Three built-in limits:

- **Auto-clears after 45 seconds.** The TUI shows a live countdown once
  something is copied; `c` clears it immediately instead of waiting, and
  quitting the TUI with anything still copied clears it too. Clearing
  restores whatever was on the clipboard before the copy, or empties it if
  there was nothing.
- **A do-not-record hint goes out on every platform**, via arboard's
  `exclude_from_history`: KDE's `x-kde-passwordManagerHint` mime type on
  Linux, the community `org.nspasteboard.ConcealedType` convention on macOS,
  and the native `CanIncludeInClipboardHistory` clipboard format on Windows.
  Verified against a real Klipper here: a plain copy shows up in its
  history, a tagged one does not, and Klipper doesn't even report the
  tagged one as the *current* clipboard contents. Each is a hint a specific
  tool chooses to honour, not something maskrun enforces — see below.
- It **never opens in an agent session**, same gate as the rest of the TUI.

**What this doesn't fix:** any clipboard-history tool that isn't looking for
that hint (GNOME's, most non-KDE Wayland setups, and — observed directly
while building this — even KDE's own Klipper when it isn't watching the
clipboard transport your compositor happens to use) still records the value
permanently, and the 45-second auto-clear does nothing to that copy once
it's in a history file. Treat clipboard copy the same as `/proc/<pid>/environ`
and a human pasting an unmasked value by hand, below: a real limit, not a
solved problem.

## What this is not

**maskrun is not a security boundary.** It is hardening against accidents, and
it should not be sold to you — or by you — as anything more.

- **A keyring solves storage, not access.** Every process running as you can
  call `secret-tool lookup` or `security find-generic-password`, including your
  agent. The guard raises the cost of doing it by accident; it does not make it
  impossible. The same is true of you: if a human pastes an unmasked value into
  the transcript by hand, no tool downstream of that keystroke can catch it.
- **Code the guard cannot read.** Heredoc bodies and script files are treated
  as data, not commands — deliberately, because parsing them produced false
  positives. So a script that reads `.env` itself from inside its own source
  gets through. A pattern matcher never catches arbitrary code.
- **Process environment.** While `maskrun run` is running, its child's
  environment is readable via `/proc/<pid>/environ`. The guard blocks that path
  directly, but env injection has this shape by design.
- **Clipboard copy (`y` in the interactive view) is not undone by auto-clear.**
  The 45-second timeout empties *the clipboard*; it does nothing to a
  clipboard-history tool that already recorded the value permanently before
  that timeout fired. maskrun tags the copy so KDE's Klipper skips it — real
  and verified, not every history-keeping tool honours that tag, and GNOME's
  clipboard history, most non-KDE Wayland setups, and Windows Clipboard
  History are not known to. Same shelf as `/proc/<pid>/environ` above and a
  human pasting an unmasked value by hand below: a limit stated plainly, not
  quietly solved.
- **`ps` during a write — closed.** On macOS, the value used to reach `security`
  as a CLI argument, briefly visible in the process list via `ps`. That path is
  gone: maskrun now calls Security.framework's generic-password API directly,
  so the value never becomes an argv the OS has to expose to anyone.
- **Pipeline-segment matching, not a shell parser.** The guard evaluates each
  segment of a pipeline on its own, which is what lets `sed 's/maskrun
  get/x/' notes.md` through — that text never invokes `maskrun get`, it just
  mentions it. The same scoping means the guard does not follow a command
  through indirection: `echo 'maskrun get x' | sh` reads as an `echo`, not as
  the command `sh` ends up running.

If you want a real boundary, the agent's shell has to run somewhere that cannot
reach the keyring at all — no D-Bus session socket on Linux, a separate user
account, or a container. Then maskrun works from your terminal and not from the
agent's. Blast radius is better managed at the provider: a separate key per
tool, spend caps, rotation, and short-lived credentials where they exist.

## Backends

| Platform | Backend name | Storage |
|---|---|---|
| Linux | `secret-service` | libsecret's Secret Service, over D-Bus directly (`dbus-secret-service` crate) — gnome-keyring, KWallet, KeePassXC |
| macOS | `keychain` | login keychain via Security.framework's generic-password API (`security-framework` crate) |
| Windows | `dpapi` | Windows Credential Manager (`windows` crate, `Win32_Security_Credentials`) — the name is kept from the old DPAPI-file backend for override compatibility; the storage underneath it is not DPAPI files anymore |

Earlier versions shelled out to `secret-tool` on Linux and `security` on
macOS for every operation, and hand-rolled DPAPI file storage on Windows. All
three now go through a library binding to the platform API for `put`/`get`/
`delete` instead of a subprocess or hand-written crypto — no `secret-tool`
install required on Linux, and on macOS the secret value no longer becomes a
CLI argument. (macOS `list` still shells out to `security dump-keychain` to
enumerate names — that call takes no secret value as an argument, so nothing
new is exposed; `security-framework` has no service-scoped enumeration call
to replace it with.)

Linux storage keeps the schema `secret-tool lookup service maskrun name
<name>` expects (`service` + `name` attributes), so a secret maskrun stores is
still readable with the standard CLI if you need to check by hand — maskrun
itself just no longer depends on that CLI being installed.

### Where the platform label and note live

| Platform | Where |
|---|---|
| Linux | Two extra Secret Service attributes, `platform` and `note`, alongside the existing `service`/`name` — `secret-tool lookup` only matches on the attributes you give it, so these ride along without affecting anything that already reads `service`+`name`. |
| macOS | The generic password's `kSecAttrDescription` (platform) and `kSecAttrComment` (note) fields, set/read through `security-framework`'s attribute-search and attribute-only update APIs — the stored value itself is never touched by a relabel. |
| Windows | Both encoded into the one `CREDENTIALW.Comment` field Credential Manager offers (`platform=<p><US>note=<n>`, `<US>` = U+001F): there's no separate attribute store here, and relabelling has to rewrite the whole entry (value included) because `CredWriteW` has no partial-update call. |

Override detection with `MASKRUN_BACKEND=secret-service|keychain|dpapi`.

### What's actually verified

All three backends are exercised against a real keyring in CI: Secret Service
on Linux, Keychain on macOS, Credential Manager on Windows. The keyring tests
round-trip an actual secret rather than mocking the backend, and a job with no
keyring installed at all proves the guard still answers.

The platform label / note feature is the same story: exercised against a
real Secret Service on Linux (including the `secret-tool`/attribute-schema
compatibility check), type-checked cross-target for macOS and Windows the
same as the rest of the platform code, but only actually run against a real
Keychain or Credential Manager once those CI jobs do.

The first CI run that ever started found three genuine bugs in the platform
code, all in paths a Linux build never type-checks because they are
`cfg`-gated: two mistyped Win32 arguments, a `CredEnumerateW` call passing
both a filter and the all-credentials flag (invalid together), and an
`ERROR_NOT_FOUND` comparison against the raw Win32 code instead of the
`HRESULT` that actually arrives — which made a missing secret and an empty
store both surface as a raw error. The lint job now cross-checks the Windows
and macOS targets from Linux so that class of mistake cannot reach a platform
runner again.

What is *not* covered: the lock-handling tests are opt-in
(`MASKRUN_LOCK_TESTS=1`), because spawning a throwaway `gnome-keyring-daemon`
on a real desktop prompts the user to create a keyring and outlives the test.

## Commands

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

`import` skips `VITE_`, `NEXT_PUBLIC_`, `PUBLIC_`, `REACT_APP_`, `NUXT_PUBLIC_`,
`EXPO_PUBLIC_` and `GATSBY_` variables: they are compiled into your client
bundle and shipped to every visitor, so they are configuration, not secrets, and
moving them buys nothing. `--all` overrides.

### Platform labels and notes

Once you're past a handful of secrets, a flat name is not enough to remember
what each one is for — especially when a platform has more than one key and
the difference between them is scope, not name. `--for` tags a secret with a
platform/service; `--note` adds a short free-text description:

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

Both flags are optional and apply to secrets that already have neither.
`label` retags an existing secret after the fact; on either command, giving
`--for ""` (or `--note ""`) clears that field, and leaving a flag off entirely
leaves the existing value alone — `put`-ing over a secret's value never
silently drops its label. **The platform and note are not secrets**: they are
stored unmasked, are visible to an AI agent session, and are shown by `list`
and `status`. Do not put a secret value in `--note`; it is capped at 200
characters and may not contain a newline.

`maskrun` with no arguments opens [the interactive view](#the-interactive-view)
in a real terminal; piped (`maskrun | cat`), non-interactive, or in an agent
session, it prints a short overview instead: the manifest status and the
grouped secret list above, so you don't have to hold the command surface in
your head.

### Shell completion

```bash
maskrun completions fish > ~/.config/fish/completions/maskrun.fish
maskrun completions bash > ~/.local/share/bash-completion/completions/maskrun
maskrun completions zsh > ~/.zfunc/_maskrun   # then `fpath+=~/.zfunc` before compinit
```

Beyond flags and subcommands, `maskrun get`/`rm`/`label` complete actual
secret names — fish does this out of the box (via the same `maskrun list
--plain` this flag exists for); bash/zsh get the static completions only.

## Environment variables

| Variable | Effect |
|---|---|
| `MASKRUN_MASK` | `1` always mask, `0` never mask |
| `MASKRUN_AGENT` | `1` treat this as an agent session, `0` treat it as human |
| `MASKRUN_BACKEND` | force a backend |
| `MASKRUN_ALLOW_READ` | `1` re-enables `get`/`import` in an agent session |

Agent sessions are detected from `CLAUDECODE`, `CLAUDE_CODE_ENTRYPOINT`,
`AI_AGENT`, `AIDER_CHAT`, `CURSOR_AGENT`, `OPENAI_CODEX`, `GEMINI_CLI` and
`REPLIT_AGENT`. For anything else, set `MASKRUN_AGENT=1` in the harness.

## Platform support

- **Linux** — glibc 2.35 or newer (the release binaries are built on Ubuntu
  22.04). Tested in CI against a live Secret Service.
- **macOS** — Intel and Apple Silicon. Tested in CI against a real Keychain.
- **Windows** — x86_64. Tested in CI against the real Credential Manager.

## Prior art

[`envchain`](https://github.com/sorah/envchain) put secrets in the keychain and
injected them into the environment years ago, and is the direct ancestor of the
`run` half of this tool. [`direnv`](https://direnv.net/) manages per-directory
environments, [`sops`](https://github.com/getsops/sops) and
[`dotenvx`](https://github.com/dotenvx/dotenvx) encrypt secrets at rest in the
repository, and [`aws-vault`](https://github.com/99designs/aws-vault) does the
keychain dance for one provider.

What maskrun adds is the agent-facing half: masking the child's output, and a
harness-enforced guard on the commands that would leak a value. If you are not
working with an AI agent, `envchain` may be all you need.

## Development

```bash
cargo test -- --test-threads=1   # single-threaded: backends share real keyring state
cargo clippy --all-targets -- -D warnings
cargo fmt --check
make test                        # same test run
make lint                        # fmt --check + clippy
```

Keyring tests skip themselves when no backend is reachable so the guard tests
still run in a bare container.

Secret values in the test suite are randomly generated and never printed —
assertions check for absence or presence, never equality against a logged
value.

Contributions welcome. Adding a backend means implementing the `Backend`
trait's `put`/`get`/`delete`/`list`; adding a harness means one entry under
`integrations/`.

## License

Copyright (C) 2026 Furkan Akyol.

maskrun is free software: you may redistribute and modify it under the terms
of the GNU General Public License, version 3 or any later version, as
published by the Free Software Foundation. It comes with no warranty. See
[LICENSE](LICENSE) for the full terms.

A practical consequence: if you distribute a modified maskrun — as source, as
a binary, or inside a product — you must make your modified source available
to whoever you distribute it to, under this same license.
