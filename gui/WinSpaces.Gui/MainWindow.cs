using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using Microsoft.UI;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Shapes;
using Windows.Graphics;
using Windows.System;
using WinSpaces.Gui.Models;
using WinSpaces.Gui.Services;
using WinRT.Interop;

namespace WinSpaces.Gui
{
    public class MainWindow : Window
    {
        [DllImport("user32.dll")]
        private static extern uint GetDpiForWindow(IntPtr hWnd);

        [DllImport("user32.dll")]
        private static extern short GetAsyncKeyState(int vKey);

        private ConfigModel _config = new();
        private bool _isDaemonActive;

        // Dark Fluent Theme Brushes
        private readonly Brush _windowBg = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 20, 20, 20));
        private readonly Brush _sidebarBg = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 26, 26, 26));
        private readonly Brush _cardBg = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 32, 32, 32));
        private readonly Brush _cardBorder = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 50, 50, 50));
        private readonly Brush _accentBrush = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 0, 120, 212));
        private readonly Brush _textPrimary = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 240, 240, 240));
        private readonly Brush _textSecondary = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 160, 160, 160));

        private Border _badgeDaemon = null!;
        private Ellipse _dotDaemon = null!;
        private TextBlock _txtDaemonStatus = null!;
        private ToggleSwitch _toggleShowAllTaskbar = null!;
        private ToggleSwitch _toggleAutoRestoreWorkspaces = null!;
        private ToggleSwitch _toggleInterceptWinTab = null!;

        private Border _alertBanner = null!;
        private TextBlock _txtAlertTitle = null!;
        private TextBlock _txtAlertMsg = null!;

        private StackPanel _panelSystem = null!;
        private StackPanel _panelHotkeys = null!;
        private StackPanel _panelWorkspaces = null!;
        private StackPanel _panelWorkspaceRulesStack = null!;
        private TextBlock _txtHeaderTitle = null!;

        private Button _btnNavSystem = null!;
        private Button _btnNavHotkeys = null!;
        private Button _btnNavWorkspaces = null!;

        private Button[] _btnSwitches = new Button[4];
        private Button[] _btnMoves = new Button[4];
        private Button _btnPrev = null!;
        private Button _btnNext = null!;
        private Button _btnMissionControl = null!;

        private Button? _capturingButton;
        private string? _capturingTag;

        public MainWindow()
        {
            Title = "WinSpaces Settings";

            // Enable Windows 11 Mica backdrop & titlebar extension
            SystemBackdrop = new MicaBackdrop();
            ExtendsContentIntoTitleBar = true;

            // Load Config
            _config = IpcService.LoadConfig();

            // Build UI
            BuildUI();

            // Set Titlebar drag element
            SetTitleBar(GetTitleBarElement());

            // Window sizing with DPI scaling
            IntPtr hwnd = WindowNative.GetWindowHandle(this);
            uint dpi = GetDpiForWindow(hwnd);
            double scale = dpi > 0 ? dpi / 96.0 : 1.0;
            AppWindow.Resize(new SizeInt32((int)(960 * scale), (int)(720 * scale)));

            RefreshDaemonStatus();
            UpdateHotkeyButtons();
            UpdateWorkspaceRulesUI();
        }

        private UIElement GetTitleBarElement()
        {
            var titleGrid = new Grid
            {
                Height = 44,
                Padding = new Thickness(16, 0, 0, 0),
                HorizontalAlignment = HorizontalAlignment.Stretch
            };

            var stack = new StackPanel
            {
                Orientation = Orientation.Horizontal,
                VerticalAlignment = VerticalAlignment.Center
            };

            var icon = new FontIcon { Glyph = "\uE713", FontSize = 16, Foreground = _accentBrush, Margin = new Thickness(0, 0, 12, 0) };
            var text = new TextBlock { Text = "WinSpaces Settings", FontSize = 12, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold, Foreground = _textPrimary, VerticalAlignment = VerticalAlignment.Center };

            stack.Children.Add(icon);
            stack.Children.Add(text);
            titleGrid.Children.Add(stack);

            return titleGrid;
        }

        private void BuildUI()
        {
            var rootGrid = new Grid { Background = _windowBg };
            rootGrid.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
            rootGrid.RowDefinitions.Add(new RowDefinition { Height = new GridLength(1, GridUnitType.Star) });

            var titleBar = GetTitleBarElement();
            Grid.SetRow((FrameworkElement)titleBar, 0);
            rootGrid.Children.Add(titleBar);

            var mainLayout = new Grid();
            mainLayout.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(240) });
            mainLayout.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });

            // Left Sidebar
            var sidebarBorder = new Border
            {
                Background = _sidebarBg,
                BorderBrush = _cardBorder,
                BorderThickness = new Thickness(0, 0, 1, 0),
                Padding = new Thickness(12, 16, 12, 16)
            };

            var sidebarStack = new StackPanel { Spacing = 8 };

            _btnNavSystem = CreateSidebarButton("System", "\uE7F4", true);
            _btnNavSystem.Click += (s, e) => SwitchTab("system");

            _btnNavHotkeys = CreateSidebarButton("Hotkeys & Desktops", "\uE92C", false);
            _btnNavHotkeys.Click += (s, e) => SwitchTab("hotkeys");

            _btnNavWorkspaces = CreateSidebarButton("App Workspaces", "\uEA37", false);
            _btnNavWorkspaces.Click += (s, e) => SwitchTab("workspaces");

            sidebarStack.Children.Add(_btnNavSystem);
            sidebarStack.Children.Add(_btnNavHotkeys);
            sidebarStack.Children.Add(_btnNavWorkspaces);
            sidebarBorder.Child = sidebarStack;

            Grid.SetColumn(sidebarBorder, 0);
            mainLayout.Children.Add(sidebarBorder);

            // Right Main Content Region
            var scroll = new ScrollViewer
            {
                VerticalScrollBarVisibility = ScrollBarVisibility.Auto,
                HorizontalScrollBarVisibility = ScrollBarVisibility.Disabled,
                Padding = new Thickness(32, 16, 32, 32)
            };

            var mainContentPanel = new StackPanel { Spacing = 16 };

            _txtHeaderTitle = new TextBlock { Text = "System", FontSize = 24, FontWeight = Microsoft.UI.Text.FontWeights.Bold, Foreground = _textPrimary, Margin = new Thickness(0, 0, 0, 8) };
            mainContentPanel.Children.Add(_txtHeaderTitle);

            // Alert Banner
            _alertBanner = CreateAlertBanner();
            mainContentPanel.Children.Add(_alertBanner);

            // Hero Card: System Status
            var heroCard = CreateHeroCard();
            mainContentPanel.Children.Add(heroCard);

            // System Panel Cards
            _panelSystem = new StackPanel { Spacing = 8, Visibility = Visibility.Visible };
            _panelSystem.Children.Add(CreateClickableCard("Desktop Switching Shortcuts", "Configure global key combinations for desktops 1 through 4", "\uE7F4", (s, e) => SwitchTab("hotkeys")));
            _panelSystem.Children.Add(CreateClickableCard("Move Window Shortcuts", "Send active window directly to specific monitor desktop", "\uE898", (s, e) => SwitchTab("hotkeys")));
            _panelSystem.Children.Add(CreateClickableCard("App Workspaces & Placement", "Assign applications to specific displays and desktop spaces", "\uEA37", (s, e) => SwitchTab("workspaces")));

            _toggleShowAllTaskbar = new ToggleSwitch { IsOn = _config.show_all_taskbar, OnContent = "", OffContent = "", VerticalAlignment = VerticalAlignment.Center };
            _toggleShowAllTaskbar.Toggled += (s, e) =>
            {
                _config.show_all_taskbar = _toggleShowAllTaskbar.IsOn;
                AutoSave("Taskbar visibility mode updated");
            };
            _panelSystem.Children.Add(CreateControlCard("Show All Windows on Taskbar", "Keep inactive desktop windows visible on taskbar across space switches", "\uE737", _toggleShowAllTaskbar));

            _toggleInterceptWinTab = new ToggleSwitch { IsOn = _config.intercept_win_tab, OnContent = "", OffContent = "", VerticalAlignment = VerticalAlignment.Center };
            _toggleInterceptWinTab.Toggled += (s, e) =>
            {
                _config.intercept_win_tab = _toggleInterceptWinTab.IsOn;
                AutoSave("Intercept Win + Tab preference updated");
            };
            _panelSystem.Children.Add(CreateControlCard("Intercept Win + Tab for Mission Control", "Open native WinSpaces Mission Control overlay when pressing Windows + Tab", "\uE7F4", _toggleInterceptWinTab));

            mainContentPanel.Children.Add(_panelSystem);

            // Hotkeys Detail Panel Cards
            _panelHotkeys = new StackPanel { Spacing = 8, Visibility = Visibility.Collapsed };

            _btnMissionControl = new Button { Width = 140, Content = _config.mission_control?.ToDisplayString() ?? "Ctrl+Up" };
            _btnMissionControl.Click += (s, e) => StartHotkeyCapture(_btnMissionControl, "mission:0");
            _panelHotkeys.Children.Add(CreateControlCard("Mission Control Shortcut", "Toggle full-screen Mission Control spaces and live window thumbnails", "\uE7F4", _btnMissionControl));

            for (int i = 0; i < 4; i++)
            {
                int index = i;
                _btnSwitches[i] = new Button { Width = 140, Content = GetHotkeyString(_config.switch_desktops, i, $"Alt+{i + 1}") };
                _btnSwitches[i].Click += (s, e) => StartHotkeyCapture(_btnSwitches[index], $"switch:{index}");

                _btnMoves[i] = new Button { Width = 140, Content = GetHotkeyString(_config.move_desktops, i, $"Ctrl+Alt+{i + 1}") };
                _btnMoves[i].Click += (s, e) => StartHotkeyCapture(_btnMoves[index], $"move:{index}");

                _panelHotkeys.Children.Add(CreateControlCard($"Switch Desktop {i + 1}", $"Focus desktop {i + 1} on current display", "\uE7F4", _btnSwitches[i]));
                _panelHotkeys.Children.Add(CreateControlCard($"Move Window to Desk {i + 1}", $"Move active window to desktop {i + 1} on current display", "\uE898", _btnMoves[i]));
            }

            _btnPrev = new Button { Width = 140, Content = _config.prev?.ToDisplayString() ?? "Alt+Left" };
            _btnPrev.Click += (s, e) => StartHotkeyCapture(_btnPrev, "prev:0");
            _panelHotkeys.Children.Add(CreateControlCard("Previous Desktop", "Cycle to previous desktop", "\uE892", _btnPrev));

            _btnNext = new Button { Width = 140, Content = _config.next?.ToDisplayString() ?? "Alt+Right" };
            _btnNext.Click += (s, e) => StartHotkeyCapture(_btnNext, "next:0");
            _panelHotkeys.Children.Add(CreateControlCard("Next Desktop", "Cycle to next desktop", "\uE893", _btnNext));

            mainContentPanel.Children.Add(_panelHotkeys);

            // Workspaces Detail Panel Cards
            _panelWorkspaces = new StackPanel { Spacing = 12, Visibility = Visibility.Collapsed };

            _toggleAutoRestoreWorkspaces = new ToggleSwitch { IsOn = _config.auto_restore_workspaces, OnContent = "", OffContent = "", VerticalAlignment = VerticalAlignment.Center };
            _toggleAutoRestoreWorkspaces.Toggled += (s, e) =>
            {
                _config.auto_restore_workspaces = _toggleAutoRestoreWorkspaces.IsOn;
                AutoSave("Auto-restore preference updated");
            };
            _panelWorkspaces.Children.Add(CreateControlCard("Auto-Restore Desktop Layout on Launch", "Automatically restore saved window placement rules when WinSpaces daemon starts", "\uE777", _toggleAutoRestoreWorkspaces));

            // Action Buttons Card
            var actionBorder = new Border
            {
                Background = _cardBg,
                BorderBrush = _cardBorder,
                BorderThickness = new Thickness(1),
                CornerRadius = new CornerRadius(8),
                Padding = new Thickness(16, 12, 16, 12)
            };
            var actionGrid = new Grid();
            actionGrid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
            actionGrid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });

            var txtActionDesc = new TextBlock
            {
                Text = "Snapshot active open windows or trigger window restoration across displays",
                FontSize = 12,
                Foreground = _textSecondary,
                VerticalAlignment = VerticalAlignment.Center
            };

            var btnStack = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };

            var btnCapture = new Button { Content = "📸 Capture Current Layout", Padding = new Thickness(12, 6, 12, 6) };
            btnCapture.Click += (s, e) =>
            {
                IpcService.NotifyCaptureWorkspace();
                System.Threading.Thread.Sleep(300);
                _config = IpcService.LoadConfig();
                UpdateWorkspaceRulesUI();
                ShowAlert("Layout Captured", $"Snapshot saved {_config.workspace_rules.Count} window workspace rules.");
            };

            var btnRestore = new Button { Content = "📐 Restore Layout Now", Padding = new Thickness(12, 6, 12, 6) };
            btnRestore.Click += (s, e) =>
            {
                IpcService.NotifyRestoreWorkspace();
                ShowAlert("Layout Restored", "Restored open windows to target display and desktop spaces.");
            };

            btnStack.Children.Add(btnCapture);
            btnStack.Children.Add(btnRestore);

            Grid.SetColumn(txtActionDesc, 0);
            Grid.SetColumn(btnStack, 1);
            actionGrid.Children.Add(txtActionDesc);
            actionGrid.Children.Add(btnStack);
            actionBorder.Child = actionGrid;
            _panelWorkspaces.Children.Add(actionBorder);

            // Workspace Rules Container Header
            var txtRulesHeader = new TextBlock
            {
                Text = "Configured Window Rules",
                FontSize = 16,
                FontWeight = Microsoft.UI.Text.FontWeights.SemiBold,
                Foreground = _textPrimary,
                Margin = new Thickness(0, 8, 0, 0)
            };
            _panelWorkspaces.Children.Add(txtRulesHeader);

            _panelWorkspaceRulesStack = new StackPanel { Spacing = 8 };
            _panelWorkspaces.Children.Add(_panelWorkspaceRulesStack);

            mainContentPanel.Children.Add(_panelWorkspaces);

            // Bottom Action Bar: Instant Auto-Save Status & Reset Defaults
            var actionBar = new Grid { Margin = new Thickness(0, 20, 0, 0) };
            var txtAutoSaveStatus = new TextBlock
            {
                Text = "⚡ Changes are saved automatically",
                FontSize = 12,
                Foreground = _textSecondary,
                VerticalAlignment = VerticalAlignment.Center,
                HorizontalAlignment = HorizontalAlignment.Left
            };

            var actionStack = new StackPanel { Orientation = Orientation.Horizontal, HorizontalAlignment = HorizontalAlignment.Right };

            var btnReset = new Button { Content = "Reset Defaults", Padding = new Thickness(16, 8, 16, 8) };
            btnReset.Click += BtnReset_Click;

            actionStack.Children.Add(btnReset);
            actionBar.Children.Add(txtAutoSaveStatus);
            actionBar.Children.Add(actionStack);
            mainContentPanel.Children.Add(actionBar);

            scroll.Content = mainContentPanel;
            Grid.SetColumn(scroll, 1);
            mainLayout.Children.Add(scroll);

            Grid.SetRow(mainLayout, 1);
            rootGrid.Children.Add(mainLayout);

            rootGrid.KeyDown += RootGrid_KeyDown;
            Content = rootGrid;
        }

        private void UpdateWorkspaceRulesUI()
        {
            if (_panelWorkspaceRulesStack == null) return;
            _panelWorkspaceRulesStack.Children.Clear();

            if (_config.workspace_rules.Count == 0)
            {
                var emptyBorder = new Border
                {
                    Background = _cardBg,
                    BorderBrush = _cardBorder,
                    BorderThickness = new Thickness(1),
                    CornerRadius = new CornerRadius(8),
                    Padding = new Thickness(20)
                };
                var emptyText = new TextBlock
                {
                    Text = "No workspace window rules captured yet. Click 'Capture Current Layout' above to record active open application placements.",
                    FontSize = 12,
                    Foreground = _textSecondary,
                    TextWrapping = TextWrapping.Wrap
                };
                emptyBorder.Child = emptyText;
                _panelWorkspaceRulesStack.Children.Add(emptyBorder);
                return;
            }

            for (int i = 0; i < _config.workspace_rules.Count; i++)
            {
                int index = i;
                var rule = _config.workspace_rules[i];

                var border = new Border
                {
                    Background = _cardBg,
                    BorderBrush = _cardBorder,
                    BorderThickness = new Thickness(1),
                    CornerRadius = new CornerRadius(8),
                    Padding = new Thickness(16, 12, 16, 12)
                };

                var grid = new Grid();
                grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
                grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });

                var stack = new StackPanel { Spacing = 2 };
                var titleText = new TextBlock
                {
                    Text = string.IsNullOrEmpty(rule.name) ? "Application Rule" : rule.name,
                    FontSize = 14,
                    FontWeight = Microsoft.UI.Text.FontWeights.SemiBold,
                    Foreground = _textPrimary
                };

                string pathDesc = string.IsNullOrEmpty(rule.exe_path) ? rule.class_name : rule.exe_path;
                var descText = new TextBlock
                {
                    Text = $"Target: Display {rule.display_index + 1} • Space {rule.desktop_index + 1} | Path: {pathDesc}",
                    FontSize = 12,
                    Foreground = _textSecondary
                };

                stack.Children.Add(titleText);
                stack.Children.Add(descText);

                var btnDelete = new Button
                {
                    Content = "🗑️",
                    Padding = new Thickness(8, 4, 8, 4),
                    VerticalAlignment = VerticalAlignment.Center
                };
                btnDelete.Click += (s, e) =>
                {
                    _config.workspace_rules.RemoveAt(index);
                    UpdateWorkspaceRulesUI();
                    AutoSave("Workspace rule removed");
                };

                Grid.SetColumn(stack, 0);
                Grid.SetColumn(btnDelete, 1);
                grid.Children.Add(stack);
                grid.Children.Add(btnDelete);

                border.Child = grid;
                _panelWorkspaceRulesStack.Children.Add(border);
            }
        }

        private Button CreateSidebarButton(string title, string glyph, bool selected)
        {
            var stack = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 12 };
            stack.Children.Add(new FontIcon { Glyph = glyph, FontSize = 16, Foreground = selected ? _accentBrush : _textSecondary });
            stack.Children.Add(new TextBlock { Text = title, FontSize = 14, FontWeight = selected ? Microsoft.UI.Text.FontWeights.SemiBold : Microsoft.UI.Text.FontWeights.Normal, Foreground = selected ? _textPrimary : _textSecondary });

            return new Button
            {
                Content = stack,
                HorizontalAlignment = HorizontalAlignment.Stretch,
                HorizontalContentAlignment = HorizontalAlignment.Left,
                Padding = new Thickness(12, 10, 12, 10),
                Background = selected ? new SolidColorBrush(Windows.UI.Color.FromArgb(255, 38, 38, 38)) : new SolidColorBrush(Microsoft.UI.Colors.Transparent),
                BorderThickness = new Thickness(0),
                CornerRadius = new CornerRadius(6)
            };
        }

        private Border CreateAlertBanner()
        {
            _alertBanner = new Border
            {
                Background = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 20, 46, 29)),
                BorderBrush = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 37, 82, 52)),
                BorderThickness = new Thickness(1),
                CornerRadius = new CornerRadius(8),
                Padding = new Thickness(16, 12, 16, 12),
                Visibility = Visibility.Collapsed
            };

            var stack = new StackPanel { Spacing = 4 };
            _txtAlertTitle = new TextBlock { FontSize = 14, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold, Foreground = new SolidColorBrush(Microsoft.UI.Colors.White) };
            _txtAlertMsg = new TextBlock { FontSize = 12, Foreground = _textSecondary };

            stack.Children.Add(_txtAlertTitle);
            stack.Children.Add(_txtAlertMsg);
            _alertBanner.Child = stack;

            return _alertBanner;
        }

        private Border CreateHeroCard()
        {
            var border = new Border
            {
                Background = _cardBg,
                BorderBrush = _cardBorder,
                BorderThickness = new Thickness(1),
                CornerRadius = new CornerRadius(8),
                Padding = new Thickness(20, 16, 20, 16)
            };

            var grid = new Grid();
            grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
            grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
            grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });

            var icon = new FontIcon { Glyph = "\uE7F4", FontSize = 24, Foreground = _accentBrush, VerticalAlignment = VerticalAlignment.Center, Margin = new Thickness(0, 0, 16, 0) };
            Grid.SetColumn(icon, 0);

            var infoStack = new StackPanel { VerticalAlignment = VerticalAlignment.Center };
            var txtMachine = new TextBlock { Text = Environment.MachineName, FontSize = 18, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold, Foreground = _textPrimary };
            var txtDesc = new TextBlock { Text = "WinSpaces Per-Monitor Virtual Desktop Manager for Windows 11", FontSize = 12, Foreground = _textSecondary };
            infoStack.Children.Add(txtMachine);
            infoStack.Children.Add(txtDesc);
            Grid.SetColumn(infoStack, 1);

            var statusStack = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 12, VerticalAlignment = VerticalAlignment.Center };

            _badgeDaemon = new Border
            {
                Background = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 20, 46, 29)),
                BorderBrush = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 37, 82, 52)),
                BorderThickness = new Thickness(1),
                CornerRadius = new CornerRadius(12),
                Padding = new Thickness(12, 6, 12, 6),
                VerticalAlignment = VerticalAlignment.Center
            };

            var badgeStack = new StackPanel { Orientation = Orientation.Horizontal };
            _dotDaemon = new Ellipse { Width = 8, Height = 8, Fill = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 16, 185, 129)), Margin = new Thickness(0, 0, 8, 0) };
            _txtDaemonStatus = new TextBlock { Text = "Checking...", FontSize = 12, FontWeight = Microsoft.UI.Text.FontWeights.Medium, Foreground = new SolidColorBrush(Microsoft.UI.Colors.White) };

            badgeStack.Children.Add(_dotDaemon);
            badgeStack.Children.Add(_txtDaemonStatus);
            _badgeDaemon.Child = badgeStack;

            var btnReload = new Button { Content = "Reload Daemon", Padding = new Thickness(12, 6, 12, 6) };
            btnReload.Click += (s, e) => RefreshDaemonStatus();

            statusStack.Children.Add(_badgeDaemon);
            statusStack.Children.Add(btnReload);
            Grid.SetColumn(statusStack, 2);

            grid.Children.Add(icon);
            grid.Children.Add(infoStack);
            grid.Children.Add(statusStack);

            border.Child = grid;
            return border;
        }

        private Border CreateControlCard(string title, string description, string glyph, UIElement rightControl)
        {
            var border = new Border
            {
                Background = _cardBg,
                BorderBrush = _cardBorder,
                BorderThickness = new Thickness(1),
                CornerRadius = new CornerRadius(8),
                Padding = new Thickness(16, 12, 16, 12)
            };

            var grid = new Grid();
            grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
            grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
            grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });

            var icon = new FontIcon { Glyph = glyph, FontSize = 18, Foreground = _accentBrush, VerticalAlignment = VerticalAlignment.Center, Margin = new Thickness(0, 0, 16, 0) };
            Grid.SetColumn(icon, 0);

            var infoStack = new StackPanel { VerticalAlignment = VerticalAlignment.Center };
            var txtTitle = new TextBlock { Text = title, FontSize = 14, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold, Foreground = _textPrimary };
            var txtDesc = new TextBlock { Text = description, FontSize = 12, Foreground = _textSecondary };
            infoStack.Children.Add(txtTitle);
            infoStack.Children.Add(txtDesc);
            Grid.SetColumn(infoStack, 1);

            Grid.SetColumn((FrameworkElement)rightControl, 2);

            grid.Children.Add(icon);
            grid.Children.Add(infoStack);
            grid.Children.Add(rightControl);

            border.Child = grid;
            return border;
        }

        private Button CreateClickableCard(string title, string description, string glyph, RoutedEventHandler clickHandler)
        {
            var arrowIcon = new FontIcon { Glyph = "\uE974", FontSize = 12, Foreground = _textSecondary, VerticalAlignment = VerticalAlignment.Center };
            var cardBorder = CreateControlCard(title, description, glyph, arrowIcon);

            var btn = new Button
            {
                HorizontalAlignment = HorizontalAlignment.Stretch,
                HorizontalContentAlignment = HorizontalAlignment.Stretch,
                Padding = new Thickness(0),
                Background = new SolidColorBrush(Microsoft.UI.Colors.Transparent),
                BorderThickness = new Thickness(0),
                Content = cardBorder
            };
            btn.Click += clickHandler;
            return btn;
        }

        private void RefreshDaemonStatus()
        {
            _isDaemonActive = IpcService.IsDaemonRunning();
            if (_isDaemonActive)
            {
                _txtDaemonStatus.Text = "Daemon Active & Running";
                _dotDaemon.Fill = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 16, 185, 129));
                _badgeDaemon.Background = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 20, 46, 29));
                _badgeDaemon.BorderBrush = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 37, 82, 52));
            }
            else
            {
                _txtDaemonStatus.Text = "Daemon Stopped";
                _dotDaemon.Fill = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 239, 68, 68));
                _badgeDaemon.Background = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 69, 10, 10));
                _badgeDaemon.BorderBrush = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 127, 29, 29));
            }
        }

        private string GetHotkeyString(List<HotkeyModel> list, int index, string fallback)
        {
            if (index >= 0 && index < list.Count)
            {
                return list[index].ToDisplayString();
            }
            return fallback;
        }

        private void UpdateHotkeyButtons()
        {
            for (int i = 0; i < 4; i++)
            {
                if (i < _config.switch_desktops.Count) _btnSwitches[i].Content = _config.switch_desktops[i].ToDisplayString();
                if (i < _config.move_desktops.Count) _btnMoves[i].Content = _config.move_desktops[i].ToDisplayString();
            }
            if (_config.prev != null) _btnPrev.Content = _config.prev.ToDisplayString();
            if (_config.next != null) _btnNext.Content = _config.next.ToDisplayString();
            if (_config.mission_control != null) _btnMissionControl.Content = _config.mission_control.ToDisplayString();
        }

        private void SwitchTab(string tag)
        {
            if (tag == "hotkeys")
            {
                _txtHeaderTitle.Text = "Hotkeys & Desktops";
                _panelSystem.Visibility = Visibility.Collapsed;
                _panelHotkeys.Visibility = Visibility.Visible;
                _panelWorkspaces.Visibility = Visibility.Collapsed;
                _btnNavSystem.Background = new SolidColorBrush(Microsoft.UI.Colors.Transparent);
                _btnNavHotkeys.Background = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 38, 38, 38));
                _btnNavWorkspaces.Background = new SolidColorBrush(Microsoft.UI.Colors.Transparent);
            }
            else if (tag == "workspaces")
            {
                _txtHeaderTitle.Text = "App Workspaces";
                _panelSystem.Visibility = Visibility.Collapsed;
                _panelHotkeys.Visibility = Visibility.Collapsed;
                _panelWorkspaces.Visibility = Visibility.Visible;
                _btnNavSystem.Background = new SolidColorBrush(Microsoft.UI.Colors.Transparent);
                _btnNavHotkeys.Background = new SolidColorBrush(Microsoft.UI.Colors.Transparent);
                _btnNavWorkspaces.Background = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 38, 38, 38));
            }
            else
            {
                _txtHeaderTitle.Text = "System";
                _panelSystem.Visibility = Visibility.Visible;
                _panelHotkeys.Visibility = Visibility.Collapsed;
                _panelWorkspaces.Visibility = Visibility.Collapsed;
                _btnNavSystem.Background = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 38, 38, 38));
                _btnNavHotkeys.Background = new SolidColorBrush(Microsoft.UI.Colors.Transparent);
                _btnNavWorkspaces.Background = new SolidColorBrush(Microsoft.UI.Colors.Transparent);
            }
        }

        private void StartHotkeyCapture(Button btn, string tag)
        {
            if (_capturingButton != null) UpdateHotkeyButtons();
            _capturingButton = btn;
            _capturingTag = tag;
            btn.Content = "Press keys...";
        }

        private void RootGrid_KeyDown(object sender, KeyRoutedEventArgs e)
        {
            if (_capturingButton == null || string.IsNullOrEmpty(_capturingTag)) return;

            e.Handled = true;

            if (e.Key == VirtualKey.Escape)
            {
                _capturingButton = null;
                _capturingTag = null;
                UpdateHotkeyButtons();
                return;
            }

            uint modifiers = 0;
            if ((GetAsyncKeyState(0x11) & 0x8000) != 0) modifiers |= 0x0002; // Ctrl
            if ((GetAsyncKeyState(0x12) & 0x8000) != 0) modifiers |= 0x0001; // Alt
            if ((GetAsyncKeyState(0x10) & 0x8000) != 0) modifiers |= 0x0004; // Shift
            if ((GetAsyncKeyState(0x5B) & 0x8000) != 0 || (GetAsyncKeyState(0x5C) & 0x8000) != 0) modifiers |= 0x0008; // Win

            uint vk = (uint)e.Key;

            if (vk == 0x11 || vk == 0x12 || vk == 0x10 || vk == 0x5B || vk == 0x5C) return;

            var newHk = new HotkeyModel { modifiers = modifiers, vk = vk };

            var parts = _capturingTag.Split(':');
            string action = parts[0];
            int index = int.Parse(parts[1]);

            if (action == "switch" && index < _config.switch_desktops.Count) _config.switch_desktops[index] = newHk;
            else if (action == "move" && index < _config.move_desktops.Count) _config.move_desktops[index] = newHk;
            else if (action == "prev") _config.prev = newHk;
            else if (action == "next") _config.next = newHk;
            else if (action == "mission") _config.mission_control = newHk;

            _capturingButton = null;
            _capturingTag = null;
            UpdateHotkeyButtons();
            AutoSave("Recorded new hotkey shortcut");
        }

        private void AutoSave(string reason = "Settings auto-saved")
        {
            IpcService.SaveConfig(_config);
            RefreshDaemonStatus();
            ShowAlert("Auto-Saved", $"{reason}. WinSpaces daemon reloaded live via Win32 IPC.");
        }

        private void BtnReset_Click(object sender, RoutedEventArgs e)
        {
            _config = new ConfigModel();
            UpdateHotkeyButtons();
            UpdateWorkspaceRulesUI();
            AutoSave("Reset all settings to defaults");
        }

        private void ShowAlert(string title, string message)
        {
            _txtAlertTitle.Text = title;
            _txtAlertMsg.Text = message;
            _alertBanner.Visibility = Visibility.Visible;
        }
    }
}
