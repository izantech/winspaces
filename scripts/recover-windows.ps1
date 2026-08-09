# WinSpaces window recovery.
# A broken build hid/cloaked application windows (ShowWindow(SW_HIDE) and/or
# DwmSetWindowAttribute(DWMWA_CLOAK)) and never restored them on exit, so they stay
# invisible while still running. This script enumerates every top-level window,
# removes the DWM cloak, re-shows any window WinSpaces was tracking (marked with the
# "WinSpacesWindowState" property), and clears that property. Safe to re-run.

Add-Type -TypeDefinition @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;

public static class WinSpacesRecover {
    private delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);

    [DllImport("user32.dll")] private static extern bool EnumWindows(EnumWindowsProc cb, IntPtr p);
    [DllImport("user32.dll")] private static extern bool IsWindow(IntPtr h);
    [DllImport("user32.dll")] private static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] private static extern bool ShowWindow(IntPtr h, int cmd);
    [DllImport("user32.dll", CharSet = CharSet.Ansi)] private static extern IntPtr GetPropA(IntPtr h, string s);
    [DllImport("user32.dll", CharSet = CharSet.Ansi)] private static extern IntPtr RemovePropA(IntPtr h, string s);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern int GetWindowTextW(IntPtr h, System.Text.StringBuilder s, int n);
    [DllImport("dwmapi.dll")] private static extern int DwmSetWindowAttribute(IntPtr h, int a, ref int v, uint cb);

    private const int DWMWA_CLOAK = 14;
    private const int SW_SHOW = 5;

    private static List<IntPtr> _all = new List<IntPtr>();
    private static bool Cb(IntPtr h, IntPtr lp) { _all.Add(h); return true; }

    public class Result {
        public int Uncloaked;
        public int Reshown;
        public List<string> Titles = new List<string>();
    }

    public static Result Recover() {
        EnumWindows(Cb, IntPtr.Zero);
        var r = new Result();
        foreach (var h in _all) {
            if (!IsWindow(h)) continue;
            // Uncloak everyone (no-op on windows that were never cloaked).
            int zero = 0;
            int hr = DwmSetWindowAttribute(h, DWMWA_CLOAK, ref zero, 4u);
            if (hr >= 0) r.Uncloaked++;

            IntPtr prop = GetPropA(h, "WinSpacesWindowState");
            if (prop != IntPtr.Zero) {
                ShowWindow(h, SW_SHOW);
                RemovePropA(h, "WinSpacesWindowState");
                r.Reshown++;
                var sb = new System.Text.StringBuilder(256);
                GetWindowTextW(h, sb, 256);
                r.Titles.Add("0x" + h.ToString("X") + "  " + sb.ToString().Trim());
            }
        }
        return r;
    }
}
"@

$res = [WinSpacesRecover]::Recover()
Write-Host "Uncloak attempts: $($res.Uncloaked)"
Write-Host "WinSpaces-tracked windows restored: $($res.Reshown)"
if ($res.Titles.Count -gt 0) {
    Write-Host ""
    Write-Host "Restored windows:"
    foreach ($t in $res.Titles) { Write-Host "  $t" }
}
Write-Host ""
Write-Host "Done. Your apps should be visible again now."
