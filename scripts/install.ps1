# Fleety CLI installer (Windows) — downloads the latest release of `fleety` and
# installs it onto your user PATH.
#
#   irm https://raw.githubusercontent.com/TimLai666/fleety-agent/main/scripts/install.ps1 | iex
#
# Override the install dir with $env:FLEETY_INSTALL_DIR.
$ErrorActionPreference = 'Stop'

$repo = 'TimLai666/fleety-agent'
$target = 'x86_64-pc-windows-msvc'
$asset = "fleety-$target.zip"
$url = "https://github.com/$repo/releases/latest/download/$asset"

$dir = if ($env:FLEETY_INSTALL_DIR) { $env:FLEETY_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\fleety' }
New-Item -ItemType Directory -Force -Path $dir | Out-Null

$zip = Join-Path $env:TEMP $asset
Write-Host "fleety: downloading $asset ..."
try {
    Invoke-WebRequest -Uri $url -OutFile $zip
} catch {
    Write-Error "fleety: download failed from $url (has a release been published yet? see github.com/$repo/releases)"
    return
}

Expand-Archive -Path $zip -DestinationPath $dir -Force
Remove-Item $zip -Force

# Add to user PATH if missing.
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if (-not ($userPath -split ';' | Where-Object { $_ -eq $dir })) {
    [Environment]::SetEnvironmentVariable('Path', "$userPath;$dir", 'User')
    Write-Host "fleety: added $dir to your user PATH (restart your shell to pick it up)"
}

Write-Host "fleety: installed to $dir\fleety.exe"
