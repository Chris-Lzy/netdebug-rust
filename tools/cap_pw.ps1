param([string]$Page = 'master')
$ErrorActionPreference = 'Stop'
Add-Type -MemberDefinition @'
[DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
[DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
[DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
[DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint msg, IntPtr wp, IntPtr lp);
public struct RECT { public int L; public int T; public int R; public int B; }
'@ -Name W -Namespace N
Add-Type -AssemblyName System.Drawing
[N.W]::SetProcessDPIAware() | Out-Null
$exe = 'F:\netdebug-rust-v0.1.0-source\target\release\netdebug.exe'
$p = Start-Process -FilePath $exe -PassThru
if ($Page -eq 'nic') { Start-Sleep -Seconds 9 } else { Start-Sleep -Seconds 3 }
$p.Refresh()
$h = $p.MainWindowHandle
$r = New-Object N.W+RECT
[N.W]::GetWindowRect($h, [ref]$r) | Out-Null
$ww = $r.R - $r.L; $wh = $r.B - $r.T
function Shot($path) {
    $bmp = New-Object System.Drawing.Bitmap $ww, $wh
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $hdc = $g.GetHdc()
    [N.W]::PrintWindow($h, $hdc, 2) | Out-Null
    $g.ReleaseHdc()
    $g.Dispose()
    $bmp.Save($path)
    $bmp.Dispose()
}
if ($Page -eq 'master') {
    $x = 267; $y = $wh - 198
    $lp = [IntPtr](($y -shl 16) -bor $x)
    [N.W]::PostMessage($h, 0x0200, [IntPtr]::new(0), $lp) | Out-Null
    Start-Sleep -Milliseconds 120
    [N.W]::PostMessage($h, 0x0201, [IntPtr]::new(1), $lp) | Out-Null
    Start-Sleep -Milliseconds 80
    [N.W]::PostMessage($h, 0x0202, [IntPtr]::new(0), $lp) | Out-Null
    Start-Sleep -Milliseconds 1800
}
Shot "F:\netdebug-rust-v0.1.0-source\target\vid-$Page.png"
Stop-Process -Id $p.Id -Force
Write-Host "done $Page size ${ww}x${wh}"