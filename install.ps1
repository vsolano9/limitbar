<#
.SYNOPSIS
  Builds Limitbar, installs it for the current user, starts it and (by default)
  registers it to start with Windows. Run again to upgrade in place.
.PARAMETER Exe
  Install this already-built limitbar.exe (for example one downloaded from a
  release) instead of building from source.
.PARAMETER NoStartup
  Do not add the "start with Windows" entry.
.PARAMETER Uninstall
  Stop Limitbar, remove the startup entry, the installed exe and its cached state.
#>
param([string]$Exe, [switch]$NoStartup, [switch]$Uninstall)
$ErrorActionPreference = 'Stop'

$dest = Join-Path $env:LOCALAPPDATA 'Programs\Limitbar'
$installed = Join-Path $dest 'limitbar.exe'
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'

function Stop-Limitbar {
    if (Test-Path $installed) { & $installed --exit }
    $deadline = (Get-Date).AddSeconds(5)
    while ((Get-Process limitbar -ErrorAction SilentlyContinue) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 200 }
}

if ($Uninstall) {
    Stop-Limitbar
    Remove-ItemProperty -Path $runKey -Name Limitbar -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force $dest -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force (Join-Path $env:LOCALAPPDATA 'Limitbar') -ErrorAction SilentlyContinue
    Write-Host 'Limitbar uninstalled (config in %APPDATA%\Limitbar, if any, was kept).'
    return
}

if ($Exe) {
    $built = (Resolve-Path $Exe).Path
} else {
    Push-Location $PSScriptRoot
    try {
        cargo build --release
        if ($LASTEXITCODE) { throw 'cargo build failed' }
        $targetDir = (cargo metadata --format-version 1 --no-deps | ConvertFrom-Json).target_directory
        $built = Join-Path $targetDir 'release\limitbar.exe'
    } finally { Pop-Location }
}

Stop-Limitbar
New-Item -ItemType Directory -Force $dest | Out-Null
Copy-Item $built $installed -Force

if ($NoStartup) {
    Remove-ItemProperty -Path $runKey -Name Limitbar -ErrorAction SilentlyContinue
} else {
    Set-ItemProperty -Path $runKey -Name Limitbar -Value "`"$installed`""
}
# Started through Explorer so the widget outlives this shell: terminals and agent
# tools can run scripts in a job object that kills every child when it closes.
Start-Process explorer.exe -ArgumentList "`"$installed`""
$deadline = (Get-Date).AddSeconds(10)
while (-not (Get-Process limitbar -ErrorAction SilentlyContinue) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 200 }
if (-not (Get-Process limitbar -ErrorAction SilentlyContinue)) { Start-Process $installed }
Write-Host "Limitbar installed to $installed and started."
