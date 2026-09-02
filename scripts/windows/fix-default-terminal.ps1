#Requires -Version 5.1
<#
.SYNOPSIS
  Stop Windows Terminal from being the blast radius for every console
  couchlink spawns.

.DESCRIPTION
  docs/INCIDENT-2026-08-19-terminals-died.md root-caused one session-ending
  crash to WindowsTerminal.exe's XAML engine, and the same crash recurred
  live during testing the same night. The second occurrence lined up with
  heavy powershell.exe invocation from the WSL side (build, launch, query,
  relaunch - couchlink's own tooling), not an unrelated OS bug: on a default
  Windows 11 install, HKCU\Console\%%Startup (the "Default Terminal
  Application" setting) is unset, which means every new console process -
  including ones spawned non-interactively from WSL/scripts, with no user
  ever choosing to open a terminal at all - gets attached as a tab inside
  the same Windows Terminal process. Enough rapid tab churn destabilizes
  its UI framework, and because Windows Terminal hosts every tab in one
  process, that one crash takes down everything else running in it too -
  which is exactly what turned "a console app hiccuped" into "all 8 windows
  died and the game session with them."

  This is the actual root-cause fix: couchlink's own consoles should never
  land inside the user's interactive Windows Terminal in the first place.
  Idempotent - checks current values before writing, safe to run every
  install/build.

.NOTES
  GUIDs are Microsoft's own well-known Default Terminal Application ids:
  {B23D10C0-E52E-411E-9D5B-C09FDF709C7D} = Windows Console Host (conhost)
  {E12CFF52-A866-4C77-9A90-F570A7AA2C6B} = Windows Terminal
#>
$ErrorActionPreference = "Continue"

$ConsoleHostId = "{B23D10C0-E52E-411E-9D5B-C09FDF709C7D}"
$KeyPath = "HKCU:\Console\%%Startup"

function Ok([string]$m) { Write-Host "OK: $m" }
function Info([string]$m) { Write-Host "INFO: $m" }

if (-not (Test-Path $KeyPath)) {
    New-Item -Path $KeyPath -Force | Out-Null
}

$current = Get-ItemProperty -Path $KeyPath -ErrorAction SilentlyContinue
$alreadyConsoleHost = $current -and
    $current.DelegationConsole -eq $ConsoleHostId -and
    $current.DelegationTerminal -eq $ConsoleHostId

if ($alreadyConsoleHost) {
    Ok "default terminal already set to Windows Console Host - nothing to do"
    exit 0
}

Set-ItemProperty -Path $KeyPath -Name "DelegationConsole" -Value $ConsoleHostId
Set-ItemProperty -Path $KeyPath -Name "DelegationTerminal" -Value $ConsoleHostId
Ok "default terminal set to Windows Console Host"
Info "new console processes (including couchlink's own build/launch/query calls) no longer attach into Windows Terminal - takes effect immediately, no reboot needed"
exit 0
