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

Override detection with `MASKRUN_BACKEND=secret-service|keychain|dpapi`.

### What's actually verified

CI has never actually run for this repo, on any platform — see below. So
"verified" here means run by hand, not a green check.

The Linux backend runs for real: 85 tests pass locally against a live Secret
Service, including the keyring round-trip itself, not just the code around
it.

The macOS and Windows backends **have never been run at all**, against a real
Keychain or Credential Manager or otherwise — there is no such machine in this
loop. GitHub Actions does not start on this account right now — every run
across every private repo dies at `startup_failure` before a single job is
scheduled, most likely an account-level billing or email-verification issue,
not anything in this repo's workflow file. There is no CI badge in this
README for exactly that reason: a badge implies a check that actually ran,
and none has, on any platform. For a secrets tool, that is the one place this
cannot be allowed to overstate.

Practically: the Linux path is battle-tested by hand, the other two are
"compiles, matches the platform docs, has never touched real hardware."

## Commands

```
maskrun put <name>                    store a secret (prompts; not echoed)
maskrun put <name> --stdin            store from stdin (for scripts)
maskrun get <name>                    print it (refused in an agent session)
maskrun list                          names only
maskrun rm <name>                     delete
maskrun status                        check the manifest against the keyring
maskrun run [--raw|--mask] -- CMD     run with the manifest injected
maskrun exec VAR=name -- CMD          run with explicit pairs, no manifest
maskrun -- CMD                        shorthand for run
maskrun import <.env> [--dry-run]     move a .env into the keyring
maskrun install-guard [--remove]      register the agent guard
maskrun hook                          the guard itself (reads JSON on stdin)
```

`import` skips `VITE_`, `NEXT_PUBLIC_`, `PUBLIC_`, `REACT_APP_`, `NUXT_PUBLIC_`,
`EXPO_PUBLIC_` and `GATSBY_` variables: they are compiled into your client
bundle and shipped to every visitor, so they are configuration, not secrets, and
moving them buys nothing. `--all` overrides.

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
  22.04). Verified locally: 85 tests passing against a real Secret
  Service (see "What's actually verified" above — CI itself has not run).
- **macOS** — Intel and Apple Silicon. Implemented, never run.
- **Windows** — x86_64. Implemented, never run.

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
