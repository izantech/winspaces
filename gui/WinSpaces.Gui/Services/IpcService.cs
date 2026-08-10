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
                var config = JsonSerializer.Deserialize<ConfigModel>(json, options) ?? new ConfigModel();
                Normalize(config);
                return config;
            }
            catch
            {
                return new ConfigModel();
            }
        }

        // The daemon indexes the desktop hotkey lists 0..4; never hand it a
        // config with short, oversized, or null members.
        private static void Normalize(ConfigModel config)
        {
            config.switch_desktops ??= new();
            config.move_desktops ??= new();
            while (config.switch_desktops.Count < 4) config.switch_desktops.Add(new HotkeyModel());
            while (config.move_desktops.Count < 4) config.move_desktops.Add(new HotkeyModel());
            if (config.switch_desktops.Count > 4) config.switch_desktops.RemoveRange(4, config.switch_desktops.Count - 4);
            if (config.move_desktops.Count > 4) config.move_desktops.RemoveRange(4, config.move_desktops.Count - 4);
            config.mission_control ??= new HotkeyModel();
            config.prev ??= new HotkeyModel();
            config.next ??= new HotkeyModel();
            config.move_prev ??= new HotkeyModel();
            config.move_next ??= new HotkeyModel();
            config.workspace_rules ??= new();
        }

        public static bool SaveConfig(ConfigModel config)
        {
            try
            {
                string path = GetConfigPath();
                var options = new JsonSerializerOptions { WriteIndented = true };
                string json = JsonSerializer.Serialize(config, options);
                File.WriteAllText(path, json);
            }
            catch
            {
                return false;
            }
            NotifyDaemonReload();
            return true;
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
