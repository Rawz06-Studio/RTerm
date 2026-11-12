# Builds RTerm for Windows and packs it under dist/ as:
#   RTerm-<version>-windows-<arch>-setup.exe  installer (per user, no admin
#                                             prompt; Start menu entry)
#   RTerm-<version>-windows-<arch>.zip        portable: RTerm.exe, README,
#                                             license; nothing to install
#
#   scripts/bundle-windows.ps1 [version]
#
# With a version (as CI passes it), the program reports that version.
# Without one, the version from Cargo.toml is used and reported as
# "<version>-dev". The installer needs Inno Setup 6 (installed with
# Chocolatey when missing).
param([string]$Version)
$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')

$cargoVersion = (Select-String -Path Cargo.toml -Pattern '^version = "(.*)"' |
    Select-Object -First 1).Matches[0].Groups[1].Value
if ($Version) { $env:RTERM_VERSION = $Version } else { $Version = $cargoVersion }
$arch = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'aarch64' } else { 'x86_64' }
$name = "RTerm-$Version-windows-$arch"

Write-Host "==> Building RTerm $Version ($arch)"
cargo build --release --locked
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
$exe = (Resolve-Path target/release/rterm.exe).Path

Remove-Item -Recurse -Force dist -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Path dist | Out-Null
$dist = (Resolve-Path dist).Path

Write-Host "==> Packing $name.zip (portable)"
$folder = Join-Path $dist $name
New-Item -ItemType Directory -Path $folder | Out-Null
Copy-Item $exe (Join-Path $folder 'RTerm.exe')
Copy-Item README.md, LICENSE $folder
Compress-Archive -Path $folder -DestinationPath (Join-Path $dist "$name.zip")
Remove-Item -Recurse $folder

Write-Host "==> Building $name-setup.exe (installer)"
$iscc = "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe"
if (-not (Test-Path $iscc)) {
    choco install innosetup -y --no-progress
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}
& $iscc /Qp "/DAppVersion=$Version" "/DSourceExe=$exe" "/DOutputDir=$dist" "/DOutputName=$name-setup" packaging\windows\rterm.iss
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Push-Location $dist
$packages = @(Get-ChildItem "$name*")
$packages | ForEach-Object {
    "$((Get-FileHash $_.Name -Algorithm SHA256).Hash.ToLower())  $($_.Name)"
} | Out-File -Encoding ascii "$name.sha256"
Pop-Location
Get-ChildItem dist
