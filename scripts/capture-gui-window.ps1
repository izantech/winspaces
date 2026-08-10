Add-Type -TypeDefinition @"
using System;
using System.Text;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;

public class WindowCapturer {
    [DllImport("user32.dll")]
    public static extern bool EnumWindows(EnumWindowsProc enumProc, IntPtr lParam);

    [DllImport("user32.dll")]
    public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint lpdwProcessId);

    [DllImport("user32.dll")]
    public static extern bool IsWindowVisible(IntPtr hWnd);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern int GetWindowTextW(IntPtr hWnd, StringBuilder lpString, int nMaxCount);

    [DllImport("user32.dll")]
    public static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);

    [DllImport("user32.dll")]
    public static extern bool PrintWindow(IntPtr hwnd, IntPtr hdcBmp, uint nFlags);

    public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);

    [StructLayout(LayoutKind.Sequential)]
    public struct RECT {
        public int Left;
        public int Top;
        public int Right;
        public int Bottom;
    }

    public static void CaptureProcessWindow(uint pid, string outputPath) {
        EnumWindows((hWnd, lParam) => {
            GetWindowThreadProcessId(hWnd, out uint windowPid);
            if (windowPid == pid && IsWindowVisible(hWnd)) {
                StringBuilder sb = new StringBuilder(256);
                GetWindowTextW(hWnd, sb, 256);
                string title = sb.ToString();

                GetWindowRect(hWnd, out RECT rect);
                int width = rect.Right - rect.Left;
                int height = rect.Bottom - rect.Top;

                Console.WriteLine($"Found Window: HWND=0x{hWnd.ToInt64():X}, Title='{title}', Bounds=({rect.Left},{rect.Top},{width}x{height})");

                if (width > 0 && height > 0) {
                    using (Bitmap bmp = new Bitmap(width, height)) {
                        using (Graphics g = Graphics.FromImage(bmp)) {
                            IntPtr hdc = g.GetHdc();
                            try {
                                PrintWindow(hWnd, hdc, 2); // PW_RENDERFULLCONTENT
                            } finally {
                                g.ReleaseHdc(hdc);
                            }
                        }
                        bmp.Save(outputPath, ImageFormat.Png);
                        Console.WriteLine($"Successfully captured window screenshot to {outputPath}");
                    }
                }
            }
            return true;
        }, IntPtr.Zero);
    }
}
"@ -ReferencedAssemblies "System.Drawing"

$procs = Get-Process WinSpaces.Gui -ErrorAction SilentlyContinue
if ($procs) {
    $outPath = "C:\Users\Izan\.gemini\antigravity-cli\brain\05643fa2-b6ae-4eb9-bc3c-9bb933b5046f\winspaces_gui_captured.png"
    foreach ($p in $procs) {
        Write-Host "Checking Process PID=$($p.Id)..."
        [WindowCapturer]::CaptureProcessWindow($p.Id, $outPath)
    }
} else {
    Write-Host "No WinSpaces.Gui process running."
}
