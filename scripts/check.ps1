# Runs the checks of .github/workflows/ci.yml on this machine, with the same
# commands, flags and toolchains, so a change that passes here passes CI.
# .githooks/pre-push runs it before every push.
#
#   scripts/check.ps1          the jobs whose files changed since origin/main
#   scripts/check.ps1 -All     both jobs, whatever changed
#
# The root job runs on Linux in CI. On Windows its Linux half runs in WSL
# (scripts/check-linux.sh), and the root workspace's Windows-only crates are
# linted and tested here, as CI's client job does. The client job needs
# Windows. The checks read the working tree, so commit or stash what isn't
# being pushed.
#
# Build output stays in each workspace's target directory. CARGO_BUILD_JOBS
# defaults to 4.
param(
    [switch]$All,
    # The commit the changes are measured from. The pre-push hook passes the
    # remote branch's commit.
    [string]$Base = 'origin/main'
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
if (-not $env:CARGO_BUILD_JOBS) { $env:CARGO_BUILD_JOBS = '4' }
$onWindows = [Environment]::OSVersion.Platform -eq 'Win32NT'

function Invoke-Step {
    param([string]$Name, [string]$Dir, [string[]]$Command)
    Write-Host "==> $Name" -ForegroundColor Cyan
    Write-Host "    ($Dir) $($Command -join ' ')"
    Push-Location $Dir
    try {
        & $Command[0] $Command[1..($Command.Length - 1)]
        $code = $LASTEXITCODE
    } finally {
        Pop-Location
    }
    if ($code -ne 0) {
        Write-Host "FAILED: $Name (exit $code)" -ForegroundColor Red
        exit 1
    }
}

# Files that differ from the merge base with $Base, committed or not. CI
# ignores Markdown and LICENSE.
function Get-ChangedFiles {
    # Windows PowerShell turns a native command's stderr into an error, which
    # 'Stop' would make fatal. An unknown $Base is expected here.
    $ErrorActionPreference = 'Continue'
    $mb = git -C $repo merge-base $Base HEAD 2>$null
    if ($LASTEXITCODE -ne 0 -or -not $mb) {
        $mb = git -C $repo merge-base origin/main HEAD
        if ($LASTEXITCODE -ne 0) { throw "can't find a merge base with $Base or origin/main" }
    }
    $files = @(git -C $repo diff --name-only $mb) + @(git -C $repo ls-files --others --exclude-standard)
    $files | Where-Object { $_ -and $_ -notmatch '\.md$' -and $_ -ne 'LICENSE' } | Sort-Object -Unique
}

$root = $All
$client = $All
if (-not $All) {
    $changed = @(Get-ChangedFiles)
    if ($changed.Count -eq 0) {
        Write-Host 'No change CI checks (Markdown and LICENSE are ignored).'
        exit 0
    }
    foreach ($f in $changed) {
        if ($f -like '.github/*' -or $f -like 'scripts/*') { $root = $true; $client = $true }
        elseif ($f -like 'client/*') { $client = $true }
        else {
            $root = $true
            # client/ builds on the root crates and reads the root's lint
            # configuration.
            if ($f -like 'crates/*' -or $f -eq 'clippy.toml' -or $f -eq 'rustfmt.toml') { $client = $true }
        }
    }
}
$jobs = @()
if ($root) { $jobs += 'root' }
if ($client) { $jobs += 'client' }
Write-Host "Jobs: $($jobs -join ', ')"

$clientDir = Join-Path $repo 'client'

# Formatting first: it takes seconds and fails most often.
if ($root) { Invoke-Step 'root: Format' $repo @('cargo', 'fmt', '--all', '--check') }
if ($client) { Invoke-Step 'client: Format' $clientDir @('cargo', 'fmt', '--all', '--check') }

if ($root) {
    if ($onWindows) {
        $wsl = Get-Command wsl.exe -ErrorAction SilentlyContinue
        if (-not $wsl) {
            throw 'The root job runs on Linux and needs WSL here (wsl --install). Without it, Linux-only code is not checked.'
        }
        $linuxRepo = (wsl.exe -e wslpath -a ($repo -replace '\\', '/')).Trim()
        Invoke-Step 'root: Linux (WSL)' $repo @('wsl.exe', '-e', 'env', "CARGO_BUILD_JOBS=$env:CARGO_BUILD_JOBS", 'bash', "$linuxRepo/scripts/check-linux.sh", '--mirror', '--no-fmt')
        $crates = @('-p', 'rekindle-launcher', '-p', 'rekindle-hook', '-p', 'phoenix-guest')
        Invoke-Step 'root: Windows-only crates, Clippy' $repo (@('cargo', 'clippy') + $crates + @('--all-targets', '--locked', '--', '-D', 'warnings'))
        Invoke-Step 'root: Windows-only crates, Test' $repo (@('cargo', 'test') + $crates + @('--locked'))
    } else {
        Invoke-Step 'root: Linux' $repo @('bash', (Join-Path $repo 'scripts/check-linux.sh'), '--no-fmt')
    }
}

if ($client) {
    if (-not $onWindows) {
        Write-Warning 'The client job runs on Windows only. It was not run.'
    } else {
        Invoke-Step 'client: Build' $clientDir @('cargo', 'build', '--workspace', '--locked')
        Invoke-Step 'client: Clippy' $clientDir @('cargo', 'clippy', '--workspace', '--all-targets', '--locked', '--', '-D', 'warnings')
        Invoke-Step 'client: Test phoenix-view' $clientDir @('cargo', 'test', '-p', 'phoenix-view', '--locked')
        Invoke-Step 'client: Build phoenix-view for release' $clientDir @('cargo', 'build', '-p', 'phoenix-view', '--release', '--locked')
    }
}

Write-Host "All checks passed ($($jobs -join ', '))." -ForegroundColor Green
