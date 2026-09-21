# maskrun installer (Windows)
#
#   irm https://raw.githubusercontent.com/furkanakyol/maskrun/main/install.ps1 | iex
#
# Options, as environment variables (a piped script cannot prompt):
#   $env:MASKRUN_BIN         where to install    (default: %LOCALAPPDATA%\maskrun\bin)
#   $env:MASKRUN_VERSION     release to install   (default: latest)
#   $env:MASKRUN_WITH_GUARD  set to 1 to also register the Claude Code guard hook
#   $env:MASKRUN_BASE_URL    release base URL, for testing against a mirror
#                            (default: https://github.com/furkanakyol/maskrun/releases)
#
# Installs one maskrun.exe. To remove it: Remove-Item "$env:MASKRUN_BIN\maskrun.exe"

$ErrorActionPreference = 'Stop'

$repo    = 'furkanakyol/maskrun'
$binDir  = if ($env:MASKRUN_BIN) { $env:MASKRUN_BIN } else { Join-Path $env:LOCALAPPDATA 'maskrun\bin' }
$baseUrl = if ($env:MASKRUN_BASE_URL) { $env:MASKRUN_BASE_URL } else { "https://github.com/$repo/releases" }
$apiUrl  = "https://api.github.com/repos/$repo/releases/latest"
# Only Windows target maskrun ships; x86_64 binaries run fine under ARM64's emulator.
$target  = 'x86_64-pc-windows-msvc'
$exe     = Join-Path $binDir 'maskrun.exe'

function Fail($message) { Write-Error "error: $message"; exit 1 }

$version = $env:MASKRUN_VERSION
if (-not $version) {
    try {
        $release = Invoke-RestMethod -Uri $apiUrl -UseBasicParsing
        $version = $release.tag_name
    } catch {
        Fail "could not reach $apiUrl to find the latest version`n$($_.Exception.Message)"
    }
    if (-not $version) { Fail "could not parse a version out of $apiUrl" }
}

$asset   = "maskrun-$version-$target.zip"
$workDir = Join-Path ([System.IO.Path]::GetTempPath()) ([System.Guid]::NewGuid().ToString())
New-Item -ItemType Directory -Force -Path $workDir | Out-Null
$archive = Join-Path $workDir $asset
$sums    = Join-Path $workDir 'SHA256SUMS'

try {
    try {
        Invoke-WebRequest -Uri "$baseUrl/download/$version/$asset" -OutFile $archive -UseBasicParsing
        Invoke-WebRequest -Uri "$baseUrl/download/$version/SHA256SUMS" -OutFile $sums -UseBasicParsing
    } catch {
        Fail "download failed: $($_.Exception.Message)"
    }

    # The one thing standing between a tampered or truncated download and a
    # secret manager landing on disk. Not optional, not warn-and-continue.
    $expectedLine = Select-String -Path $sums -Pattern ([regex]::Escape($asset)) | Select-Object -First 1
    if (-not $expectedLine) { Fail "SHA256SUMS has no entry for $asset" }
    $expected = ($expectedLine.Line -split '\s+')[0].TrimStart('*')
    $actual = (Get-FileHash -Path $archive -Algorithm SHA256).Hash
    if ($expected.ToLower() -ne $actual.ToLower()) {
        Fail "checksum mismatch for $asset`n  expected: $expected`n  actual:   $actual`nThe download is corrupted or was tampered with. Not installing."
    }

    Expand-Archive -Path $archive -DestinationPath $workDir -Force
    New-Item -ItemType Directory -Force -Path $binDir | Out-Null
    Copy-Item -Path (Join-Path $workDir 'maskrun.exe') -Destination $exe -Force
} finally {
    Remove-Item -Recurse -Force $workDir -ErrorAction SilentlyContinue
}

$versionOutput = & $exe --version
if ($LASTEXITCODE -ne 0) { Fail "installed but did not run: $exe --version" }
Write-Host "installed $versionOutput -> $exe"

$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($userPath -notlike "*$binDir*") {
    Write-Host ""
    Write-Host "warning: $binDir is not on your PATH." -ForegroundColor Yellow
    Write-Host "Add it for future sessions with:"
    Write-Host "    [Environment]::SetEnvironmentVariable('Path', `"$binDir;`" + [Environment]::GetEnvironmentVariable('Path','User'), 'User')"
}

if ($env:MASKRUN_WITH_GUARD -eq '1') {
    Write-Host ""
    & $exe install-guard
}

Write-Host ""
Write-Host "Next:"
Write-Host "  maskrun put myapp-database-url      store a secret (prompts, not echoed)"
Write-Host "  maskrun import .env --dry-run       see what would move out of a .env"
Write-Host "  maskrun run -- npm run dev          run with secrets injected, output masked"
Write-Host ""
Write-Host "Using an AI coding agent? Register the guard so it cannot print your secrets:"
Write-Host "  maskrun install-guard"
