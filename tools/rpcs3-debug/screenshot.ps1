# Save a PNG of the whole virtual desktop (all monitors). Usage: -Out C:\path\shot.png
param([string]$Out = "$env:TEMP\rpcs3-shot.png")
Add-Type -AssemblyName System.Windows.Forms,System.Drawing
$b = [System.Windows.Forms.SystemInformation]::VirtualScreen
$bmp = New-Object System.Drawing.Bitmap $b.Width,$b.Height
[System.Drawing.Graphics]::FromImage($bmp).CopyFromScreen($b.Left,$b.Top,0,0,$bmp.Size)
$bmp.Save($Out,[System.Drawing.Imaging.ImageFormat]::Png); $Out
