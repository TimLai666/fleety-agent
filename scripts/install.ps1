# Fleety client installer (Windows) — downloads the latest release of `fleety`
# and `fleetyd`, installs both onto your user PATH, and starts the local daemon
# service.
#
#   irm https://raw.githubusercontent.com/TimLai666/fleety-agent/main/scripts/install.ps1 | iex
#
# Override the install dir with $env:FLEETY_INSTALL_DIR.
$ErrorActionPreference = 'Stop'

$repo = 'TimLai666/fleety-agent'
$target = 'x86_64-pc-windows-msvc'

$dir = if ($env:FLEETY_INSTALL_DIR) { $env:FLEETY_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\fleety' }
New-Item -ItemType Directory -Force -Path $dir | Out-Null

function Download-Asset([string] $bin, [string] $stage) {
    $asset = "$bin-$target.zip"
    $url = "https://github.com/$repo/releases/latest/download/$asset"
    $zip = Join-Path $env:TEMP $asset
    Write-Host "$bin`: downloading $asset ..."
    try {
        Invoke-WebRequest -Uri $url -OutFile $zip
        Expand-Archive -Path $zip -DestinationPath $stage -Force
    } catch {
        throw "$bin`: download or extraction failed from $url (has a release been published yet? see github.com/$repo/releases)"
    } finally {
        if (Test-Path $zip) { Remove-Item $zip -Force }
    }
}

$stage = Join-Path $env:TEMP ("fleety-install-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $stage | Out-Null
try {
    Download-Asset 'fleety' $stage
    Download-Asset 'fleetyd' $stage

    foreach ($bin in @('fleety', 'fleetyd')) {
        $staged = Join-Path $stage "$bin.exe"
        if (-not (Test-Path $staged -PathType Leaf)) {
            throw "$bin`: archive did not contain $bin.exe"
        }
    }
    Copy-Item (Join-Path $stage 'fleety.exe') (Join-Path $dir 'fleety.exe') -Force
    Copy-Item (Join-Path $stage 'fleetyd.exe') (Join-Path $dir 'fleetyd.exe') -Force
} finally {
    Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue
}

# Add to user PATH if missing.
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if (-not ($userPath -split ';' | Where-Object { $_ -eq $dir })) {
    [Environment]::SetEnvironmentVariable('Path', "$userPath;$dir", 'User')
    Write-Host "fleety: added $dir to your user PATH (restart your shell to pick it up)"
}

Write-Host "fleety: installed to $dir\fleety.exe"
Write-Host "fleetyd: installed to $dir\fleetyd.exe"

$fleetyd = Join-Path $dir 'fleetyd.exe'
& $fleetyd install
if ($LASTEXITCODE -ne 0) {
    throw "fleetyd: service registration failed; rerun '$fleetyd install' from an Administrator terminal"
}
& $fleetyd start
if ($LASTEXITCODE -ne 0) {
    throw "fleetyd: service start failed; rerun '$fleetyd start' after fixing the service error"
}
Write-Host "fleetyd: service registered and started (login autostart remains disabled)"
