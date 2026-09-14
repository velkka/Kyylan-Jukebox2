# Builds the Windows installer from a built program.
#
#   packaging/windows/build-msi.ps1 -Program target/release/kyylan-jukebox.exe -Version 0.3.0 -Output dist/Kyylan-Jukebox-0.3.0-x64.msi
#
# Needs WiX 5 and its firewall and util extensions:
#   dotnet tool install --global wix --version 5.0.2
#   wix extension add -g WixToolset.Firewall.wixext/5.0.2 WixToolset.Util.wixext/5.0.2
param(
    [Parameter(Mandatory)] [string] $Program,
    [Parameter(Mandatory)] [string] $Version,
    [Parameter(Mandatory)] [string] $Output
)
$ErrorActionPreference = 'Stop'

$repo = Resolve-Path (Join-Path $PSScriptRoot '../..')
$icon = Join-Path $repo 'build/icon.ico'
if (-not (Test-Path $icon)) { throw 'build/icon.ico is missing: run node scripts/gen-icons.cjs first' }

$work = Join-Path ([System.IO.Path]::GetTempPath()) "kyylan-jukebox-msi-$PID"
New-Item -ItemType Directory -Force -Path $work | Out-Null
# schtasks /XML reads UTF-16.
$task = Join-Path $work 'task.xml'
[System.IO.File]::WriteAllText($task, (Get-Content (Join-Path $PSScriptRoot 'task.xml') -Raw -Encoding UTF8), [System.Text.Encoding]::Unicode)

New-Item -ItemType Directory -Force -Path (Split-Path -Parent $Output) | Out-Null
wix build (Join-Path $PSScriptRoot 'kyylan-jukebox.wxs') `
    -arch x64 `
    -ext WixToolset.Firewall.wixext `
    -ext WixToolset.Util.wixext `
    -d "Version=$Version" `
    -d "ProgramFile=$(Resolve-Path $Program)" `
    -d "TaskFile=$task" `
    -d "IconFile=$icon" `
    -o $Output
if ($LASTEXITCODE -ne 0) { throw "wix build failed with $LASTEXITCODE" }
Remove-Item -Recurse -Force $work
Write-Output "wrote $Output"
