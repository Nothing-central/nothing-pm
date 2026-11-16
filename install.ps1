# A `#Requires` line is unreliable when the script is piped into iex,
# so the version is checked at runtime instead.
if ($PSVersionTable.PSVersion.Major -lt 5) {
    Write-Host "error: PowerShell 5.1 or newer is required" -ForegroundColor Red
    exit 1
}
<#
.SYNOPSIS
    Installs the nth (nothing) package manager on Windows.

.DESCRIPTION
    Native PowerShell installer — no Git Bash required.

        irm https://raw.githubusercontent.com/Nothing-central/nothing-pm/main/install.ps1 | iex

    Downloads the prebuilt nth.exe for this machine, checks it against the
    release's SHA256SUMS, installs it to %LOCALAPPDATA%\nothing\bin and adds
    that directory to your user PATH.

.PARAMETER Version
    Release tag to install, e.g. v0.1.1. Defaults to the latest release.

.PARAMETER InstallDir
    Directory to install nth.exe into. Defaults to %LOCALAPPDATA%\nothing\bin.

.PARAMETER NoVerify
    Skip checksum verification (not recommended).

.PARAMETER NoPath
    Do not touch the user PATH.
#>
[CmdletBinding()]
param(
    [string]$Version = "",
    [string]$InstallDir = "",
    [switch]$NoVerify,
    [switch]$NoPath
)

$ErrorActionPreference = "Stop"
$Repo = "Nothing-central/nothing-pm"

# Windows PowerShell 5.1 still defaults to TLS 1.0 against GitHub.
try {
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
}
catch {
    # newer PowerShell already negotiates TLS 1.2+
}

function Write-Say {
    param([string]$Message)
    Write-Host $Message
}

function Fail {
    param([string]$Message)
    Write-Host "error: $Message" -ForegroundColor Red
    exit 1
}

# --- what are we on? ---------------------------------------------------
switch ($env:PROCESSOR_ARCHITECTURE) {
    "AMD64" { $TargetArch = "x86_64" }
    "ARM64" { Fail "no prebuilt nth for windows-arm64 yet - build it with: cargo build --release" }
    default { Fail "unsupported architecture: $env:PROCESSOR_ARCHITECTURE" }
}

$Asset = "nth-windows-$TargetArch.zip"

# --- which release? ----------------------------------------------------
if (-not $Version) {
    Write-Say "looking up the latest release..."
    try {
        $Release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" -UseBasicParsing
        $Version = $Release.tag_name
    }
    catch {
        Fail "could not determine the latest release - pass -Version v0.1.1"
    }
}
if (-not $Version) {
    Fail "could not determine the latest release - pass -Version v0.1.1"
}

$Base = "https://github.com/$Repo/releases/download/$Version"
$Scratch = Join-Path ([System.IO.Path]::GetTempPath()) ("nth-" + [Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force -Path $Scratch | Out-Null

try {
    $Zip = Join-Path $Scratch $Asset
    Write-Say "downloading $Asset ($Version)..."
    try {
        Invoke-WebRequest -Uri "$Base/$Asset" -OutFile $Zip -UseBasicParsing
    }
    catch {
        Fail "failed to download $Base/$Asset"
    }

    if (-not $NoVerify) {
        $SumsPath = Join-Path $Scratch "SHA256SUMS"
        $HaveSums = $true
        try {
            Invoke-WebRequest -Uri "$Base/SHA256SUMS" -OutFile $SumsPath -UseBasicParsing
        }
        catch {
            $HaveSums = $false
            Write-Warning "SHA256SUMS not available for $Version - skipping checksum verification"
        }

        if ($HaveSums) {
            $Expected = $null
            foreach ($Line in Get-Content $SumsPath) {
                if ($Line -match '^\s*([0-9a-fA-F]{64})\s+\*?(.+?)\s*$') {
                    # sha256sum may write "./name" or "*name"
                    $Name = $Matches[2] -replace '^\./', '' -replace '^.*/', ''
                    if ($Name -eq $Asset) {
                        $Expected = $Matches[1].ToLower()
                        break
                    }
                }
            }
            if ($Expected) {
                $Actual = (Get-FileHash -Path $Zip -Algorithm SHA256).Hash.ToLower()
                if ($Actual -ne $Expected) {
                    Fail "checksum mismatch for $Asset (expected $Expected, got $Actual)"
                }
            }
            else {
                Write-Warning "no checksum listed for $Asset - skipping verification"
            }
        }
    }

    Write-Say "unpacking..."
    $Extracted = Join-Path $Scratch "x"
    Expand-Archive -Path $Zip -DestinationPath $Extracted -Force

    $Exe = Get-ChildItem -Path $Extracted -Filter "nth.exe" -Recurse | Select-Object -First 1
    if (-not $Exe) {
        Fail "archive did not contain nth.exe"
    }

    if (-not $InstallDir) {
        $InstallDir = Join-Path $env:LOCALAPPDATA "nothing\bin"
    }
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    $Target = Join-Path $InstallDir "nth.exe"
    Copy-Item -Path $Exe.FullName -Destination $Target -Force

    $InstalledVersion = $null
    try {
        $InstalledVersion = (& $Target --version) 2>$null
    }
    catch {
        # a working install that refuses --version is still worth reporting
    }

    Write-Say ""
    if ($InstalledVersion) {
        Write-Say "installed $InstalledVersion to $Target"
    }
    else {
        Write-Say "installed nth to $Target"
    }

    if (-not $NoPath) {
        $UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
        if (-not $UserPath) { $UserPath = "" }
        $Entries = @($UserPath -split ';' | Where-Object { $_ -ne "" })

        if ($Entries -notcontains $InstallDir) {
            if ($UserPath.TrimEnd(';')) {
                $NewPath = "$($UserPath.TrimEnd(';'));$InstallDir"
            }
            else {
                $NewPath = $InstallDir
            }
            [Environment]::SetEnvironmentVariable("Path", $NewPath, "User")
            Write-Say ""
            Write-Say "added $InstallDir to your user PATH - open a new terminal to use nth"
        }
    }
}
finally {
    Remove-Item -Path $Scratch -Recurse -Force -ErrorAction SilentlyContinue
}
