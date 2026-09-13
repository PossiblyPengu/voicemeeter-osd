# Logs every strip/bus gain + mute for a while, reporting which ones move.
# Used to identify exactly which Voicemeeter parameter a hardware control drives.
param([int]$Seconds = 40, [string]$LogPath = "$PSScriptRoot\..\param_watch.log")

$sig = @'
using System;
using System.Runtime.InteropServices;
public class VMW {
  const string DLL = @"C:\Program Files (x86)\VB\Voicemeeter\VoicemeeterRemote64.dll";
  [DllImport(DLL, CallingConvention=CallingConvention.StdCall)] public static extern int VBVMR_Login();
  [DllImport(DLL, CallingConvention=CallingConvention.StdCall)] public static extern int VBVMR_Logout();
  [DllImport(DLL, CallingConvention=CallingConvention.StdCall)] public static extern int VBVMR_IsParametersDirty();
  [DllImport(DLL, CallingConvention=CallingConvention.StdCall)] public static extern int VBVMR_GetParameterFloat([MarshalAs(UnmanagedType.LPStr)] string p, out float v);
}
'@
Add-Type -TypeDefinition $sig

[VMW]::VBVMR_Login() | Out-Null
Start-Sleep -Milliseconds 200

$names = @()
foreach ($i in 0..7) { $names += "Strip[$i].Gain" }
foreach ($i in 0..7) { $names += "Bus[$i].Gain" }

$baseline = @{}
$seen = @{}
[VMW]::VBVMR_IsParametersDirty() | Out-Null
foreach ($n in $names) {
    $v = 0.0
    if ([VMW]::VBVMR_GetParameterFloat($n, [ref]$v) -eq 0) { $baseline[$n] = $v }
}

"=== watching for $Seconds s, started $(Get-Date -Format HH:mm:ss) ===" | Out-File $LogPath -Encoding utf8
"baseline: " + (($baseline.GetEnumerator() | Sort-Object Name | ForEach-Object { "$($_.Name)=$($_.Value)" }) -join "  ") | Out-File $LogPath -Append -Encoding utf8

$end = (Get-Date).AddSeconds($Seconds)
while ((Get-Date) -lt $end) {
    [VMW]::VBVMR_IsParametersDirty() | Out-Null
    foreach ($n in $baseline.Keys) {
        $v = 0.0
        if ([VMW]::VBVMR_GetParameterFloat($n, [ref]$v) -eq 0) {
            if ([Math]::Abs($v - $baseline[$n]) -gt 0.01) {
                "$(Get-Date -Format HH:mm:ss.fff)  $n : $($baseline[$n]) -> $v" | Out-File $LogPath -Append -Encoding utf8
                $baseline[$n] = $v
                $seen[$n] = $true
            }
        }
    }
    Start-Sleep -Milliseconds 40
}

"=== done ===" | Out-File $LogPath -Append -Encoding utf8
if ($seen.Count -eq 0) {
    "NO PARAMETER CHANGED" | Out-File $LogPath -Append -Encoding utf8
} else {
    "parameters that moved: " + (($seen.Keys | Sort-Object) -join ", ") | Out-File $LogPath -Append -Encoding utf8
}
[VMW]::VBVMR_Logout() | Out-Null
