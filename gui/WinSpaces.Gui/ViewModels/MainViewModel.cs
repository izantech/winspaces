using System;
using System.Collections.ObjectModel;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using Microsoft.UI.Xaml.Controls;
using WinSpaces.Gui.Models;
using WinSpaces.Gui.Services;

namespace WinSpaces.Gui.ViewModels
{
    public partial class MainViewModel : ObservableObject
    {
        [ObservableProperty]
        private string _computerName = Environment.MachineName;

        [ObservableProperty]
        private string _daemonStatusText = "Checking...";

        [ObservableProperty]
        private bool _isDaemonActive;

        [ObservableProperty]
        private bool _showAllTaskbar;

        [ObservableProperty]
        private bool _autoRestoreWorkspaces;

        [ObservableProperty]
        private string _infoBarTitle = string.Empty;

        [ObservableProperty]
        private string _infoBarMessage = string.Empty;

        [ObservableProperty]
        private bool _isInfoBarOpen;

        [ObservableProperty]
        private InfoBarSeverity _infoBarSeverity = InfoBarSeverity.Informational;

        public ObservableCollection<HotkeyModel> SwitchDesktops { get; } = new();
        public ObservableCollection<HotkeyModel> MoveDesktops { get; } = new();
        public ObservableCollection<WorkspaceRuleModel> WorkspaceRules { get; } = new();

        [ObservableProperty]
        private HotkeyModel _prevHotkey = new();

        [ObservableProperty]
        private HotkeyModel _nextHotkey = new();

        private ConfigModel _config = new();

        public MainViewModel()
        {
            LoadConfig();
            RefreshDaemonStatus();
        }

        [RelayCommand]
        public void RefreshDaemonStatus()
        {
            IsDaemonActive = IpcService.IsDaemonRunning();
            DaemonStatusText = IsDaemonActive ? "Daemon Active & Running" : "Daemon Stopped";
        }

        public void LoadConfig()
        {
            _config = IpcService.LoadConfig();
            ShowAllTaskbar = _config.show_all_taskbar;
            AutoRestoreWorkspaces = _config.auto_restore_workspaces;

            SwitchDesktops.Clear();
            foreach (var hk in _config.switch_desktops) SwitchDesktops.Add(hk);
            while (SwitchDesktops.Count < 4) SwitchDesktops.Add(new HotkeyModel());

            MoveDesktops.Clear();
            foreach (var hk in _config.move_desktops) MoveDesktops.Add(hk);
            while (MoveDesktops.Count < 4) MoveDesktops.Add(new HotkeyModel());

            PrevHotkey = _config.prev ?? new HotkeyModel();
            NextHotkey = _config.next ?? new HotkeyModel();

            WorkspaceRules.Clear();
            foreach (var rule in _config.workspace_rules) WorkspaceRules.Add(rule);
        }

        [RelayCommand]
        public void SaveConfig()
        {
            _config.show_all_taskbar = ShowAllTaskbar;
            _config.auto_restore_workspaces = AutoRestoreWorkspaces;

            _config.switch_desktops.Clear();
            foreach (var hk in SwitchDesktops) _config.switch_desktops.Add(hk);

            _config.move_desktops.Clear();
            foreach (var hk in MoveDesktops) _config.move_desktops.Add(hk);

            _config.prev = PrevHotkey;
            _config.next = NextHotkey;

            _config.workspace_rules.Clear();
            foreach (var r in WorkspaceRules) _config.workspace_rules.Add(r);

            IpcService.SaveConfig(_config);
            RefreshDaemonStatus();

            ShowInfo("Settings Applied", "Config saved to settings.json and WinSpaces daemon reloaded live via Win32 IPC.", InfoBarSeverity.Success);
        }

        [RelayCommand]
        public void CaptureWorkspaceLayout()
        {
            IpcService.NotifyCaptureWorkspace();
            System.Threading.Thread.Sleep(300);
            LoadConfig();
            ShowInfo("Layout Captured", $"Captured {WorkspaceRules.Count} active window rules into Workspaces profile.", InfoBarSeverity.Success);
        }

        [RelayCommand]
        public void RestoreWorkspaceLayout()
        {
            IpcService.NotifyRestoreWorkspace();
            ShowInfo("Layout Restored", "Restored windows to configured display and desktop spaces.", InfoBarSeverity.Informational);
        }

        [RelayCommand]
        public void AddWorkspaceRule()
        {
            WorkspaceRules.Add(new WorkspaceRuleModel { name = "Custom App Rule", display_index = 0, desktop_index = 0 });
        }

        [RelayCommand]
        public void RemoveWorkspaceRule(WorkspaceRuleModel rule)
        {
            if (rule != null && WorkspaceRules.Contains(rule))
            {
                WorkspaceRules.Remove(rule);
            }
        }

        [RelayCommand]
        public void ResetDefaults()
        {
            _config = new ConfigModel();
            LoadConfig();
            ShowInfo("Defaults Restored", "All desktop shortcuts and options have been reset to defaults.", InfoBarSeverity.Informational);
        }

        public void ShowInfo(string title, string message, InfoBarSeverity severity)
        {
            InfoBarTitle = title;
            InfoBarMessage = message;
            InfoBarSeverity = severity;
            IsInfoBarOpen = true;
        }
    }
}
