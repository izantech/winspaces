using System.Collections.Generic;

namespace WinSpaces.Gui.Models
{
    public class HotkeyModel
    {
        public uint modifiers { get; set; }
        public uint vk { get; set; }

        public string ToDisplayString()
        {
            if (vk == 0) return "Unassigned";
            var parts = new List<string>();
            if ((modifiers & 0x0002) != 0) parts.Add("Ctrl");
            if ((modifiers & 0x0001) != 0) parts.Add("Alt");
            if ((modifiers & 0x0004) != 0) parts.Add("Shift");
            if ((modifiers & 0x0008) != 0) parts.Add("Win");

            string vkStr = vk switch
            {
                >= 0x41 and <= 0x5A => ((char)vk).ToString(),
                >= 0x30 and <= 0x39 => ((char)vk).ToString(),
                >= 0x70 and <= 0x87 => $"F{vk - 0x70 + 1}",
                0x09 => "Tab",
                0x1B => "Esc",
                0x20 => "Space",
                0x0D => "Enter",
                0x08 => "Backspace",
                0x2E => "Delete",
                0x25 => "Left",
                0x27 => "Right",
                0x26 => "Up",
                0x28 => "Down",
                _ => $"VK{vk}"
            };
            parts.Add(vkStr);
            return string.Join("+", parts);
        }
    }

    public class WindowRectModel
    {
        public int left { get; set; }
        public int top { get; set; }
        public int right { get; set; }
        public int bottom { get; set; }
    }

    public class WorkspaceRuleModel
    {
        public string name { get; set; } = "New Rule";
        public string aumid { get; set; } = string.Empty;
        public string exe_path { get; set; } = string.Empty;
        public string class_name { get; set; } = string.Empty;
        public string title_pattern { get; set; } = string.Empty;
        public int display_index { get; set; }
        public int desktop_index { get; set; }
        public uint show_cmd { get; set; } = 1;
        public WindowRectModel rect { get; set; } = new();
        public bool is_snapped { get; set; }
    }

    public class ConfigModel
    {
        public bool show_all_taskbar { get; set; }
        public bool auto_restore_workspaces { get; set; }
        public bool intercept_win_tab { get; set; } = true;
        public HotkeyModel mission_control { get; set; } = new();
        public List<HotkeyModel> switch_desktops { get; set; } = new();
        public List<HotkeyModel> move_desktops { get; set; } = new();
        public HotkeyModel prev { get; set; } = new();
        public HotkeyModel next { get; set; } = new();
        public HotkeyModel move_prev { get; set; } = new();
        public HotkeyModel move_next { get; set; } = new();
        public List<WorkspaceRuleModel> workspace_rules { get; set; } = new();
    }
}
