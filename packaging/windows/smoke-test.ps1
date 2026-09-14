# Installs the MSI on this machine (a CI runner, with no sound card), checks the task, the
# firewall rule and the jukebox itself, and uninstalls it.
#
#   packaging/windows/smoke-test.ps1 -Msi dist/Kyylan-Jukebox-0.3.0-x64.msi
param([Parameter(Mandatory)] [string] $Msi)
$ErrorActionPreference = 'Stop'

$msi = Resolve-Path $Msi
$program = Join-Path $env:ProgramFiles 'Kyylan Jukebox\kyylan-jukebox.exe'
$task = 'Kyylan Jukebox'

function Step($text) { Write-Output "`n== $text" }

function Invoke-Msiexec($arguments, $log) {
    $process = Start-Process msiexec.exe -ArgumentList ($arguments + @('/qn', '/l*v', $log)) -Wait -PassThru
    if ($process.ExitCode -ne 0) {
        Get-Content $log -Tail 80
        throw "msiexec $arguments exited with $($process.ExitCode)"
    }
}

function Wait-Jukebox {
    for ($i = 0; $i -lt 60; $i++) {
        try {
            Invoke-RestMethod http://127.0.0.1:8080/api/config | Out-Null
            return
        } catch {
            Start-Sleep -Seconds 1
        }
    }
    Get-ChildItem "$env:APPDATA\kyylan-jukebox\logs" -ErrorAction SilentlyContinue | Get-Content -Tail 50
    throw 'the jukebox never answered'
}

function Test-FirewallRule {
    netsh advfirewall firewall show rule name="Kyylan Jukebox" | Out-Null
    return $LASTEXITCODE -eq 0
}

Step 'Install'
Invoke-Msiexec @('/i', "$msi") 'install.log'
if (-not (Test-Path $program)) { throw "$program is missing" }
schtasks /Query /TN $task /V /FO LIST
if ($LASTEXITCODE -ne 0) { throw 'the scheduled task is missing' }
$definition = [xml](schtasks /Query /TN $task /XML | Out-String)
if ($definition.Task.Settings.ExecutionTimeLimit -ne 'PT0S') { throw 'the task has a run time limit' }
if ($definition.Task.Settings.DisallowStartIfOnBatteries -ne 'false') { throw 'the task needs AC power' }
if (-not (Test-FirewallRule)) { throw 'the firewall rule is missing' }
# A GUI-subsystem program: piped, so PowerShell waits for it and its output.
$version = & $program --version | Out-String
if ($LASTEXITCODE -ne 0) { throw 'the program does not run' }
Write-Output $version

Step 'Started by the installer, through the task'
Wait-Jukebox
$page = Invoke-WebRequest -UseBasicParsing http://127.0.0.1:8080/
if ($page.Content -notmatch '<div id="root"') { throw 'the web UI is not served' }

Step 'Installing again repairs it'
Invoke-Msiexec @('/i', "$msi") 'reinstall.log'
Wait-Jukebox

Step 'Uninstall keeps the data'
Invoke-Msiexec @('/x', "$msi") 'uninstall.log'
Start-Sleep -Seconds 2
if (Test-Path $program) { throw "$program is still there" }
schtasks /Query /TN $task 2>$null
if ($LASTEXITCODE -eq 0) { throw 'the scheduled task is still there' }
if (Test-FirewallRule) { throw 'the firewall rule is still there' }
if (Get-Process kyylan-jukebox -ErrorAction SilentlyContinue) { throw 'the jukebox is still running' }
if (-not (Test-Path "$env:APPDATA\kyylan-jukebox\config.json")) { throw 'the data went with it' }

Step 'Passed'
