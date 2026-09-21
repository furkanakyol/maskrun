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

One file, no build step, no daemon. Linux, macOS and Windows.

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

**Linux / macOS**

```bash
curl -fsSL https://raw.githubusercontent.com/furkanakyol/maskrun/main/install.sh | sh
```

**Windows (PowerShell)**

```powershell
irm https://raw.githubusercontent.com/furkanakyol/maskrun/main/install.ps1 | iex
```

Or just copy [`bin/maskrun`](bin/maskrun) onto your `PATH` and make it
executable — it is a single Python file with no dependencies beyond the
standard library.

Requirements: Python 3.9+, and on Linux `secret-tool` (libsecret) with a
running Secret Service.

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
  impossible.
- **Code the guard cannot read.** Heredoc bodies and script files are treated
  as data, not commands — deliberately, because parsing them produced false
  positives. So a script that calls `open(".env").read()` gets through. A
  pattern matcher never catches arbitrary code.
- **Process environment.** While `maskrun run` is running, its child's
  environment is readable via `/proc/<pid>/environ`. The guard blocks that path
  directly, but env injection has this shape by design.
- **`ps` during a write.** On macOS, `security` takes the value as an argument,
  so it is briefly visible in the process list to your own user. `put` and
  `import` are human-run commands, which limits the window.

If you want a real boundary, the agent's shell has to run somewhere that cannot
reach the keyring at all — no D-Bus session socket on Linux, a separate user
account, or a container. Then maskrun works from your terminal and not from the
agent's. Blast radius is better managed at the provider: a separate key per
tool, spend caps, rotation, and short-lived credentials where they exist.

## Backends

| Platform | Backend | Storage |
|---|---|---|
| Linux | `secret-service` | libsecret via `secret-tool` (gnome-keyring, KWallet, KeePassXC) |
| macOS | `keychain` | login keychain via `security` |
| Windows | `dpapi` | DPAPI-encrypted files under `%LOCALAPPDATA%\maskrun` |

Windows has no scriptable equivalent of `secret-tool`: Credential Manager's
PowerShell module is not built in, and `cmdkey` cannot read a secret back.
DPAPI ships with the OS, ties the ciphertext to the logged-in account, and
needs no third-party dependency.

Override detection with `MASKRUN_BACKEND=secret-service|keychain|dpapi`.

All three are exercised by CI on every push, on real `ubuntu-latest`,
`macos-latest` and `windows-latest` runners — including a job that fails if the
keyring tests were merely skipped.

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
python3 test/test_maskrun.py          # everything reachable here
python3 test/test_maskrun.py -v       # verbose
make test
```

Keyring tests skip themselves when no backend is reachable so the guard tests
still run in a bare container; CI passes `--require-keyring` to make sure that
skip never masquerades as a pass.

Secret values in the test suite are randomly generated and never printed —
assertions check for absence or presence, never equality against a logged
value.

Contributions welcome. Adding a backend means implementing four methods
(`put`, `get`, `delete`, `list`); adding a harness means one entry under
`integrations/`.

## License

MIT — see [LICENSE](LICENSE).
