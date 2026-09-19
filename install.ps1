param(
    [string]$InstallRoot = $env:DRAFT_INSTALL_ROOT,
    [string]$InstallDir = $env:DRAFT_INSTALL_DIR,
    [string]$Version = $env:DRAFT_VERSION,
    [string]$Repo = $(if ($env:DRAFT_REPO) { $env:DRAFT_REPO } else { "Shiva936/draft" }),
    [switch]$UpdatePath
)

# Draft installer for native Windows PowerShell.
#
# Trust: this installer verifies the downloaded archive over HTTPS against the
# release's published SHA256SUMS (stage 1). It does not verify the signed
# release manifest; that stage-2 trust applies to `draft update` once Draft
# is installed.
#
# Mutation: this script never writes the installation. It routes, downloads,
# verifies and extracts, then launches a Rust lifecycle actor, which takes
# <install_root>\.draft-install\lifecycle.lock, re-reads the state under it,
# and performs every change — including the User PATH entry.

$ErrorActionPreference = "Stop"

function Fail($Message) {
    Write-Error "draft install: $Message"
    exit 1
}

if ([System.Environment]::OSVersion.Platform -ne [System.PlatformID]::Win32NT) {
    Fail "install.ps1 supports native Windows PowerShell only. Use install.sh on Linux, macOS, or WSL."
}

# Root selection. DRAFT_INSTALL_DIR keeps its meaning (the bin directory);
# alone it selects its parent as the root, and with DRAFT_INSTALL_ROOT the two
# must agree.
if ($InstallRoot -and $InstallDir) {
    if ($InstallDir.TrimEnd("\") -ne (Join-Path $InstallRoot "bin").TrimEnd("\")) {
        Fail "DRAFT_INSTALL_DIR ($InstallDir) must be DRAFT_INSTALL_ROOT\bin ($InstallRoot\bin)"
    }
} elseif ($InstallDir) {
    $InstallRoot = Split-Path -Parent $InstallDir.TrimEnd("\")
} elseif (-not $InstallRoot) {
    $InstallRoot = Join-Path $env:LOCALAPPDATA "Programs\Draft"
}
if (-not [System.IO.Path]::IsPathRooted($InstallRoot) -or $InstallRoot.StartsWith("\\")) {
    Fail "the installation root must be an absolute local path: $InstallRoot"
}

$envUpdatePath = $env:DRAFT_UPDATE_PATH
if ($envUpdatePath -and ($envUpdatePath -eq "1" -or $envUpdatePath.ToLowerInvariant() -eq "true")) {
    $UpdatePath = $true
}

$arch = $env:PROCESSOR_ARCHITECTURE
switch ($arch) {
    "AMD64" { $Target = "x86_64-pc-windows-msvc" }
    default { Fail "unsupported Windows CPU architecture for v0.3.x: $arch" }
}

$Lifecycle = Join-Path $InstallRoot ".draft-install"
$InstalledDraft = Join-Path $InstallRoot "bin\draft.exe"
$Journal = Join-Path $Lifecycle "operation.json"
$Bootstrap = Join-Path $Lifecycle "bootstrap.recovery"
$Ready = Join-Path $Lifecycle "terminal-cleanup"

# --- Lifecycle preflight -------------------------------------------------
# Fixed-slot existence and the bounded bootstrap.recovery grammar only. This
# never parses operation.json and never infers an operation kind.

function Read-BootstrapRecord($Path) {
    $bytes = [System.IO.File]::ReadAllBytes($Path)
    if ($bytes.Length -gt 512) { return $null }
    $text = [System.Text.Encoding]::ASCII.GetString($bytes)
    if (-not $text.EndsWith("`n")) { return $null }
    $lines = $text.Substring(0, $text.Length - 1).Split("`n")
    if ($lines.Count -ne 6) { return $null }
    $clean = @()
    foreach ($line in $lines) {
        if ($line.EndsWith("`r")) { $line = $line.Substring(0, $line.Length - 1) }
        if ($line.Length -gt 128 -or $line -cmatch '[^\x20-\x7e]') { return $null }
        $clean += $line
    }
    if ($clean[0] -cne "draft-lifecycle-bootstrap 1") { return $null }
    if ($clean[1] -cnotmatch '^installation ins_[0-9a-f]{12}$') { return $null }
    if ($clean[2] -cnotmatch '^operation ilo_[0-9a-f]{12}$') { return $null }
    if ($clean[3] -cne "kind uninstall") { return $null }
    if ($clean[4] -cnotmatch '^helper-sha256 [0-9a-f]{64}$') { return $null }
    if ($clean[5] -cnotmatch '^helper-size [0-9]+$') { return $null }
    return @{
        Installation = $clean[1].Substring(13)
        Operation = $clean[2].Substring(10)
        Sha256 = $clean[4].Substring(14)
        Size = [UInt64]$clean[5].Substring(12)
    }
}

function Stop-Unavailable($Why, $Record) {
    Write-Host "draft install: an unfinished uninstall at $InstallRoot cannot continue: $Why"
    if ($Record) {
        Write-Host "  installation $($Record.Installation), operation $($Record.Operation)"
        Write-Host "  helper slot: $Lifecycle\staging\$($Record.Operation)\draft.exe (expected sha256 $($Record.Sha256), size $($Record.Size))"
    }
    Write-Host "  This lifecycle state requires manual lifecycle repair/support because the"
    Write-Host "  operation's previously authorized executor is unavailable."
    Write-Host "  Do NOT delete lifecycle.lock; do NOT delete operation.json; do NOT"
    Write-Host "  recursively remove .draft-install\; do NOT delete the installation root."
    exit 1
}

function Get-Route {
    $hasJournal = Test-Path -LiteralPath $Journal -PathType Leaf
    $hasBootstrap = Test-Path -LiteralPath $Bootstrap -PathType Leaf
    if ($hasJournal -and $hasBootstrap) { return "A" }
    if ($hasJournal -and (Test-Path -LiteralPath $InstalledDraft -PathType Leaf)) { return "B" }
    if ($hasJournal) { return "C" }
    if ($hasBootstrap -and -not (Test-Path -LiteralPath $Ready -PathType Leaf)) { return "E" }
    return "F"
}

switch (Get-Route) {
    "A" {
        $record = Read-BootstrapRecord $Bootstrap
        if (-not $record) { Stop-Unavailable "bootstrap.recovery is malformed" $null }
        $helper = Join-Path $Lifecycle "staging\$($record.Operation)\draft.exe"
        $valid = (Test-Path -LiteralPath $helper -PathType Leaf) -and
            ((Get-FileHash -Algorithm SHA256 -LiteralPath $helper).Hash.ToLowerInvariant() -eq $record.Sha256) -and
            ((Get-Item -LiteralPath $helper).Length -eq $record.Size)
        if (-not $valid) {
            if (Test-Path -LiteralPath $InstalledDraft -PathType Leaf) {
                & $InstalledDraft __installer recover
                if ($LASTEXITCODE -ne 0) { Stop-Unavailable "the installed Draft could not resume it" $record }
            } else {
                Stop-Unavailable "the staged helper is missing or does not match its recorded identity" $record
            }
        } else {
            # Identity/integrity consistency inside the installation's private
            # lifecycle directory — not a signature. The helper validates
            # everything authoritatively against operation.json.
            Write-Host "Resuming an interrupted uninstall at $InstallRoot"
            & $helper __lifecycle-helper --installation-id $record.Installation --operation-id $record.Operation --bootstrap-recovery
            if ($LASTEXITCODE -ne 0) { Fail "the uninstall helper refused or failed; the lifecycle state was left intact" }
        }
    }
    "B" {
        Write-Host "Resuming an interrupted Draft lifecycle operation at $InstallRoot"
        & $InstalledDraft __installer recover
        if ($LASTEXITCODE -ne 0) { Fail "the installed Draft could not resume its interrupted operation; nothing else was changed" }
    }
    "E" {
        Fail "$Lifecycle holds uninstall residue with no journal and no terminal record; nothing was changed. This state needs lifecycle repair/support."
    }
}
$PendingRecovery = (Get-Route) -eq "C"

$TempDir = Join-Path ([System.IO.Path]::GetTempPath()) ("draft-install-" + [System.Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $TempDir | Out-Null

try {
    if ($Version) {
        $Tag = "v$($Version.TrimStart('v'))"
    } else {
        $Latest = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" -Headers @{ "User-Agent" = "draft-installer" }
        $Tag = $Latest.tag_name
    }

    if (-not $Tag) {
        Fail "could not resolve latest Draft release from https://github.com/$Repo"
    }

    $ResolvedVersion = $Tag.TrimStart("v")
    $Asset = "draft-v$ResolvedVersion-$Target.zip"
    $BaseUrl = "https://github.com/$Repo/releases/download/$Tag"
    $ArchivePath = Join-Path $TempDir $Asset
    $ChecksumsPath = Join-Path $TempDir "SHA256SUMS"

    Write-Host "Downloading Draft $Tag for $Target"
    Invoke-WebRequest -Uri "$BaseUrl/$Asset" -OutFile $ArchivePath -Headers @{ "User-Agent" = "draft-installer" }
    Invoke-WebRequest -Uri "$BaseUrl/SHA256SUMS" -OutFile $ChecksumsPath -Headers @{ "User-Agent" = "draft-installer" }

    $ChecksumLine = Get-Content $ChecksumsPath | Where-Object { $_ -match "\s+$([regex]::Escape($Asset))$" } | Select-Object -First 1
    if (-not $ChecksumLine) {
        Fail "checksum entry for $Asset was not found in SHA256SUMS"
    }

    $Expected = ($ChecksumLine -split "\s+")[0].ToLowerInvariant()
    $Actual = (Get-FileHash -Algorithm SHA256 -Path $ArchivePath).Hash.ToLowerInvariant()
    if ($Expected -ne $Actual) {
        Fail "checksum verification failed for $Asset"
    }

    $PackageRoot = Join-Path $TempDir "package"
    Expand-Archive -Path $ArchivePath -DestinationPath $PackageRoot -Force
    $PackageDir = Join-Path $PackageRoot "draft-v$ResolvedVersion-$Target"
    $Coordinator = Join-Path $PackageDir "bin\draft.exe"
    if (-not (Test-Path $Coordinator)) { Fail "archive did not contain bin\draft.exe" }
    if (-not (Test-Path (Join-Path $PackageDir "bin\draftd.exe"))) { Fail "archive did not contain bin\draftd.exe" }

    if ($PendingRecovery) {
        # Journal present, no bootstrap, no installed Draft: this downloaded
        # coordinator may continue only a compatible FreshInstall of exactly
        # this release. Anything else fails closed in Rust.
        Write-Host "Resuming an interrupted installation at $InstallRoot"
        & $Coordinator __installer recover --install-root $InstallRoot
        if ($LASTEXITCODE -ne 0) {
            if ($env:DRAFT_INSTALL_ROOT) { Write-Host "Keep DRAFT_INSTALL_ROOT=$InstallRoot when re-running." }
            Fail "the interrupted installation could not be resumed by this release; nothing was changed"
        }
    }

    $Arguments = @("__installer", "install", "--install-root", $InstallRoot)
    if ($UpdatePath) { $Arguments += "--update-path" }
    & $Coordinator @Arguments
    if ($LASTEXITCODE -ne 0) { Fail "installation failed; see the message above" }

    if ($UpdatePath) {
        Write-Host "Restart PowerShell before running draft from a new shell."
    } else {
        Write-Host "To put Draft on your User PATH, re-run with -UpdatePath or DRAFT_UPDATE_PATH=1."
    }

    & (Join-Path $InstallRoot "bin\draft.exe") --version
}
finally {
    if (Test-Path $TempDir) {
        Remove-Item -Recurse -Force $TempDir
    }
}
