# SCIP-IO CLI installer (Windows)
#
# Usage:
#   irm https://github.com/GlitterKill/scip-io/releases/latest/download/install.ps1 | iex
#   irm https://github.com/GlitterKill/scip-io/releases/download/v0.1.0/install.ps1 | iex
#
# Environment variables:
#   $env:SCIP_IO_VERSION      — tag to install (default: latest)
#   $env:SCIP_IO_INSTALL_DIR  — install location (default: %LOCALAPPDATA%\scip-io\bin)

$ErrorActionPreference = "Stop"

$Repo = "GlitterKill/scip-io"
$Version = if ($env:SCIP_IO_VERSION) { $env:SCIP_IO_VERSION } else { "latest" }
$InstallDir = if ($env:SCIP_IO_INSTALL_DIR) {
    $env:SCIP_IO_INSTALL_DIR
} else {
    Join-Path $env:LOCALAPPDATA "scip-io\bin"
}

function Write-Info($msg) { Write-Host "==> " -ForegroundColor Cyan -NoNewline; Write-Host $msg }
function Write-Ok($msg)   { Write-Host " ok " -ForegroundColor Green -NoNewline; Write-Host $msg }
function Write-Err($msg)  { Write-Host "error: " -ForegroundColor Red -NoNewline; Write-Host $msg; exit 1 }

# ---------- detect architecture ----------
$Arch = (Get-CimInstance Win32_Processor | Select-Object -First 1).Architecture
switch ($Arch) {
    9 { $Target = "x86_64-pc-windows-msvc" }    # x64
    12 { Write-Err "Windows on ARM64 is not yet published. Please build from source." }
    default { Write-Err "unsupported architecture: $Arch" }
}

Write-Info "Detected platform: $Target"

# ---------- resolve version ----------
if ($Version -eq "latest") {
    Write-Info "Resolving latest release..."
    try {
        $Release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" -UseBasicParsing
        $Version = $Release.tag_name
    } catch {
        Write-Err "could not resolve latest release tag: $_"
    }
}

Write-Info "Installing scip-io $Version"

# ---------- download ----------
$ArchiveName = "scip-io-$Version-$Target.zip"
$Url = "https://github.com/$Repo/releases/download/$Version/$ArchiveName"

$TempDir = Join-Path $env:TEMP ("scip-io-install-" + [Guid]::NewGuid())
New-Item -ItemType Directory -Force -Path $TempDir | Out-Null
$ArchivePath = Join-Path $TempDir $ArchiveName

try {
    Write-Info "Downloading $Url"
    Invoke-WebRequest -Uri $Url -OutFile $ArchivePath -UseBasicParsing

    Write-Info "Extracting..."
    Expand-Archive -Path $ArchivePath -DestinationPath $TempDir -Force

    # ---------- install ----------
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null

    $BinSrc = Join-Path $TempDir "scip-io-$Version-$Target\scip-io.exe"
    $BinDst = Join-Path $InstallDir "scip-io.exe"

    if (-not (Test-Path $BinSrc)) {
        Write-Err "expected binary not found in archive: $BinSrc"
    }

    Move-Item -Force -Path $BinSrc -Destination $BinDst
    Write-Ok "Installed to $BinDst"
} finally {
    Remove-Item -Recurse -Force $TempDir -ErrorAction SilentlyContinue
}

# ---------- PATH hint ----------
$UserPath = [Environment]::GetEnvironmentVariable("PATH", "User")
if ($UserPath -notlike "*$InstallDir*") {
    Write-Host ""
    Write-Host "note: " -ForegroundColor Yellow -NoNewline
    Write-Host "$InstallDir is not on your user PATH."
    Write-Host "Adding it now..."
    $NewPath = if ([string]::IsNullOrEmpty($UserPath)) { $InstallDir } else { "$UserPath;$InstallDir" }
    [Environment]::SetEnvironmentVariable("PATH", $NewPath, "User")
    Write-Ok "Added to PATH. Open a new terminal for changes to take effect."
}

Write-Host ""
Write-Host "Run '" -NoNewline
Write-Host "scip-io --help" -ForegroundColor Cyan -NoNewline
Write-Host "' to get started."
