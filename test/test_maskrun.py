#!/usr/bin/env python3
"""maskrun test suite. Runs on Linux, macOS and Windows.

    python3 test/test_maskrun.py            everything available here
    python3 test/test_maskrun.py -v         verbose

Keyring-backed tests are skipped (not failed) when no keyring is reachable, so
the guard tests still run in a bare container. CI asserts that the keyring
tests actually ran on all three platforms.

Secret values are generated at random and never printed: assertions check for
absence/presence, never equality against a logged value.
"""

import base64
import hashlib
import importlib.util
import json
import os
import secrets
import shutil
import subprocess
import sys
import tempfile
import unittest
import urllib.parse

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
MASKRUN = os.path.join(ROOT, "bin", "maskrun")
PY = sys.executable


def load_module():
    """Import bin/maskrun (no .py extension) for direct unit tests."""
    spec = importlib.util.spec_from_loader(
        "maskrun_mod",
        importlib.machinery.SourceFileLoader("maskrun_mod", MASKRUN),
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


mr = load_module()


def run_cli(args, env_extra=None, stdin_data=None, cwd=None):
    env = dict(os.environ)
    env["MASKRUN_AGENT"] = "1"        # tests assert agent-session behaviour
    env.pop("MASKRUN_MASK", None)
    env.pop("MASKRUN_ALLOW_READ", None)
    if env_extra:
        env.update(env_extra)
    return subprocess.run(
        [PY, MASKRUN] + args,
        input=stdin_data,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=env,
        cwd=cwd,
    )


def keyring_available():
    probe = "maskrun-selftest-probe"
    try:
        put = run_cli(["put", probe, "--stdin"], stdin_data=b"probe-value-long-enough")
        if put.returncode != 0:
            return False
        got = run_cli(["get", probe], env_extra={"MASKRUN_ALLOW_READ": "1"})
        run_cli(["rm", probe])
        return got.returncode == 0 and b"probe-value-long-enough" in got.stdout
    except Exception:
        return False


KEYRING = keyring_available()
needs_keyring = unittest.skipUnless(
    KEYRING, "no reachable keyring backend on this machine")


class KeyringBase(unittest.TestCase):
    """Stores a random secret per test and removes it afterwards."""

    def setUp(self):
        self.names = []

    def tearDown(self):
        for name in self.names:
            run_cli(["rm", name])

    def store(self, value, name=None):
        name = name or "maskrun-test-%s" % secrets.token_hex(6)
        result = run_cli(["put", name, "--stdin"], stdin_data=value.encode())
        self.assertEqual(result.returncode, 0, result.stderr.decode())
        self.names.append(name)
        return name


# --------------------------------------------------------------------------
# Keyring round-trip
# --------------------------------------------------------------------------

@needs_keyring
class TestKeyring(KeyringBase):

    def test_roundtrip_preserves_awkward_values(self):
        cases = {
            "url with = in query": "postgres://u:p@h:5432/db?opt=a=b&x=1",
            "double quoted": 'value "with" quotes',
            "single quoted": "value 'with' quotes",
            "base64 padding": base64.b64encode(b"some binary payload").decode(),
            "spaces": "a value with spaces",
            "unicode": "parola-çöğüş-éè",
            "trailing equals": "abcdefgh==",
        }
        for label, value in cases.items():
            with self.subTest(label):
                name = self.store(value)
                got = run_cli(["get", name], env_extra={"MASKRUN_ALLOW_READ": "1"})
                self.assertEqual(got.returncode, 0, got.stderr.decode())
                returned = got.stdout.decode("utf-8").rstrip("\n")
                self.assertEqual(
                    hashlib.sha256(returned.encode()).hexdigest(),
                    hashlib.sha256(value.encode()).hexdigest(),
                    "%s did not round-trip byte-for-byte" % label,
                )

    def test_list_shows_name_not_value(self):
        value = secrets.token_hex(24)
        name = self.store(value)
        listed = run_cli(["list"])
        self.assertEqual(listed.returncode, 0)
        self.assertIn(name, listed.stdout.decode())
        self.assertNotIn(value, listed.stdout.decode())

    def test_rm_removes(self):
        name = self.store(secrets.token_hex(24))
        self.assertEqual(run_cli(["rm", name]).returncode, 0)
        gone = run_cli(["get", name], env_extra={"MASKRUN_ALLOW_READ": "1"})
        self.assertNotEqual(gone.returncode, 0)

    def test_get_missing_fails(self):
        result = run_cli(["get", "maskrun-definitely-absent-%s" % secrets.token_hex(4)],
                         env_extra={"MASKRUN_ALLOW_READ": "1"})
        self.assertNotEqual(result.returncode, 0)

    def test_invalid_name_rejected(self):
        for bad in ["has space", "has/slash", "-leading-dash", ""]:
            with self.subTest(bad):
                self.assertNotEqual(run_cli(["get", bad]).returncode, 0)


# --------------------------------------------------------------------------
# Output masking
# --------------------------------------------------------------------------

@needs_keyring
class TestMasking(KeyringBase):

    def setUp(self):
        super().setUp()
        self.value = secrets.token_hex(24)
        self.name = self.store(self.value)

    def exec_child(self, code, extra_args=None, env_extra=None, stdin_data=None):
        args = ["exec"] + (extra_args or []) + \
               ["TEST=%s" % self.name, "--", PY, "-c", code]
        return run_cli(args, env_extra=env_extra, stdin_data=stdin_data)

    def test_stdout_masked(self):
        result = self.exec_child(
            'import os;print("leak: " + os.environ["TEST"])')
        out = result.stdout.decode()
        self.assertNotIn(self.value, out)
        self.assertIn("<masked:TEST>", out)

    def test_stderr_masked(self):
        result = self.exec_child(
            'import os,sys;sys.stderr.write("err: " + os.environ["TEST"] + "\\n")')
        err = result.stderr.decode()
        self.assertNotIn(self.value, err)
        self.assertIn("<masked:TEST>", err)

    def test_raw_flag_disables_masking(self):
        result = self.exec_child(
            'import os;print(os.environ["TEST"])', extra_args=["--raw"])
        self.assertIn(self.value, result.stdout.decode())

    def test_mask_env_var_disables_masking(self):
        result = self.exec_child(
            'import os;print(os.environ["TEST"])',
            env_extra={"MASKRUN_MASK": "0"})
        self.assertIn(self.value, result.stdout.decode())

    def test_base64_variant_masked(self):
        result = self.exec_child(
            'import os,base64;'
            'print(base64.b64encode(os.environ["TEST"].encode()).decode())')
        encoded = base64.b64encode(self.value.encode()).decode()
        self.assertNotIn(encoded, result.stdout.decode())

    def test_urlencoded_variant_masked(self):
        result = self.exec_child(
            'import os,urllib.parse;'
            'print(urllib.parse.quote(os.environ["TEST"], safe=""))')
        self.assertNotIn(self.value, result.stdout.decode())
        self.assertNotIn(urllib.parse.quote(self.value, safe=""),
                         result.stdout.decode())

    def test_output_without_newline_masked(self):
        result = self.exec_child(
            'import os,sys;sys.stdout.write("prompt " + os.environ["TEST"])')
        self.assertNotIn(self.value, result.stdout.decode())

    def test_value_split_across_writes_masked(self):
        # The regression that matters: a secret straddling two flushes.
        result = self.exec_child(
            'import os,sys,time;v=os.environ["TEST"];'
            'sys.stdout.write("head " + v[:20]);sys.stdout.flush();'
            'time.sleep(0.3);sys.stdout.write(v[20:] + " tail\\n")')
        self.assertNotIn(self.value, result.stdout.decode())

    def test_exit_code_preserved(self):
        self.assertEqual(self.exec_child("raise SystemExit(42)").returncode, 42)

    def test_stdin_reaches_child(self):
        result = self.exec_child(
            'import sys;sys.stdout.write(sys.stdin.read())',
            stdin_data=b"hello-from-stdin")
        self.assertIn("hello-from-stdin", result.stdout.decode())

    def test_short_value_warns_and_is_not_masked(self):
        short = self.store("abc")
        result = run_cli(["exec", "SHORT=%s" % short, "--", PY, "-c",
                          'import os;print(os.environ["SHORT"])'])
        self.assertIn("left unmasked", result.stderr.decode())

    def test_missing_secret_refuses_to_run(self):
        result = run_cli(["exec", "GONE=maskrun-absent-x", "--", PY, "-c", "print(1)"])
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("half-filled", result.stderr.decode())

    def test_get_refused_in_agent_session(self):
        result = run_cli(["get", self.name])
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn(self.value, result.stdout.decode())
        self.assertIn("disabled inside an AI agent session", result.stderr.decode())


# --------------------------------------------------------------------------
# Manifest and import
# --------------------------------------------------------------------------

@needs_keyring
class TestManifest(KeyringBase):

    def setUp(self):
        super().setUp()
        self.value = secrets.token_hex(24)
        self.name = self.store(self.value)
        self.dir = tempfile.mkdtemp(prefix="maskrun-test-")
        with open(os.path.join(self.dir, ".maskrun"), "w") as fh:
            fh.write("TEST=%s\n" % self.name)

    def tearDown(self):
        shutil.rmtree(self.dir, ignore_errors=True)
        super().tearDown()

    def test_run_injects_and_masks(self):
        result = run_cli(["run", "--", PY, "-c",
                          'import os;print("run: " + os.environ["TEST"])'],
                         cwd=self.dir)
        self.assertEqual(result.returncode, 0, result.stderr.decode())
        self.assertNotIn(self.value, result.stdout.decode())
        self.assertIn("<masked:TEST>", result.stdout.decode())

    def test_bare_double_dash_is_run(self):
        result = run_cli(["--", PY, "-c",
                          'import os;print("bare: " + os.environ["TEST"])'],
                         cwd=self.dir)
        self.assertEqual(result.returncode, 0, result.stderr.decode())
        self.assertIn("<masked:TEST>", result.stdout.decode())

    def test_manifest_found_from_subdirectory(self):
        deep = os.path.join(self.dir, "a", "b")
        os.makedirs(deep)
        result = run_cli(["status"], cwd=deep)
        self.assertEqual(result.returncode, 0, result.stderr.decode())
        self.assertIn("TEST", result.stdout.decode())

    def test_status_ok(self):
        result = run_cli(["status"], cwd=self.dir)
        self.assertEqual(result.returncode, 0)
        self.assertIn("ok", result.stdout.decode())
        self.assertNotIn(self.value, result.stdout.decode())

    def test_status_reports_missing(self):
        with open(os.path.join(self.dir, ".maskrun"), "a") as fh:
            fh.write("OTHER=maskrun-absent-%s\n" % secrets.token_hex(4))
        result = run_cli(["status"], cwd=self.dir)
        self.assertEqual(result.returncode, 1)
        self.assertIn("MISSING", result.stdout.decode())

    def test_malformed_manifest_errors(self):
        with open(os.path.join(self.dir, ".maskrun"), "w") as fh:
            fh.write("this line has no equals sign\n")
        result = run_cli(["status"], cwd=self.dir)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("expected VAR=secret-name", result.stderr.decode())


@needs_keyring
class TestImport(KeyringBase):

    def setUp(self):
        super().setUp()
        self.dir = tempfile.mkdtemp(prefix="maskrun-import-")
        self.values = {
            "DATABASE_URL": "postgres://u:p@h:5432/db?opt=a=b",
            "JWT_SECRET": base64.b64encode(b"jwt-signing-material").decode(),
            "QUOTED": "value with spaces",
            "VITE_PUBLIC_URL": "https://example.com",
        }
        with open(os.path.join(self.dir, ".env"), "w") as fh:
            fh.write('DATABASE_URL=%s\n' % self.values["DATABASE_URL"])
            fh.write('JWT_SECRET="%s"\n' % self.values["JWT_SECRET"])
            fh.write("QUOTED='%s'\n" % self.values["QUOTED"])
            fh.write("# a comment\n\n")
            fh.write("VITE_PUBLIC_URL=%s\n" % self.values["VITE_PUBLIC_URL"])

    def tearDown(self):
        for var in self.values:
            self.names.append("imp-%s" % var.lower().replace("_", "-"))
        shutil.rmtree(self.dir, ignore_errors=True)
        super().tearDown()

    def import_cmd(self, *extra):
        return run_cli(["import", ".env", "--prefix", "imp"] + list(extra),
                       env_extra={"MASKRUN_AGENT": "0"}, cwd=self.dir)

    def test_refused_in_agent_session(self):
        result = run_cli(["import", ".env"], cwd=self.dir)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("disabled inside an AI agent session", result.stderr.decode())

    def test_dry_run_writes_nothing(self):
        result = self.import_cmd("--dry-run")
        self.assertEqual(result.returncode, 0, result.stderr.decode())
        self.assertFalse(os.path.exists(os.path.join(self.dir, ".maskrun")))
        self.assertIn("nothing was written", result.stdout.decode())

    def test_import_preserves_values_and_skips_public(self):
        result = self.import_cmd()
        self.assertEqual(result.returncode, 0, result.stderr.decode())

        manifest = open(os.path.join(self.dir, ".maskrun")).read()
        self.assertIn("DATABASE_URL=imp-database-url", manifest)
        self.assertNotIn("VITE_PUBLIC_URL", manifest)   # client-bundled, skipped
        for value in self.values.values():
            self.assertNotIn(value, manifest)           # manifest holds no values

        for var in ("DATABASE_URL", "JWT_SECRET", "QUOTED"):
            name = "imp-%s" % var.lower().replace("_", "-")
            got = run_cli(["get", name], env_extra={"MASKRUN_ALLOW_READ": "1"})
            self.assertEqual(got.returncode, 0, got.stderr.decode())
            returned = got.stdout.decode("utf-8").rstrip("\n")
            self.assertEqual(
                hashlib.sha256(returned.encode()).hexdigest(),
                hashlib.sha256(self.values[var].encode()).hexdigest(),
                "%s did not survive import byte-for-byte" % var,
            )


# --------------------------------------------------------------------------
# Agent guard — no keyring needed
# --------------------------------------------------------------------------

BLOCK = [
    ("maskrun get", "Bash", "command", "maskrun get my-api-key"),
    ("secret-tool lookup", "Bash", "command",
     "secret-tool lookup service maskrun name x"),
    ("secret-tool search", "Bash", "command", "secret-tool search --all service maskrun"),
    ("security find-generic-password", "Bash", "command",
     "security find-generic-password -s maskrun -a x -w"),
    ("cat .env", "Bash", "command", "cat .env"),
    ("head .env.production", "Bash", "command", "head -5 apps/api/.env.production"),
    ("cat .env.local piped", "Bash", "command", "cat .env.local | grep DB"),
    ("source .env", "Bash", "command", "source .env && npm run dev"),
    ("cat .envrc", "Bash", "command", "cat .envrc"),
    ("grep on .env", "Bash", "command", "grep DATABASE .env"),
    ("grep -E with a pattern then .env", "Bash", "command", "grep -E 'DB|KEY' .env"),
    ("awk over .env", "Bash", "command", "awk '{print $1}' .env"),
    ("diff .env", "Bash", "command", "diff .env .env.example"),
    ("cat redirected from .env", "Bash", "command", "cat < .env"),
    ("sed on .env", "Bash", "command", "sed -n 1,5p .env"),
    ("/proc/pid/environ", "Bash", "command", "cat /proc/1234/environ"),
    ("/proc/self/environ", "Bash", "command", "tr '\\0' '\\n' < /proc/self/environ"),
    ("bare env", "Bash", "command", "env"),
    ("env piped", "Bash", "command", "env | grep -i key"),
    ("bare printenv", "Bash", "command", "printenv"),
    ("powershell env dump", "Bash", "command", "Get-ChildItem Env:"),
    ("run --raw", "Bash", "command", "maskrun run --raw -- npm run dev"),
    ("MASKRUN_MASK=0", "Bash", "command", "MASKRUN_MASK=0 maskrun run -- npm run dev"),
    ("MASKRUN_ALLOW_READ=1", "Bash", "command", "MASKRUN_ALLOW_READ=1 maskrun get x"),
    ("maskrun import", "Bash", "command", "maskrun import .env --prefix app"),
    ("Read .env", "Read", "file_path", "/home/x/proj/.env"),
    ("Read .env.production", "Read", "file_path", "/home/x/proj/.env.production"),
    ("Edit .env", "Edit", "file_path", "/home/x/proj/.env"),
]

ALLOW = [
    ("maskrun run", "Bash", "command", "maskrun run -- npm run dev"),
    ("maskrun exec", "Bash", "command",
     "maskrun exec API=my-key -- curl https://api.example.com"),
    ("maskrun list", "Bash", "command", "maskrun list"),
    ("maskrun status", "Bash", "command", "maskrun status"),
    ("maskrun put", "Bash", "command", "maskrun put new-key"),
    ("env VAR=1 cmd", "Bash", "command", "env NODE_ENV=test npm test"),
    ("cat .env.example", "Bash", "command", "cat .env.example"),
    ("cat .env.sample", "Bash", "command", "cat .env.sample"),
    ("cat .maskrun", "Bash", "command", "cat .maskrun"),
    ("printenv PATH", "Bash", "command", "printenv PATH"),
    ("single powershell var", "Bash", "command", "echo $env:PATH"),
    ("ls -la .env", "Bash", "command", "ls -la .env"),
    ("rm .env", "Bash", "command", "rm .env"),
    ("write to .env", "Bash", "command", 'echo "DATABASE_URL=x" > .env'),
    ("grep in src", "Bash", "command", "grep -rn foo src/"),
    ("git status", "Bash", "command", "git status"),
    ("node --env-file", "Bash", "command", "node --env-file=.env server.js"),
    (".env in prose piped to head", "Bash", "command",
     'echo "secrets live in the keyring, not .env" | head -3'),
    (".env in prose piped to grep", "Bash", "command",
     'echo "look at .env someday" | grep -o env'),
    # Found by dogfooding: ".env" as a search PATTERN, not a file to read.
    ("ls piped to grep -E for .env", "Bash", "command", 'ls -la | grep -E "\\.env"'),
    ("ls piped to plain grep .env", "Bash", "command", "ls -la | grep '\\.env'"),
    ("rg searching for the text .env", "Bash", "command", "rg .env"),
    ("grep -r for .env across the tree", "Bash", "command", "grep -rn .env src/"),
    ("wc counts lines without printing them", "Bash", "command", "wc -l .env"),
    ("Read .env.example", "Read", "file_path", "/home/x/proj/.env.example"),
    ("Read .maskrun", "Read", "file_path", "/home/x/proj/.maskrun"),
    ("Read source file", "Read", "file_path", "/home/x/proj/src/index.ts"),
    ("heredoc body then head", "Bash", "command",
     'cat <<\'EOF\' > note.txt\nDATABASE_URL is not in .env any more\nEOF\nls | head -3'),
    ("heredoc documenting the guard", "Bash", "command",
     'cat <<\'EOF\' > doc.md\nWe used to run `maskrun get` and `cat .env`.\nEOF\nwc -l doc.md'),
    ("python heredoc mentioning .env", "Bash", "command",
     'python3 - <<\'PY\'\ntext = """\ncat .env is a leak\n"""\nPY\nbash x.sh | tail -5'),
]


class TestGuard(unittest.TestCase):
    """The guard is pure logic: exercised directly and through the CLI."""

    def decide(self, tool, field, value):
        return mr.guard_decision({"tool_name": tool, "tool_input": {field: value}})

    def test_blocks(self):
        for label, tool, field, value in BLOCK:
            with self.subTest(label):
                self.assertIsNotNone(
                    self.decide(tool, field, value),
                    "should have been blocked: %s" % value)

    def test_allows(self):
        for label, tool, field, value in ALLOW:
            with self.subTest(label):
                self.assertIsNone(
                    self.decide(tool, field, value),
                    "should have been allowed: %s" % value)

    def test_cli_hook_emits_deny_json(self):
        payload = json.dumps(
            {"tool_name": "Bash", "tool_input": {"command": "maskrun get x"}})
        result = run_cli(["hook"], stdin_data=payload.encode())
        self.assertEqual(result.returncode, 0)
        decision = json.loads(result.stdout.decode())
        self.assertEqual(
            decision["hookSpecificOutput"]["permissionDecision"], "deny")

    def test_cli_hook_stays_silent_when_allowed(self):
        payload = json.dumps(
            {"tool_name": "Bash", "tool_input": {"command": "maskrun status"}})
        result = run_cli(["hook"], stdin_data=payload.encode())
        self.assertEqual(result.returncode, 0)
        self.assertEqual(result.stdout.decode().strip(), "")

    def test_cli_hook_never_blocks_on_garbage(self):
        result = run_cli(["hook"], stdin_data=b"not json at all")
        self.assertEqual(result.returncode, 0)
        self.assertEqual(result.stdout.decode().strip(), "")

    def test_hook_needs_no_keyring(self):
        payload = json.dumps({"tool_name": "Bash", "tool_input": {"command": "ls"}})
        result = run_cli(["hook"], stdin_data=payload.encode(),
                         env_extra={"MASKRUN_BACKEND": "secret-service",
                                    "PATH": os.path.dirname(PY)})
        self.assertEqual(result.returncode, 0)


class TestUnits(unittest.TestCase):
    """Small pure functions worth pinning down."""

    def test_variants_skip_short_values(self):
        self.assertEqual(list(mr.value_variants("abc")), [])

    def test_variants_include_encodings(self):
        variants = list(mr.value_variants("secret-value-here"))
        self.assertIn("secret-value-here", variants)
        self.assertIn(base64.b64encode(b"secret-value-here").decode(), variants)

    def test_filter_is_none_when_nothing_maskable(self):
        self.assertIsNone(mr.build_filter([("A", "abc")]))

    def test_strip_heredocs_removes_body(self):
        command = "cat <<'EOF' > f\nsecret .env line\nEOF\nls"
        self.assertNotIn("secret .env line", mr.strip_heredocs(command))
        self.assertIn("ls", mr.strip_heredocs(command))

    def test_drop_pattern_argument(self):
        # grep -E "\.env"  ->  no file arguments left
        self.assertEqual(mr.drop_pattern_argument(["-E", "\\.env"]), [])
        # grep DATABASE .env  ->  .env survives as a file argument
        self.assertEqual(mr.drop_pattern_argument(["DATABASE", ".env"]), [".env"])

    def test_segment_command_skips_env_prefix_and_sudo(self):
        self.assertEqual(mr.segment_command(["FOO=1", "sudo", "cat", "x"]), "cat")

    def test_env_line_parsing_unwraps_quotes(self):
        with tempfile.NamedTemporaryFile("w", suffix=".env", delete=False) as fh:
            fh.write('A="quoted"\nB=\'single\'\n# comment\nexport C=plain\nD=\n')
            path = fh.name
        try:
            parsed = dict(mr.parse_env_file(path))
        finally:
            os.unlink(path)
        self.assertEqual(parsed["A"], "quoted")
        self.assertEqual(parsed["B"], "single")
        self.assertEqual(parsed["C"], "plain")
        self.assertEqual(parsed["D"], "")

    def test_manifest_parsing_rejects_bad_line(self):
        with tempfile.NamedTemporaryFile("w", delete=False) as fh:
            fh.write("no equals here\n")
            path = fh.name
        try:
            with self.assertRaises(mr.MaskrunError):
                mr.read_manifest(path)
        finally:
            os.unlink(path)


def main():
    # CI uses --require-keyring so a silently skipped backend cannot pass as green.
    require = "--require-keyring" in sys.argv
    if require:
        sys.argv.remove("--require-keyring")

    print("maskrun test suite")
    print("  python:  %s" % sys.version.split()[0])
    print("  platform: %s" % __import__("platform").system())
    print("  keyring tests: %s" % ("ENABLED" if KEYRING else "SKIPPED (no backend)"))
    if KEYRING:
        print("  backend: %s" % mr.pick_backend().name)
    print()
    if require and not KEYRING:
        print("FAIL: --require-keyring was given but no keyring backend is "
              "reachable, so the keyring tests would be skipped.")
        return 1
    unittest.main(argv=[sys.argv[0]] + sys.argv[1:], verbosity=2)


if __name__ == "__main__":
    sys.exit(main() or 0)
