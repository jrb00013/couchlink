# Bind two local DualSense pads to distinct RPCS3 player slots via SDL.
#
# The native DualSense HID handler often fails on Windows Bluetooth
# (feature/output report errors), which makes both players latch onto one pad.
# SDL gives each controller a unique "… 1" / "… 2" name.
#
# powershell -ExecutionPolicy Bypass -File configure_local_pads.ps1
# Close RPCS3 first — it overwrites this file on exit.

param(
  [string]$Rpcs3Dir = $(if ($env:RPCS3_DIR) { $env:RPCS3_DIR } else { Join-Path $env:USERPROFILE 'RPCS3' }),
  [string]$ConfigName = 'Default',
  [string]$DeviceBaseName = 'DualSense Wireless Controller'
)

$ErrorActionPreference = 'Stop'

if (Get-Process rpcs3 -ErrorAction SilentlyContinue) {
  Write-Output "RPCS3 is running. Close it first — it overwrites this config on exit."
  exit 1
}

$cfg = Join-Path $Rpcs3Dir "config\input_configs\global\$ConfigName.yml"
if (-not (Test-Path $cfg)) {
  Write-Output "Input config not found: $cfg"
  exit 1
}

$btPads = @(Get-PnpDevice -PresentOnly -ErrorAction SilentlyContinue |
  Where-Object { $_.InstanceId -match '^BTHENUM\\DEV_' -and
    $_.FriendlyName -match 'DualSense|Wireless Controller' })
$usbPads = @(Get-PnpDevice -PresentOnly -Class HIDClass -ErrorAction SilentlyContinue |
  Where-Object { $_.InstanceId -match '^HID\\VID_054C&PID_0CE6' -and
    $_.InstanceId -match 'MI_03' })
$padCount = $btPads.Count + $usbPads.Count
Write-Output "Detected $padCount DualSense controller(s): $($btPads.Count) bluetooth, $($usbPads.Count) usb."
if ($padCount -lt 2) {
  Write-Output "Need 2 controllers connected. Pair both, then re-run."
  exit 2
}

$backup = "$cfg.pre-sdl.bak"
Copy-Item -Path $cfg -Destination $backup -Force
Write-Output "Backed up to $backup"

$playerBlock = @"
Player {0} Input:
  Handler: SDL
  Device: "{1} {0}"
  Config:
    Left Stick Left: "LS X-"
    Left Stick Down: "LS Y-"
    Left Stick Right: "LS X+"
    Left Stick Up: "LS Y+"
    Right Stick Left: "RS X-"
    Right Stick Down: "RS Y-"
    Right Stick Right: "RS X+"
    Right Stick Up: "RS Y+"
    Start: Start
    Select: Back
    PS Button: "Start&Back,Guide"
    Square: West
    Cross: South
    Circle: East
    Triangle: North
    Left: Left
    Down: Down
    Right: Right
    Up: Up
    R1: RB
    R2: RT
    R3: RS
    L1: LB
    L2: LT
    L3: LS
    IR Nose: ""
    IR Tail: ""
    IR Left: ""
    IR Right: ""
    Tilt Left: ""
    Tilt Right: ""
    Motion Sensor X:
      Axis: ""
      Mirrored: false
      Shift: 0
    Motion Sensor Y:
      Axis: ""
      Mirrored: false
      Shift: 0
    Motion Sensor Z:
      Axis: ""
      Mirrored: false
      Shift: 0
    Motion Sensor G:
      Axis: ""
      Mirrored: false
      Shift: 0
    Orientation Reset Button: ""
    Orientation Enabled: false
    Pressure Intensity Button: ""
    Pressure Intensity Percent: 50
    Pressure Intensity Toggle Mode: false
    Pressure Intensity Deadzone: 0
    Analog Limiter Button: ""
    Analog Limiter Toggle Mode: false
    Left Stick Multiplier: 100
    Right Stick Multiplier: 100
    Left Stick Deadzone: 40
    Right Stick Deadzone: 40
    Left Stick Anti-Deadzone: 33
    Right Stick Anti-Deadzone: 33
    Left Trigger Threshold: 0
    Right Trigger Threshold: 0
    Left Pad Squircling Factor: 8000
    Right Pad Squircling Factor: 8000
    Color Value R: 0
    Color Value G: 0
    Color Value B: 20
    Blink LED when battery is below 20%: true
    Use LED as a battery indicator: false
    LED battery indicator brightness: 10
    Player LED enabled: true
    Large Vibration Motor Multiplier: 100
    Small Vibration Motor Multiplier: 100
    Switch Vibration Motors: false
    Vibration Threshold: 20
    Mouse Movement Mode: Relative
    Mouse Deadzone X Axis: 60
    Mouse Deadzone Y Axis: 60
    Mouse Acceleration X Axis: 200
    Mouse Acceleration Y Axis: 250
    Left Stick Lerp Factor: 100
    Right Stick Lerp Factor: 100
    Analog Button Lerp Factor: 100
    Trigger Lerp Factor: 100
    Device Class Type: 0
    Vendor ID: 0
    Product ID: 0
  Buddy Device: ""
"@

$nullBlock = @"
Player {0} Input:
  Handler: Null
  Device: Null
  Buddy Device: ""
"@

$out = New-Object System.Collections.Generic.List[string]
for ($p = 1; $p -le 7; $p++) {
  if ($p -le 2) {
    foreach ($ln in ($playerBlock -f $p, $DeviceBaseName) -split "`n") {
      $out.Add($ln.TrimEnd("`r"))
    }
    Write-Output "Player $p -> SDL '$DeviceBaseName $p'"
  }
  else {
    foreach ($ln in ($nullBlock -f $p) -split "`n") {
      $out.Add($ln.TrimEnd("`r"))
    }
  }
}

[System.IO.File]::WriteAllLines($cfg, $out)
Write-Output ""
Write-Output "Done. Start RPCS3 → Pads and confirm each slot lights up from a different controller."
Write-Output "If a device name differs, set Handler=SDL and pick the two numbered entries from the dropdown."
