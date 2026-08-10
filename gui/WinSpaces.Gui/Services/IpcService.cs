using System;
using System.IO;
using System.Runtime.InteropServices;
using System.Text.Json;
using WinSpaces.Gui.Models;

namespace WinSpaces.Gui.Services
{
    public static class IpcService
    {
        private const uint WM_USER = 0x0400;
        private const uint WM_WINSPACES_RELOAD_CONFIG = WM_USER + 100;
        private const uint WM_WINSPACES_CAPTURE_WORKSPACE = WM_USER + 101;
        private const uint WM_WINSPACES_RESTORE_WORKSPACE = WM_USER + 102;
        private const string MSG_WINDOW_CLASS = "WinSpacesMessageClass";
        private const string MSG_WINDOW_TITLE = "WinSpacesMessageWindow";

        [DllImport("user32.dll", CharSet = CharSet.Unicode)]
        private static extern IntPtr FindWindowW(string lpClassName, string lpWindowName);

        [DllImport("user32.dll")]
        private static extern bool PostMessageW(IntPtr hWnd, uint Msg, IntPtr wParam, IntPtr lParam);

        public static string GetConfigPath()
        {
            string exeDir = AppDomain.CurrentDomain.BaseDirectory;
            string portable = Path.Combine(exeDir, "settings.json");
            if (File.Exists(portable)) return portable;

            string appData = Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData);
            string dir = Path.Combine(appData, "WinSpaces");
            Directory.CreateDirectory(dir);
            return Path.Combine(dir, "settings.json");
        }

        public static ConfigModel LoadConfig()
        {
            string path = GetConfigPath();
            if (!File.Exists(path)) return new ConfigModel();

            try
            {
                string json = File.ReadAllText(path);
                var options = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
                return JsonSerializer.Deserialize<ConfigModel>(json, options) ?? new ConfigModel();
            }
            catch
            {
                return new ConfigModel();
            }
        }

        public static void SaveConfig(ConfigModel config)
        {
            string path = GetConfigPath();
            var options = new JsonSerializerOptions { WriteIndented = true };
            string json = JsonSerializer.Serialize(config, options);
            File.WriteAllText(path, json);
            NotifyDaemonReload();
        }

        public static bool IsDaemonRunning()
        {
            IntPtr hwnd = FindWindowW(MSG_WINDOW_CLASS, MSG_WINDOW_TITLE);
            return hwnd != IntPtr.Zero;
        }

        public static void NotifyDaemonReload()
        {
            IntPtr hwnd = FindWindowW(MSG_WINDOW_CLASS, MSG_WINDOW_TITLE);
            if (hwnd != IntPtr.Zero)
            {
                PostMessageW(hwnd, WM_WINSPACES_RELOAD_CONFIG, IntPtr.Zero, IntPtr.Zero);
            }
        }

        public static void NotifyCaptureWorkspace()
        {
            IntPtr hwnd = FindWindowW(MSG_WINDOW_CLASS, MSG_WINDOW_TITLE);
            if (hwnd != IntPtr.Zero)
            {
                PostMessageW(hwnd, WM_WINSPACES_CAPTURE_WORKSPACE, IntPtr.Zero, IntPtr.Zero);
            }
        }

        public static void NotifyRestoreWorkspace()
        {
            IntPtr hwnd = FindWindowW(MSG_WINDOW_CLASS, MSG_WINDOW_TITLE);
            if (hwnd != IntPtr.Zero)
            {
                PostMessageW(hwnd, WM_WINSPACES_RESTORE_WORKSPACE, IntPtr.Zero, IntPtr.Zero);
            }
        }
    }
}
