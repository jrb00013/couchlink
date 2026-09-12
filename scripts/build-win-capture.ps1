# Ensure couchlink-win-capture.exe exists (build on Windows if missing/stale).
#
# Fully automated  -  no manual setup should ever be required on a fresh
# machine. Three things this handles on its own that a bare `cargo build`
# does not:
#
#   1. CMake: the vendored Opus C library (via `audiopus_sys`, pulled in for
#      WASAPI loopback → Opus encode) needs CMake to build. Most Windows
#      installs don't have it, and the normal installer (`winget`/MSI) pops a
#      UAC prompt that a script cannot click through. `pip install --user
#      cmake` ships a real cmake.exe with no elevation needed, so that is the
#      first thing tried; the venv-less system Python's user Scripts dir is
#      added to PATH for this process only.
#   2. CMAKE_POLICY_VERSION_MINIMUM: the vendored Opus CMakeLists.txt predates
#      CMake 4's removal of support for `cmake_minimum_required` versions
#      below 3.5  -  recent CMake refuses to configure at all without this set.
#   3. UNC build directory: when this repo is opened over `\\wsl.localhost\...`
#      (the normal way this script reaches it from WSL), CMake's Visual Studio
#      generator cannot emit a working `install` target against a UNC output
#      path  -  MSBuild fails with "Project file does not exist: install.vcxproj".
#      Building to a local target dir (`%LOCALAPPDATA%\couchlink\build-target`)
#      sidesteps it entirely; only the *output* moves, sources still build from
#      the UNC-mounted checkout.
param(
    [switch]$Force
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)

function Ensure-CMake {
    if (Get-Command cmake -ErrorAction SilentlyContinue) {
        return
    }
    Write-Host "==> cmake not found  -  installing via pip (no admin needed; libopus build dep)"
    $py = Get-Command py -ErrorAction SilentlyContinue
    if (-not $py) {
        $py = Get-Command python -ErrorAction SilentlyContinue
    }
    if (-not $py) {
        throw "cmake is required (libopus build dep) and no Python was found to install it via pip. " +
            "Install CMake manually (https://cmake.org/download) or install Python, then retry."
    }
    & $py.Source -m pip install --user --quiet cmake
    if ($LASTEXITCODE -ne 0) {
        throw "pip install cmake failed with exit code $LASTEXITCODE"
    }
    # pip's console-script dir for a per-user install  -  not on PATH by default.
    # `site.USER_BASE` alone omits the per-version folder pip actually installs
    # into on Windows (e.g. ...\Python\Python311\Scripts) - ask sysconfig for
    # the real path instead of reconstructing it.
    $scripts = (& $py.Source -c "import sysconfig; print(sysconfig.get_path('scripts', 'nt_user'))").Trim()
    if (Test-Path $scripts) {
        $env:PATH = "$env:PATH;$scripts"
    }
    if (-not (Get-Command cmake -ErrorAction SilentlyContinue)) {
        throw "cmake still not on PATH after pip install  -  expected it under $scripts"
    }
    Write-Host "==> cmake ready: $((Get-Command cmake).Source)"
}

Ensure-CMake
# See file header note 2  -  required every build, not just the first, since
# it's a build-script env var rather than a one-time install step.
$env:CMAKE_POLICY_VERSION_MINIMUM = "3.5"

# See file header note 3. UNC detection: a drive-qualified path (C:\...) is
# never UNC; anything else reaching here from `wslpath -w` is.
if (-not $env:CARGO_TARGET_DIR -and $Root -match '^\\\\') {
    $localTarget = Join-Path $env:LOCALAPPDATA "couchlink\build-target"
    Write-Host "==> building over a UNC path  -  redirecting cargo target dir to $localTarget"
    $env:CARGO_TARGET_DIR = $localTarget
}
$TargetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $Root "target" }
$Bin = Join-Path $TargetDir "release\couchlink-win-capture.exe"

$needBuild = $Force -or -not (Test-Path $Bin)
if (-not $needBuild) {
    $binTime = (Get-Item $Bin).LastWriteTimeUtc
    $srcDir = Join-Path $Root "crates\capture-bridge"
    $newestSrc = Get-ChildItem -Path $srcDir -Recurse -File -Include *.rs,Cargo.toml |
        Sort-Object LastWriteTimeUtc -Descending |
        Select-Object -First 1
    if ($newestSrc -and $newestSrc.LastWriteTimeUtc -gt $binTime) {
        $needBuild = $true
    }
}

if ($needBuild) {
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        throw "cargo not found on Windows - install Rust (https://rustup.rs) with the MSVC toolchain, then reopen your terminal"
    }
    Write-Host "==> building couchlink-win-capture.exe (Windows DXGI / Graphics Capture + WASAPI loopback audio)"
    Push-Location $Root
    try {
        cargo build -p couchlink-capture-bridge --bin couchlink-win-capture --release
        if ($LASTEXITCODE -ne 0) {
            throw "cargo build failed with exit code $LASTEXITCODE"
        }
    } finally {
        Pop-Location
    }
}

if (-not (Test-Path $Bin)) {
    throw "couchlink-win-capture.exe missing after build: $Bin"
}

Write-Host "==> ready: $Bin"
$Bin
