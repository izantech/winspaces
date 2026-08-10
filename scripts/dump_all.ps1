$code = @"
using System;
using System.Text;
using System.IO;
using System.Runtime.InteropServices;

public class WinDumper {
    public struct RECT { public int Left, Top, Right, Bottom; }
    public struct POINT { public int X, Y; }
    public struct WINDOWPLACEMENT {
        public int length;
        public int flags;
        public int showCmd;
        public POINT ptMinPosition;
        public POINT ptMaxPosition;
        public RECT rcNormalPosition;
    }

    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc enumProc, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr hWnd, StringBuilder lpString, int nMaxCount);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassNameW(IntPtr hWnd, StringBuilder lpString, int nMaxCount);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);
    [DllImport("user32.dll")] public static extern bool GetWindowPlacement(IntPtr hWnd, ref WINDOWPLACEMENT lpwndpl);
    [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr hwnd, int dwAttribute, out RECT pvAttribute, int cbAttribute);

    public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);

    public static void RunDump(string outFile) {
        StringBuilder sb = new StringBuilder();
        sb.AppendLine("=== WINSPACES ENUMWINDOWS METRICS DUMP (" + DateTime.Now.ToString("o") + ") ===");

        EnumWindows((hWnd, lParam) => {
            if (!IsWindowVisible(hWnd)) return true;

            StringBuilder sbTitle = new StringBuilder(512);
            GetWindowTextW(hWnd, sbTitle, 512);
            string title = sbTitle.ToString();

            StringBuilder sbClass = new StringBuilder(512);
            GetClassNameW(hWnd, sbClass, 512);
            string className = sbClass.ToString();

            if (string.IsNullOrEmpty(title) && string.IsNullOrEmpty(className)) return true;
            if (className == "Progman" || className == "Shell_TrayWnd") return true;

            RECT winRect;
            GetWindowRect(hWnd, out winRect);
            int w = winRect.Right - winRect.Left;
            int h = winRect.Bottom - winRect.Top;

            if (w < 100 || h < 100) return true;

            RECT frameRect = new RECT();
            DwmGetWindowAttribute(hWnd, 9, out frameRect, 16);

            WINDOWPLACEMENT wp = new WINDOWPLACEMENT();
            wp.length = Marshal.SizeOf(typeof(WINDOWPLACEMENT));
            GetWindowPlacement(hWnd, ref wp);

            sb.AppendLine("==========================================");
            sb.AppendLine(string.Format("HWND: 0x{0:X}, Title: '{1}', Class: '{2}'", hWnd.ToInt64(), title, className));
            sb.AppendLine(string.Format("GetWindowRect: Left={0}, Top={1}, Right={2}, Bottom={3} [W={4}, H={5}]", winRect.Left, winRect.Top, winRect.Right, winRect.Bottom, w, h));
            sb.AppendLine(string.Format("DwmFrameBounds: Left={0}, Top={1}, Right={2}, Bottom={3}", frameRect.Left, frameRect.Top, frameRect.Right, frameRect.Bottom));
            sb.AppendLine(string.Format("Placement: showCmd={0}, NormalPos: Left={1}, Top={2}, Right={3}, Bottom={4}", wp.showCmd, wp.rcNormalPosition.Left, wp.rcNormalPosition.Top, wp.rcNormalPosition.Right, wp.rcNormalPosition.Bottom));

            return true;
        }, IntPtr.Zero);

        File.WriteAllText(outFile, sb.ToString());
    }
}
"@

Add-Type -TypeDefinition $code
[WinDumper]::RunDump(".local/logs/win-dumps/dump_NEW.txt")
