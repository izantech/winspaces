using System;
using System.IO;
using System.Text;
using System.Runtime.InteropServices;

class DumpWindows {
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

    public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);

    [DllImport("user32.dll")] public static extern bool EnumDesktopWindows(IntPtr hDesktop, EnumWindowsProc lpfn, IntPtr lParam);
    [DllImport("user32.dll")] public static extern IntPtr OpenInputDesktop(uint dwFlags, bool fInherit, uint dwDesiredAccess);
    [DllImport("user32.dll")] public static extern bool SetThreadDesktop(IntPtr hDesktop);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr hWnd, StringBuilder lpString, int nMaxCount);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassNameW(IntPtr hWnd, StringBuilder lpString, int nMaxCount);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr hWnd, IntPtr hWndInsertAfter, int X, int Y, int cx, int cy, uint uFlags);
    [DllImport("user32.dll")] public static extern bool SetWindowPlacement(IntPtr hWnd, ref WINDOWPLACEMENT lpwndpl);
    [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr hwnd, int dwAttribute, out RECT pvAttribute, int cbAttribute);
    [DllImport("dwmapi.dll")] public static extern int DwmSetWindowAttribute(IntPtr hwnd, int dwAttribute, ref int pvAttribute, int cbAttribute);

    static EnumWindowsProc callback;

    static void Main(string[] args) {
        string outFile = ".local/logs/win-dumps/border_fix_test.txt";
        IntPtr hDesk = OpenInputDesktop(0, false, 0x0100);
        if (hDesk != IntPtr.Zero) SetThreadDesktop(hDesk);

        StringBuilder sb = new StringBuilder();
        sb.AppendLine("=== BORDER FIX TEST (" + DateTime.Now.ToString("o") + ") ===");

        callback = new EnumWindowsProc((hWnd, lParam) => {
            StringBuilder sbTitle = new StringBuilder(512);
            GetWindowTextW(hWnd, sbTitle, 512);
            string title = sbTitle.ToString();

            StringBuilder sbClass = new StringBuilder(512);
            GetClassNameW(hWnd, sbClass, 512);
            string className = sbClass.ToString();

            if (title.Contains("WhatsApp Web") || title.Contains("Novedades bajadas")) {
                int donotround = 1;
                DwmSetWindowAttribute(hWnd, 33, ref donotround, 4);

                RECT winRect, frameBefore;
                GetWindowRect(hWnd, out winRect);
                DwmGetWindowAttribute(hWnd, 9, out frameBefore, 16);

                int borderLeft = (frameBefore.Left - winRect.Left);
                if (borderLeft < 0) borderLeft = 7;
                int borderTop = (frameBefore.Top - winRect.Top);
                if (borderTop < 0) borderTop = 0;
                int borderRight = (winRect.Right - frameBefore.Right);
                if (borderRight < 0) borderRight = 7;
                int borderBottom = (winRect.Bottom - frameBefore.Bottom);
                if (borderBottom < 0) borderBottom = 7;

                // Target Monitor 2 Work Area: Left=-1536, Top=534, Right=0, Bottom=1494
                bool isWhatsApp = title.Contains("WhatsApp");
                int targetL = isWhatsApp ? -1536 : -768;
                int targetT = 534;
                int targetR = isWhatsApp ? -768 : 0;
                int targetB = 1494;

                int finalL = targetL - borderLeft;
                int finalT = targetT - borderTop;
                int finalR = targetR + borderRight;
                int finalB = targetB + borderBottom;

                SetWindowPos(hWnd, IntPtr.Zero, finalL, finalT, finalR - finalL, finalB - finalT, 0x0014); // SWP_NOZORDER | SWP_NOACTIVATE

                RECT winRectAfter, frameAfter;
                GetWindowRect(hWnd, out winRectAfter);
                DwmGetWindowAttribute(hWnd, 9, out frameAfter, 16);

                sb.AppendLine("==========================================");
                sb.AppendLine(string.Format("HWND: 0x{0:X}, Title: '{1}'", hWnd.ToInt64(), title));
                sb.AppendLine(string.Format("Borders: Left={0}, Top={1}, Right={2}, Bottom={3}", borderLeft, borderTop, borderRight, borderBottom));
                sb.AppendLine(string.Format("Target Visible Frame: Left={0}, Top={1}, Right={2}, Bottom={3} [W={4}, H={5}]", targetL, targetT, targetR, targetB, targetR - targetL, targetB - targetT));
                sb.AppendLine(string.Format("SetWindowPos Args:   Left={0}, Top={1}, Right={2}, Bottom={3} [W={4}, H={5}]", finalL, finalT, finalR, finalB, finalR - finalL, finalB - finalT));
                sb.AppendLine(string.Format("RESULT FrameBounds:  Left={0}, Top={1}, Right={2}, Bottom={3} [W={4}, H={5}]", frameAfter.Left, frameAfter.Top, frameAfter.Right, frameAfter.Bottom, frameAfter.Right - frameAfter.Left, frameAfter.Bottom - frameAfter.Top));
            }
            return true;
        });

        EnumDesktopWindows(IntPtr.Zero, callback, IntPtr.Zero);
        File.WriteAllText(outFile, sb.ToString());
        Console.WriteLine("Dumped to " + outFile);
    }
}
