#Requires -Version 5.1
<#
.SYNOPSIS
  Hot-apply couchlink pad bindings into a running PCSX2 without restarting it.

.DESCRIPTION
  Piece B of couchlink pad bring-up: reconfigure a RUNNING pcsx2-qt's
  in-memory Pad bindings. Piece A (ds-vhid preallocate) only keeps ViGEm
  devices alive — it does not touch PCSX2 SettingsInterface.

  Upstream has no PINE/IPC for this. The Controllers "Apply Profile" button
  is the public entry that runs:

    Pad::CopyConfiguration(base, profile) -> CommitBaseSettingChanges
      -> applySettings -> VMManager::ApplySettings

  This script Invokes that same Qt button (UIA InvokePattern on toolbar
  Controllers + Apply Profile) — not SendKeys. Expects
  inis/inputprofiles/<ProfileName>.ini already written by link-emulator-pad.sh.

  Called automatically when pcsx2-qt is running unless
  COUCHLINK_PCSX2_LIVE_APPLY=0.

.EXAMPLE
  .\pcsx2-live-apply-pads.ps1
  .\pcsx2-live-apply-pads.ps1 -ProfileName couchlink -TimeoutSec 20
#>
param(
    [string]$ProfileName = "couchlink",
    [int]$TimeoutSec = 25
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type -AssemblyName System.Windows.Forms

function Write-Step([string]$Msg) {
    Write-Host "==> pcsx2-live-apply: $Msg"
}

function Get-UiaRoot {
    [System.Windows.Automation.AutomationElement]::RootElement
}

function Find-ByName(
    [System.Windows.Automation.AutomationElement]$Root,
    [System.Windows.Automation.ControlType]$Type,
    [string]$Name
) {
    $cond = New-Object System.Windows.Automation.AndCondition(
        (New-Object System.Windows.Automation.PropertyCondition(
            [System.Windows.Automation.AutomationElement]::ControlTypeProperty, $Type)),
        (New-Object System.Windows.Automation.PropertyCondition(
            [System.Windows.Automation.AutomationElement]::NameProperty, $Name))
    )
    return $Root.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $cond)
}

function Find-ByNameAnyType(
    [System.Windows.Automation.AutomationElement]$Root,
    [string]$Name
) {
    $cond = New-Object System.Windows.Automation.PropertyCondition(
        [System.Windows.Automation.AutomationElement]::NameProperty, $Name)
    return $Root.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $cond)
}

function Find-Window-ByTitleSubstring([string]$Needle) {
    $typeWin = [System.Windows.Automation.ControlType]::Window
    $cond = New-Object System.Windows.Automation.PropertyCondition(
        [System.Windows.Automation.AutomationElement]::ControlTypeProperty, $typeWin)
    $wins = (Get-UiaRoot).FindAll([System.Windows.Automation.TreeScope]::Children, $cond)
    foreach ($w in $wins) {
        $t = $w.Current.Name
        if ($t -and ($t -like "*$Needle*")) { return $w }
    }
    return $null
}

function Try-Invoke([System.Windows.Automation.AutomationElement]$El) {
    if (-not $El) { return $false }
    try {
        $inv = $El.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern)
        $inv.Invoke()
        return $true
    } catch {
        return $false
    }
}

function Try-Toggle-Or-Invoke([System.Windows.Automation.AutomationElement]$El) {
    if (-not $El) { return $false }
    try {
        $tp = $El.GetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern)
        # Exit exclusive fullscreen if currently on.
        if ($tp.Current.ToggleState -eq [System.Windows.Automation.ToggleState]::On) {
            $tp.Toggle()
            return $true
        }
        return $false
    } catch {}
    return (Try-Invoke $El)
}

function Set-Combo-ByName(
    [System.Windows.Automation.AutomationElement]$Combo,
    [string]$ItemName
) {
    $expand = $null
    try {
        $expand = $Combo.GetCurrentPattern([System.Windows.Automation.ExpandCollapsePattern]::Pattern)
        $expand.Expand()
        Start-Sleep -Milliseconds 250
    } catch {}

    $itemCond = New-Object System.Windows.Automation.PropertyCondition(
        [System.Windows.Automation.AutomationElement]::NameProperty, $ItemName)
    $item = $Combo.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $itemCond)
    if (-not $item) {
        $liType = New-Object System.Windows.Automation.PropertyCondition(
            [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
            [System.Windows.Automation.ControlType]::ListItem)
        $items = $Combo.FindAll([System.Windows.Automation.TreeScope]::Descendants, $liType)
        foreach ($i in $items) {
            if ($i.Current.Name -eq $ItemName) { $item = $i; break }
        }
    }
    if (-not $item) {
        throw "Profile '$ItemName' not found in Editing Profile combo (is couchlink.ini in inputprofiles?)"
    }
    $itemSel = $item.GetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern)
    $itemSel.Select()
    Start-Sleep -Milliseconds 200
    if ($expand) {
        try { $expand.Collapse() } catch {}
    }
}

function Close-Element([System.Windows.Automation.AutomationElement]$Win) {
    try {
        $wp = $Win.GetCurrentPattern([System.Windows.Automation.WindowPattern]::Pattern)
        $wp.Close()
        return
    } catch {}
    $closeBtn = Find-ByName $Win ([System.Windows.Automation.ControlType]::Button) "Close"
    if ($closeBtn) { [void](Try-Invoke $closeBtn) }
}

function Open-Controller-Settings(
    [System.Windows.Automation.AutomationElement]$Main
) {
    # Prefer toolbar Controllers (actionToolbarControllerSettings) — InvokePattern,
    # same as clicking the button. No SendKeys.
    $candidates = @(
        @{ Type = [System.Windows.Automation.ControlType]::Button; Name = "Controllers" },
        @{ Type = [System.Windows.Automation.ControlType]::MenuItem; Name = "Controllers" },
        @{ Type = [System.Windows.Automation.ControlType]::MenuItem; Name = "Controllers..." }
    )
    foreach ($c in $candidates) {
        $el = Find-ByName $Main $c.Type $c.Name
        if ($el -and (Try-Invoke $el)) {
            Write-Step "invoked $($c.Type.ProgrammaticName) '$($c.Name)'"
            return $true
        }
    }
    # Fallback: Settings menu -> Controllers (still Invoke, not keystrokes).
    $settingsMenu = Find-ByName $Main ([System.Windows.Automation.ControlType]::MenuItem) "Settings"
    if (-not $settingsMenu) {
        $settingsMenu = Find-ByNameAnyType $Main "Settings"
    }
    if ($settingsMenu) {
        try {
            $exp = $settingsMenu.GetCurrentPattern([System.Windows.Automation.ExpandCollapsePattern]::Pattern)
            $exp.Expand()
            Start-Sleep -Milliseconds 200
        } catch {
            [void](Try-Invoke $settingsMenu)
            Start-Sleep -Milliseconds 200
        }
        foreach ($name in @("Controllers", "Controllers...", "&Controllers")) {
            $item = Find-ByNameAnyType $Main $name
            if (-not $item) { $item = Find-ByNameAnyType (Get-UiaRoot) $name }
            if ($item -and (Try-Invoke $item)) {
                Write-Step "invoked Settings -> $name"
                return $true
            }
        }
    }
    return $false
}

# --- main --------------------------------------------------------------------

$pcsx2 = Get-Process -Name "pcsx2-qt" -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $pcsx2) {
    Write-Step "pcsx2-qt not running - nothing to live-apply"
    exit 0
}

Write-Step "pcsx2-qt pid=$($pcsx2.Id) - applying profile '$ProfileName' via UIA Invoke"

$existing = Find-Window-ByTitleSubstring "Controller Settings"
if ($existing) {
    Write-Step "closing existing Controller Settings so profile list reloads"
    Close-Element $existing
    Start-Sleep -Milliseconds 400
}

$main = $null
try {
    $main = [System.Windows.Automation.AutomationElement]::FromHandle($pcsx2.MainWindowHandle)
} catch {}
if (-not $main) {
    throw "Could not attach UIA to pcsx2-qt main window"
}

# Exclusive fullscreen can hide Controllers; leave it if a Fullscreen control is on.
$fs = Find-ByNameAnyType $main "Fullscreen"
if ($fs) {
    if (Try-Toggle-Or-Invoke $fs) {
        Write-Step "left fullscreen so Controller Settings can show"
        Start-Sleep -Milliseconds 400
    }
}

try { $main.SetFocus() } catch {}
Start-Sleep -Milliseconds 150

if (-not (Open-Controller-Settings $main)) {
    throw "Could not Invoke Controllers (toolbar/menu) - open Settings -> Controllers manually once to confirm UI names"
}

$deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSec)
$settings = $null
while ([DateTime]::UtcNow -lt $deadline) {
    $settings = Find-Window-ByTitleSubstring "Controller Settings"
    if ($settings) { break }
    Start-Sleep -Milliseconds 250
}
if (-not $settings) {
    throw "Timed out waiting for PCSX2 Controller Settings window"
}
Write-Step "Controller Settings open"

$comboType = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
    [System.Windows.Automation.ControlType]::ComboBox)
$combos = $settings.FindAll([System.Windows.Automation.TreeScope]::Descendants, $comboType)
$combo = $null
if ($combos.Count -ge 1) { $combo = $combos.Item(0) }
if (-not $combo) { throw "Could not find Editing Profile combo box" }

Write-Step "selecting profile '$ProfileName'"
Set-Combo-ByName $combo $ProfileName
Start-Sleep -Milliseconds 300

$apply = Find-ByName $settings ([System.Windows.Automation.ControlType]::Button) "Apply Profile"
if (-not $apply) { throw "Apply Profile button not found" }
if (-not $apply.Current.IsEnabled) {
    throw "Apply Profile is disabled - profile may still be Shared"
}
Write-Step "Invoking Apply Profile"
if (-not (Try-Invoke $apply)) {
    throw "InvokePattern failed on Apply Profile"
}

$confirmDeadline = [DateTime]::UtcNow.AddSeconds(8)
$yes = $null
$yesName = "Yes"
$yesNameAlt = [string]::Concat([char]38, "Yes")  # &Yes
while ([DateTime]::UtcNow -lt $confirmDeadline) {
    $dialogs = (Get-UiaRoot).FindAll(
        [System.Windows.Automation.TreeScope]::Children,
        (New-Object System.Windows.Automation.PropertyCondition(
            [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
            [System.Windows.Automation.ControlType]::Window)))
    foreach ($d in $dialogs) {
        $y = Find-ByName $d ([System.Windows.Automation.ControlType]::Button) $yesName
        if ($y) { $yes = $y; break }
        $y = Find-ByName $d ([System.Windows.Automation.ControlType]::Button) $yesNameAlt
        if ($y) { $yes = $y; break }
    }
    if ($yes) { break }
    Start-Sleep -Milliseconds 150
}
if ($yes) {
    Write-Step "confirming Load Input Profile"
    [void](Try-Invoke $yes)
    Start-Sleep -Milliseconds 400
} else {
    Write-Step "WARNING: no Yes confirmation seen - Apply may have been cancelled"
}

$settings = Find-Window-ByTitleSubstring "Controller Settings"
if ($settings) {
    Write-Step "closing Controller Settings"
    Close-Element $settings
}

Write-Step "done - in-memory pads should match profile '$ProfileName'"
exit 0
