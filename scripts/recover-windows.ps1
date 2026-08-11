# WinSpaces window recovery.
# A broken build hid/cloaked application windows (ShowWindow(SW_HIDE),
# DwmSetWindowAttribute(DWMWA_CLOAK), and/or the ImmersiveShell
# IApplicationView::SetCloak) and never restored them on exit, so they stay
# invisible while still running. This script enumerates every top-level window,
# removes the DWM cloak, shell-uncloaks and re-shows any window WinSpaces was
# tracking (marked with the "WinSpacesWindowState" property), and clears that
# property. Safe to re-run.

Add-Type -TypeDefinition @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;

// Undocumented ImmersiveShell interfaces: a shell cloak is NOT cleared by
// DwmSetWindowAttribute(DWMWA_CLOAK, 0) — it must be undone with the same
// SetCloak call that applied it. Filler methods only occupy vtable slots.
[ComImport, Guid("6D5140C1-7436-11CE-8034-00AA006009FA"),
 InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
public interface IServiceProvider10 {
    [return: MarshalAs(UnmanagedType.IUnknown)]
    object QueryService(ref Guid service, ref Guid riid);
}

[ComImport, Guid("1841C6D7-4F9D-42C0-AF41-8747538F10E5"),
 InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
public interface IApplicationViewCollection {
    void M1(); void M2(); void M3();
    [PreserveSig] int GetViewForHwnd(IntPtr hwnd, out IApplicationView view);
}

[ComImport, Guid("372E1D3B-38D3-42E4-A15B-8AB2B178F513"),
 InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
public interface IApplicationView {
    void M1(); void M2(); void M3(); void M4(); void M5();
    void M6(); void M7(); void M8(); void M9();
    [PreserveSig] int SetCloak(uint cloakType, int flags);
}

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
        public int ShellUncloaked;
        public int Reshown;
        public List<string> Titles = new List<string>();
    }

    private static IApplicationViewCollection GetViewCollection() {
        try {
            var shellType = Type.GetTypeFromCLSID(new Guid("C2F03A33-21F5-47FA-B4BB-156362A2F239"));
            var provider = (IServiceProvider10)Activator.CreateInstance(shellType);
            var iid = typeof(IApplicationViewCollection).GUID;
            var service = iid;
            return (IApplicationViewCollection)provider.QueryService(ref service, ref iid);
        } catch { return null; }
    }

    public static Result Recover() {
        EnumWindows(Cb, IntPtr.Zero);
        var views = GetViewCollection();
        var r = new Result();
        foreach (var h in _all) {
            if (!IsWindow(h)) continue;
            // Uncloak everyone (no-op on windows that were never cloaked).
            int zero = 0;
            int hr = DwmSetWindowAttribute(h, DWMWA_CLOAK, ref zero, 4u);
            if (hr >= 0) r.Uncloaked++;

            IntPtr prop = GetPropA(h, "WinSpacesWindowState");
            if (prop != IntPtr.Zero) {
                if (views != null) {
                    try {
                        IApplicationView view;
                        if (views.GetViewForHwnd(h, out view) >= 0 && view != null) {
                            if (view.SetCloak(1, 0) >= 0) r.ShellUncloaked++;
                        }
                    } catch { }
                }
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
Write-Host "Shell uncloaks: $($res.ShellUncloaked)"
Write-Host "WinSpaces-tracked windows restored: $($res.Reshown)"
if ($res.Titles.Count -gt 0) {
    Write-Host ""
    Write-Host "Restored windows:"
    foreach ($t in $res.Titles) { Write-Host "  $t" }
}
Write-Host ""
Write-Host "Done. Your apps should be visible again now."
