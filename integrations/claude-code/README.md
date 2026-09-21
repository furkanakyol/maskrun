# Claude Code integration

```bash
maskrun install-guard
```

That merges the guard into `~/.claude/settings.json`, backs the file up first,
and leaves any hooks you already have alone. Restart Claude Code (or open
`/hooks` once) to load it. `maskrun install-guard --remove` undoes it, and
`--dry-run` shows the result without writing.

## What it adds

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash|Read|Edit|Write|NotebookEdit",
        "hooks": [
          {
            "type": "command",
            "command": "maskrun hook",
            "timeout": 10,
            "statusMessage": "maskrun guard"
          }
        ]
      }
    ]
  }
}
```

`PreToolUse` runs before a tool call and can deny it. Claude Code sends the
call as JSON on stdin; `maskrun hook` answers with a deny decision or stays
silent.

The `matcher` covers `Bash` because that is where `cat .env` and
`maskrun get` happen, and the file tools because `Read` on a `.env` leaks
exactly the same way.

## Why a hook and not an instruction

You can write "never print secrets" in `CLAUDE.md`. That rule is enforced by
the model, which means the model can reason its way around it — and a model
that has decided a rule does not apply to the current case will not stop
itself.

A hook is executed by the harness. The model does not get a vote.

## Checking it works

Ask Claude Code to run this, in a session where the guard is installed:

```
cat .env
```

The call should be denied before it executes, with a message pointing at
`maskrun status` and `maskrun run`. Also try `maskrun get some-name` — same
result.

If nothing happens, the hook is not loaded: open `/hooks` once, or restart.
`maskrun install-guard --dry-run` will show you whether the entry is actually
in the file.

## False positives

The guard is a pattern matcher, not a shell parser, so it will occasionally
refuse something innocent. Two deliberate scoping decisions keep that rare:

- **Every rule is checked against pipeline segments, not the whole command
  string.** A flagged name only fires when it is the command actually
  invoked (or, for env-var rules, actually assigned) in that segment — not
  whenever it merely appears somewhere in the line. This is why `sed
  's/maskrun get/x/' notes.md` is allowed: `maskrun get` never runs there,
  the text just sits inside `sed`'s own argument. Same reasoning covers a
  `.env` that appears only in prose, or as some other command's argument
  that isn't a file-reading one.
- **Heredoc bodies are ignored.** A body is data, not a command. Without this,
  writing documentation that merely mentions `cat .env` tripped the guard.

The trade-off is real: segment-level matching does not follow a command
through indirection. `echo 'maskrun get x' | sh` is read as an `echo`
segment and a `sh` segment, neither of which is the flagged command as
written, so it passes — see the README's "What this is not".

If you hit a false positive, it is a bug worth reporting — include the exact
command. If you need to get past one immediately, `--remove` the guard, do the
thing, and put it back.

## Other harnesses

Any harness that can run a command before a tool call and act on its verdict
can use `maskrun hook`. It reads

```json
{"tool_name": "Bash", "tool_input": {"command": "cat .env"}}
```

on stdin and, when the call should be refused, prints

```json
{"hookSpecificOutput": {"hookEventName": "PreToolUse",
                        "permissionDecision": "deny",
                        "permissionDecisionReason": "maskrun guard: ..."}}
```

and exits 0. Allowed calls produce no output. Unparseable input is always
allowed, so a malformed payload can never block your work.

Also set `MASKRUN_AGENT=1` in the harness environment if it is not one of the
ones maskrun already recognises (see the README) — that is what turns on output
masking and the `get` refusal.
