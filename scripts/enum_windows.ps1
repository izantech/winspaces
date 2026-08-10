$code = @"
using System;
using System.Text;
using System.Runtime.InteropServices;

public class Win32Helper {
    [DllImport("user32.dll")]
    public static extern bool EnumWindows(EnumWindowsProc enumProc, IntPtr lParam);

    [DllImport("user32.dll")]
    public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint lpdwProcessId);

    [DllImport("user32.dll")]
    public static extern bool IsWindowVisible(IntPtr hWnd);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern int GetWindowTextW(IntPtr hWnd, StringBuilder lpString, int nMaxCount);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern int GetClassNameW(IntPtr hWnd, StringBuilder lpString, int nMaxCount);

    public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);

    public static void FindWindows(uint pid) {
        EnumWindows((hWnd, lParam) => {
            uint windowPid;
            GetWindowThreadProcessId(hWnd, out windowPid);
            if (windowPid == pid) {
                StringBuilder sbTitle = new StringBuilder(256);
                GetWindowTextW(hWnd, sbTitle, 256);
                StringBuilder sbClass = new StringBuilder(256);
                GetClassNameW(hWnd, sbClass, 256);
                bool vis = IsWindowVisible(hWnd);
                Console.WriteLine("HWND: 0x{0:X8} | Visible: {1} | Class: '{2}' | Title: '{3}'", hWnd.ToInt64(), vis, sbClass.ToString(), sbTitle.ToString());
            }
            return true;
        }, IntPtr.Zero);
    }
}
"@

Add-Type -TypeDefinition $code
$procs = Get-Process WinSpaces.Gui -ErrorAction SilentlyContinue
if ($procs) {
    foreach ($p in $procs) {
        Write-Host "PID: $($p.Id)"
        [Win32Helper]::FindWindows($p.Id)
    }
} else {
    Write-Host "No process found."
}
