using System;
using System.IO;
using Microsoft.Win32;

namespace WinSpaces.Gui.Services
{
    /// <summary>
    /// Manages the HKCU Run entry that launches the WinSpaces daemon at login.
    /// </summary>
    public static class AutostartService
    {
        private const string RunKeyPath = @"Software\Microsoft\Windows\CurrentVersion\Run";
        private const string AppName = "WinSpaces";
        private const string DaemonExe = "winspaces.exe";

        public static string GetDaemonPath()
        {
            return Path.Combine(AppContext.BaseDirectory, DaemonExe);
        }

        public static bool IsEnabled()
        {
            try
            {
                using var key = Registry.CurrentUser.OpenSubKey(RunKeyPath);
                return key?.GetValue(AppName) != null;
            }
            catch
            {
                return false;
            }
        }

        public static bool SetEnabled(bool enable)
        {
            try
            {
                using var key = Registry.CurrentUser.CreateSubKey(RunKeyPath);
                if (key == null) return false;
                if (enable)
                {
                    key.SetValue(AppName, $"\"{GetDaemonPath()}\"");
                }
                else
                {
                    key.DeleteValue(AppName, throwOnMissingValue: false);
                }
                return true;
            }
            catch
            {
                return false;
            }
        }
    }
}
