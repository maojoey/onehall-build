$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public class OneHallControlConsole {
 [DllImport("kernel32.dll")] public static extern IntPtr GetConsoleWindow();
}
'@
if ([OneHallControlConsole]::GetConsoleWindow() -ne [IntPtr]::Zero) { exit 92 }
# Report readiness only after checking the actual live control process.
Write-Output $PID
Start-Sleep 120
