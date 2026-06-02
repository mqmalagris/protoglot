# protoglot CLI installer (Windows).
#
#   irm https://raw.githubusercontent.com/mqmalagris/protoglot/main/install.ps1 | iex
#
# Downloads the latest release, installs `protoglot` + `pglot` to
# %LOCALAPPDATA%\Programs\protoglot, and adds it to your user PATH.

$ErrorActionPreference = "Stop"
$repo = "mqmalagris/protoglot"

if (-not [Environment]::Is64BitOperatingSystem) {
    throw "protoglot ships 64-bit Windows builds only."
}
$target = "x86_64-pc-windows-msvc"

Write-Host "Looking up the latest protoglot release..."
$release = Invoke-RestMethod -Uri "https://api.github.com/repos/$repo/releases/latest" `
    -Headers @{ "User-Agent" = "protoglot-install" }
$asset = $release.assets | Where-Object { $_.name -like "*-$target.zip" } | Select-Object -First 1
if (-not $asset) { throw "no Windows asset found in release $($release.tag_name)." }

$tmp = Join-Path $env:TEMP ("protoglot-" + [guid]::NewGuid())
New-Item -ItemType Directory -Force -Path $tmp | Out-Null
$zip = Join-Path $tmp $asset.name
Write-Host "Downloading $($asset.name)..."
Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $zip
Expand-Archive -Path $zip -DestinationPath $tmp -Force

$src = (Get-ChildItem $tmp -Recurse -Filter protoglot.exe | Select-Object -First 1).Directory
$dest = Join-Path $env:LOCALAPPDATA "Programs\protoglot"
New-Item -ItemType Directory -Force -Path $dest | Out-Null
Copy-Item (Join-Path $src "protoglot.exe") $dest -Force
Copy-Item (Join-Path $src "pglot.exe") $dest -Force
Remove-Item $tmp -Recurse -Force

$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($userPath -notlike "*$dest*") {
    [Environment]::SetEnvironmentVariable("Path", "$userPath;$dest", "User")
    Write-Host "Added $dest to your PATH — restart your shell to use it."
}
Write-Host "Installed protoglot $($release.tag_name) (and the 'pglot' alias) to $dest"
