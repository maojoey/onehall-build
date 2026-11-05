$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public class OneHallProbe {
 [DllImport("kernel32.dll")] public static extern bool IsProcessInJob(IntPtr p, IntPtr j, out bool result);
 [DllImport("kernel32.dll")] public static extern IntPtr GetCurrentProcess();
 [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)] public struct Startup { public int cb; public string reserved,desktop,title; public int x,y,w,h,xc,yc,fill,flags; public short show,reserved2; public IntPtr reservedPtr,input,output,error; }
 [StructLayout(LayoutKind.Sequential)] public struct ProcessInfo { public IntPtr process,thread; public uint pid,tid; }
 [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)] static extern bool CreateProcess(string app, System.Text.StringBuilder command, IntPtr pa, IntPtr ta, bool inherit, uint flags, IntPtr env, string cwd, ref Startup startup, out ProcessInfo info);
 [DllImport("kernel32.dll")] static extern bool TerminateProcess(IntPtr p, uint code);
 [DllImport("kernel32.dll")] static extern uint WaitForSingleObject(IntPtr p, uint timeout);
 [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr h);
 public static bool BreakawayDenied() {
   Startup s = new Startup(); s.cb=Marshal.SizeOf(s); ProcessInfo p;
   // BREAKAWAY_FROM_JOB | CREATE_SUSPENDED | CREATE_NO_WINDOW, even on unexpected success.
   bool created=CreateProcess(Environment.GetEnvironmentVariable("ComSpec"),new System.Text.StringBuilder("cmd.exe /c exit 0"),IntPtr.Zero,IntPtr.Zero,false,0x09000004,IntPtr.Zero,null,ref s,out p);
   if(!created) return Marshal.GetLastWin32Error()==5;
   TerminateProcess(p.process,1); WaitForSingleObject(p.process,5000); CloseHandle(p.thread); CloseHandle(p.process); return false;
 }
}
'@
$inside=$false
if(-not [OneHallProbe]::IsProcessInJob([OneHallProbe]::GetCurrentProcess(),[IntPtr]::Zero,[ref]$inside) -or -not $inside -or -not [OneHallProbe]::BreakawayDenied()){ exit 91 }
# Every generation checks its own console before acknowledging readiness.
# CreateNoWindow must accompany UseShellExecute=false at EVERY spawn boundary.
$quietPrelude = @'
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition 'using System; using System.Runtime.InteropServices; public class OneHallQuietConsole { [DllImport("kernel32.dll")] public static extern IntPtr GetConsoleWindow(); }'
if ([OneHallQuietConsole]::GetConsoleWindow() -ne [IntPtr]::Zero) { exit 92 }
function Start-QuietChild([string]$script) {
 $start=New-Object System.Diagnostics.ProcessStartInfo
 $start.FileName='powershell.exe'
 $start.Arguments='-NoProfile -NonInteractive -EncodedCommand '+[Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($script))
 $start.UseShellExecute=$false
 $start.CreateNoWindow=$true
 $start.RedirectStandardOutput=$true
 return [System.Diagnostics.Process]::Start($start)
}
'@
. ([scriptblock]::Create($quietPrelude))
$leaf=$quietPrelude+"`nWrite-Output `$PID; Start-Sleep 120"
$encodedLeaf=[Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($leaf))
$middle=$quietPrelude+"`n"+@'
$leaf=[Text.Encoding]::Unicode.GetString([Convert]::FromBase64String('__LEAF__'))
$p=Start-QuietChild $leaf
$grandchild=$p.StandardOutput.ReadLine()
if ($p.HasExited -or $grandchild -ne [string]$p.Id) { exit 93 }
Write-Output "$PID $grandchild"
Start-Sleep 120
'@
$middle=$middle.Replace('__LEAF__',$encodedLeaf)
$p=Start-QuietChild $middle
$descendants=$p.StandardOutput.ReadLine()
if ($p.HasExited -or $descendants -notmatch '^([0-9]+) ([0-9]+)$' -or $Matches[1] -ne [string]$p.Id) { exit 93 }
Write-Output "$PID $descendants"
[Console]::In.ReadLine() | Out-Null
$q=Start-QuietChild $leaf
$late=$q.StandardOutput.ReadLine()
if ($q.HasExited -or $late -ne [string]$q.Id) { exit 93 }
Write-Output $late
Start-Sleep 120
