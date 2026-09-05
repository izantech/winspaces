using System;
using System.Diagnostics;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;

class Program {
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
    public struct MONITORINFO {
        public int cbSize;
        public RECT rcMonitor;
        public RECT rcWork;
        public int dwFlags;
    }

    public delegate bool EnumThreadDelegate(IntPtr hWnd, IntPtr lParam);

    [DllImport("user32.dll")] public static extern bool EnumThreadWindows(int dwThreadId, EnumThreadDelegate lpfn, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr hWnd, StringBuilder lpString, int nMaxCount);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassNameW(IntPtr hWnd, StringBuilder lpString, int nMaxCount);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);
    [DllImport("user32.dll")] public static extern bool GetWindowPlacement(IntPtr hWnd, ref WINDOWPLACEMENT lpwndpl);
    [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr hwnd, int dwAttribute, out RECT pvAttribute, int cbAttribute);
    [DllImport("user32.dll")] public static extern IntPtr MonitorFromWindow(IntPtr hwnd, uint dwFlags);
    [DllImport("user32.dll")] public static extern bool GetMonitorInfoW(IntPtr hMonitor, ref MONITORINFO lpmi);

    static EnumThreadDelegate callback;

    public static void Main(string[] args) {
        string filter = args.Length > 0 ? args[0] : "";
        string outFile = args.Length > 1 ? args[1] : "";

        StringBuilder sb = new StringBuilder();
        sb.AppendLine("=== WINSPACES WINDOW METRICS DUMP (" + DateTime.Now.ToString("o") + ") ===");

        callback = new EnumThreadDelegate((hwnd, lparam) => {
            StringBuilder sbTitle = new StringBuilder(512);
            GetWindowTextW(hwnd, sbTitle, 512);
            string title = sbTitle.ToString();

            StringBuilder sbClass = new StringBuilder(256);
            GetClassNameW(hwnd, sbClass, 256);
            string className = sbClass.ToString();

            RECT rect;
            GetWindowRect(hwnd, out rect);
            int w = rect.Right - rect.Left;
            int h = rect.Bottom - rect.Top;

            bool isMatch = string.IsNullOrEmpty(filter) ||
                           title.IndexOf(filter, StringComparison.OrdinalIgnoreCase) >= 0 ||
                           className.IndexOf(filter, StringComparison.OrdinalIgnoreCase) >= 0;

            if (isMatch && (title.Length > 0 || className.Length > 0)) {
                RECT frame;
                DwmGetWindowAttribute(hwnd, 9, out frame, 16);

                WINDOWPLACEMENT wp = new WINDOWPLACEMENT();
                wp.length = Marshal.SizeOf(wp);
                GetWindowPlacement(hwnd, ref wp);

                IntPtr hmon = MonitorFromWindow(hwnd, 2);
                MONITORINFO mi = new MONITORINFO();
                mi.cbSize = Marshal.SizeOf(mi);
                GetMonitorInfoW(hmon, ref mi);

                bool vis = IsWindowVisible(hwnd);
                int fw = frame.Right - frame.Left;
                int fh = frame.Bottom - frame.Top;
                int mw = mi.rcWork.Right - mi.rcWork.Left;
                int mh = mi.rcWork.Bottom - mi.rcWork.Top;

                sb.AppendLine("==========================================");
                sb.AppendLine(string.Format("HWND: {0} (0x{1:X}), Vis={2}, Thread={3}", hwnd, hwnd.ToInt64(), vis, lparam));
                sb.AppendLine("Title: '" + title + "'");
                sb.AppendLine("Class: '" + className + "'");
                sb.AppendLine(string.Format("GetWindowRect: Left={0}, Top={1}, Right={2}, Bottom={3} [W={4}, H={5}]", rect.Left, rect.Top, rect.Right, rect.Bottom, w, h));
                sb.AppendLine(string.Format("DwmFrameBounds: Left={0}, Top={1}, Right={2}, Bottom={3} [W={4}, H={5}]", frame.Left, frame.Top, frame.Right, frame.Bottom, fw, fh));
                sb.AppendLine(string.Format("WindowPlacement: showCmd={0}, flags={1}", wp.showCmd, wp.flags));
                sb.AppendLine(string.Format("  rcNormalPos: Left={0}, Top={1}, Right={2}, Bottom={3}", wp.rcNormalPosition.Left, wp.rcNormalPosition.Top, wp.rcNormalPosition.Right, wp.rcNormalPosition.Bottom));
                sb.AppendLine(string.Format("MonitorWorkArea: Left={0}, Top={1}, Right={2}, Bottom={3} [W={4}, H={5}]", mi.rcWork.Left, mi.rcWork.Top, mi.rcWork.Right, mi.rcWork.Bottom, mw, mh));
                sb.AppendLine(string.Format("MonitorTotalArea: Left={0}, Top={1}, Right={2}, Bottom={3}", mi.rcMonitor.Left, mi.rcMonitor.Top, mi.rcMonitor.Right, mi.rcMonitor.Bottom));
            }
            return true;
        });

        foreach (Process p in Process.GetProcesses()) {
            try {
                foreach (ProcessThread t in p.Threads) {
                    EnumThreadWindows(t.Id, callback, new IntPtr(t.Id));
                }
            } catch {}
        }

        string output = sb.ToString();
        if (!string.IsNullOrEmpty(outFile)) {
            File.WriteAllText(outFile, output);
            Console.WriteLine("Saved dump to " + outFile);
        } else {
            Console.Write(output);
        }
    }
}
