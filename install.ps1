param(
    [string]$InstallDir = $env:DRAFT_INSTALL_DIR,
    [string]$Version = $env:DRAFT_VERSION,
    [string]$Repo = $(if ($env:DRAFT_REPO) { $env:DRAFT_REPO } else { "Shiva936/draft" }),
    [switch]$UpdatePath
)

$ErrorActionPreference = "Stop"

function Fail($Message) {
    Write-Error "draft install: $Message"
    exit 1
}

if ([System.Environment]::OSVersion.Platform -ne [System.PlatformID]::Win32NT) {
    Fail "install.ps1 supports native Windows PowerShell only. Use install.sh on Linux, macOS, or WSL."
}

if (-not $InstallDir) {
    $InstallDir = Join-Path $env:LOCALAPPDATA "Programs\Draft\bin"
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

    Write-Host "Installing Draft $Tag for $Target"
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

    Expand-Archive -Path $ArchivePath -DestinationPath $TempDir -Force
    $PackageDir = Join-Path $TempDir "draft-v$ResolvedVersion-$Target"
    $DraftExe = Join-Path $PackageDir "bin\draft.exe"
    $DraftdExe = Join-Path $PackageDir "bin\draftd.exe"

    if (-not (Test-Path $DraftExe)) {
        Fail "archive did not contain bin\draft.exe"
    }
    if (-not (Test-Path $DraftdExe)) {
        Fail "archive did not contain bin\draftd.exe"
    }

    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    Copy-Item -Path $DraftExe -Destination (Join-Path $InstallDir "draft.exe") -Force
    Copy-Item -Path $DraftdExe -Destination (Join-Path $InstallDir "draftd.exe") -Force

    Write-Host "Installed draft to $(Join-Path $InstallDir "draft.exe")"
    Write-Host "Installed draftd to $(Join-Path $InstallDir "draftd.exe")"

    $EffectivePath = $env:Path
    $UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $PathParts = @()
    if ($EffectivePath) {
        $PathParts = $EffectivePath -split ";" | Where-Object { $_ }
    }
    $OnPath = $false
    foreach ($Part in $PathParts) {
        if ($Part.TrimEnd("\") -ieq $InstallDir.TrimEnd("\")) {
            $OnPath = $true
        }
    }

    if (-not $OnPath) {
        Write-Host ""
        Write-Host "$InstallDir is not on your User PATH."
        if ($UpdatePath) {
            $NewPath = if ($UserPath) { "$UserPath;$InstallDir" } else { $InstallDir }
            [Environment]::SetEnvironmentVariable("Path", $NewPath, "User")
            Write-Host "Updated User PATH. Restart PowerShell before running draft from a new shell."
        } else {
            Write-Host "Add it with:"
            Write-Host "  [Environment]::SetEnvironmentVariable('Path', [Environment]::GetEnvironmentVariable('Path', 'User') + ';$InstallDir', 'User')"
            Write-Host "To let this installer update User PATH, run with -UpdatePath or set DRAFT_UPDATE_PATH=1."
        }
    }

    & (Join-Path $InstallDir "draft.exe") --version
}
finally {
    if (Test-Path $TempDir) {
        Remove-Item -Recurse -Force $TempDir
    }
}
