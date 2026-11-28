param(
    [Parameter(Mandatory=$true)][string]$LogDir,
    [Parameter(Mandatory=$true)][string]$Prefix
)

New-Item -ItemType Directory -Force -Path $LogDir | Out-Null

Add-Type @"
using System;
using System.Runtime.InteropServices;

public class DesktopLauncher {
  [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
  public struct STARTUPINFO {
    public int cb; public string lpReserved; public string lpDesktop; public string lpTitle;
    public int dwX; public int dwY; public int dwXSize; public int dwYSize;
    public int dwXCountChars; public int dwYCountChars; public int dwFillAttribute;
    public int dwFlags; public short wShowWindow; public short cbReserved2;
    public IntPtr lpReserved2; public IntPtr hStdInput; public IntPtr hStdOutput; public IntPtr hStdError;
  }
  [StructLayout(LayoutKind.Sequential)]
  public struct PROCESS_INFORMATION {
    public IntPtr hProcess; public IntPtr hThread; public int dwProcessId; public int dwThreadId;
  }
  [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
  public static extern bool CreateProcessW(
    string lpApplicationName, string lpCommandLine, IntPtr lpProcessAttributes, IntPtr lpThreadAttributes,
    bool bInheritHandles, uint dwCreationFlags, IntPtr lpEnvironment, string lpCurrentDirectory,
    ref STARTUPINFO lpStartupInfo, out PROCESS_INFORMATION lpProcessInformation);

  public static int LaunchOnDefaultDesktop(string cmdLine, string workDir) {
    STARTUPINFO si = new STARTUPINFO();
    si.cb = Marshal.SizeOf(si);
    si.lpDesktop = "WinSta0\\Default";
    si.dwFlags = 1; // STARTF_USESHOWWINDOW
    si.wShowWindow = 1; // SW_SHOWNORMAL
    PROCESS_INFORMATION pi;
    if (!CreateProcessW(null, cmdLine, IntPtr.Zero, IntPtr.Zero, false, 0, IntPtr.Zero, workDir, ref si, out pi))
      throw new Exception("CreateProcessW failed: " + Marshal.GetLastWin32Error());
    return pi.dwProcessId;
  }
}
"@

$drrun = "$env:LOCALAPPDATA\Programs\DynamoRIO\bin64\drrun.exe"
$exe = "E:\SteamLibrary\steamapps\common\Ravenswatch\Ravenswatch.exe"
$workDir = "E:\SteamLibrary\steamapps\common\Ravenswatch"
$stdout = "$Prefix-stdout.txt"
$stderr = "$Prefix-stderr.txt"
$cmdLine = "cmd.exe /c `"`"$drrun`" -t drcov -logdir `"$LogDir`" -- `"$exe`" > `"$stdout`" 2> `"$stderr`"`""

$launchedPid = [DesktopLauncher]::LaunchOnDefaultDesktop($cmdLine, $workDir)
Set-Content -Path "$Prefix-pid.txt" -Value $launchedPid -Encoding ASCII
Write-Output "LAUNCHED_PID=$launchedPid"

$p = [System.Diagnostics.Process]::GetProcessById($launchedPid)
$p.WaitForExit()
Write-Output "EXITED code=$($p.ExitCode)"
