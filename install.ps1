# maskrun installer (Windows)
#
#   irm https://raw.githubusercontent.com/furkanakyol/maskrun/main/install.ps1 | iex
#
# Options, as environment variables (a piped script cannot prompt):
#   $env:MASKRUN_BIN        where to install  (default: %LOCALAPPDATA%\maskrun\bin)
#   $env:MASKRUN_REF        git ref to fetch  (default: main)
#   $env:MASKRUN_WITH_GUARD set to 1 to also register the Claude Code guard hook
#
# Installs maskrun plus a maskrun.cmd shim, so `maskrun` works from any shell.

$ErrorActionPreference = 'Stop'

$repo   = 'furkanakyol/maskrun'
$ref    = if ($env:MASKRUN_REF) { $env:MASKRUN_REF } else { 'main' }
$binDir = if ($env:MASKRUN_BIN) { $env:MASKRUN_BIN } else { Join-Path $env:LOCALAPPDATA 'maskrun\bin' }
$source = "https://raw.githubusercontent.com/$repo/$ref/bin/maskrun"
$target = Join-Path $binDir 'maskrun'
$shim   = Join-Path $binDir 'maskrun.cmd'

function Fail($message) { Write-Error "error: $message"; exit 1 }

# --- python ----------------------------------------------------------------
$python = $null
foreach ($candidate in @('python', 'python3', 'py')) {
    $found = Get-Command $candidate -ErrorAction SilentlyContinue
    if (-not $found) { continue }
    try {
        & $found.Source -c 'import sys; sys.exit(0 if sys.version_info >= (3,9) else 1)' 2>$null
        if ($LASTEXITCODE -eq 0) { $python = $found.Source; break }
    } catch { }
}
if (-not $python) {
    Fail @"
Python 3.9 or newer is required but was not found on PATH.
  winget install Python.Python.3.12
  or download from https://www.python.org/downloads/windows/
Make sure "Add python.exe to PATH" is checked during installation.
"@
}

# --- download --------------------------------------------------------------
New-Item -ItemType Directory -Force -Path $binDir | Out-Null
$tmp = [System.IO.Path]::GetTempFileName()
try {
    Invoke-WebRequest -Uri $source -OutFile $tmp -UseBasicParsing
} catch {
    Fail "download failed: $source`n$($_.Exception.Message)"
}

$firstLine = (Get-Content $tmp -TotalCount 1)
if ($firstLine -notmatch '^#!/usr/bin/env python3') {
    Remove-Item $tmp -Force
    Fail "downloaded file does not look like maskrun (wrong ref '$ref'?)"
}
& $python -c "import ast,sys; ast.parse(open(sys.argv[1], encoding='utf-8').read())" $tmp
if ($LASTEXITCODE -ne 0) {
    Remove-Item $tmp -Force
    Fail "downloaded file is not valid Python - refusing to install it"
}

Move-Item -Force $tmp $target

# A .cmd shim so `maskrun ...` works without typing `python maskrun`.
# %* forwards every argument; the exit code is propagated for CI and scripts.
@"
@echo off
"$python" "$target" %*
exit /b %errorlevel%
"@ | Set-Content -Path $shim -Encoding ASCII

$version = & $python $target --version
Write-Host "installed $version -> $target"
Write-Host "shim: $shim"

# --- PATH ------------------------------------------------------------------
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($userPath -notlike "*$binDir*") {
    Write-Host ""
    Write-Host "warning: $binDir is not on your PATH." -ForegroundColor Yellow
    Write-Host "Add it for future sessions with:"
    Write-Host "    [Environment]::SetEnvironmentVariable('Path', `"$binDir;`" + [Environment]::GetEnvironmentVariable('Path','User'), 'User')"
}

# --- optional guard --------------------------------------------------------
if ($env:MASKRUN_WITH_GUARD -eq '1') {
    Write-Host ""
    & $python $target install-guard --command-path $shim
}

Write-Host ""
Write-Host "Next:"
Write-Host "  maskrun put myapp-database-url      store a secret (prompts, not echoed)"
Write-Host "  maskrun import .env --dry-run       see what would move out of a .env"
Write-Host "  maskrun run -- npm run dev          run with secrets injected, output masked"
Write-Host ""
Write-Host "Using an AI coding agent? Register the guard so it cannot print your secrets:"
Write-Host "  maskrun install-guard"
