//! Page content and layout for the settings window. `build_page` describes
//! the current page as a card list; `layout` resolves those into
//! content-space rectangles plus the interactive control map used for
//! hit-testing and focus order. All coordinates are content-space device
//! pixels: the scroll offset is applied by the caller.

use windows_sys::Win32::Foundation::RECT;
use winspaces_common::{hotkey_to_string, Config};
use winspaces_win32::glyphs::{
    GLYPH_ADD, GLYPH_AUTOSTART, GLYPH_KEYBOARD, GLYPH_MONITOR, GLYPH_MOVE, GLYPH_NEXT, GLYPH_PIN,
    GLYPH_PREV, GLYPH_REMOVE, GLYPH_RESTORE, GLYPH_SNAPSHOT, GLYPH_TASKBAR, GLYPH_TASK_VIEW,
    GLYPH_THEME, GLYPH_WORKSPACES,
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    System,
    Tiling,
    Hotkeys,
    Workspaces,
}

impl Page {
    pub const ALL: [Page; 4] = [Page::System, Page::Tiling, Page::Hotkeys, Page::Workspaces];

    pub fn title(self) -> &'static str {
        match self {
            Page::System => "System",
            Page::Tiling => "Tiling Window Manager",
            Page::Hotkeys => "Hotkeys & Spaces",
            Page::Workspaces => "App Workspaces",
        }
    }

    pub fn nav_label(self) -> &'static str {
        match self {
            Page::System => "System",
            Page::Tiling => "Tiling",
            Page::Hotkeys => "Hotkeys & Spaces",
            Page::Workspaces => "App Workspaces",
        }
    }

    pub fn glyph(self) -> u16 {
        match self {
            Page::System => GLYPH_MONITOR,
            Page::Tiling => GLYPH_TASK_VIEW,
            Page::Hotkeys => GLYPH_KEYBOARD,
            Page::Workspaces => GLYPH_WORKSPACES,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HotkeyTarget {
    Mission,
    Switch(usize),
    Move(usize),
    Prev,
    Next,
    ToggleSticky,
    TilingToggle,
    TilingFocusLeft,
    TilingFocusRight,
    TilingFocusUp,
    TilingFocusDown,
    TilingSwapLeft,
    TilingSwapRight,
    TilingSwapUp,
    TilingSwapDown,
    TilingRatioGrow,
    TilingRatioShrink,
    TilingToggleFloat,
}

impl HotkeyTarget {
    pub fn display(self, config: &Config) -> String {
        let hk = match self {
            HotkeyTarget::Mission => &config.mission_control,
            HotkeyTarget::Switch(i) => &config.switch_spaces[i],
            HotkeyTarget::Move(i) => &config.move_spaces[i],
            HotkeyTarget::Prev => &config.prev,
            HotkeyTarget::Next => &config.next,
            HotkeyTarget::ToggleSticky => &config.toggle_sticky,
            HotkeyTarget::TilingToggle => &config.tiling.toggle,
            HotkeyTarget::TilingFocusLeft => &config.tiling.focus_left,
            HotkeyTarget::TilingFocusRight => &config.tiling.focus_right,
            HotkeyTarget::TilingFocusUp => &config.tiling.focus_up,
            HotkeyTarget::TilingFocusDown => &config.tiling.focus_down,
            HotkeyTarget::TilingSwapLeft => &config.tiling.swap_left,
            HotkeyTarget::TilingSwapRight => &config.tiling.swap_right,
            HotkeyTarget::TilingSwapUp => &config.tiling.swap_up,
            HotkeyTarget::TilingSwapDown => &config.tiling.swap_down,
            HotkeyTarget::TilingRatioGrow => &config.tiling.ratio_grow,
            HotkeyTarget::TilingRatioShrink => &config.tiling.ratio_shrink,
            HotkeyTarget::TilingToggleFloat => &config.tiling.toggle_float,
        };
        hotkey_to_string(hk)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ControlId {
    Nav(Page),
    /// Whole-card click that jumps to another page.
    NavCard(Page, u8),
    ToggleShowAll,
    ToggleWinTab,
    ToggleSpaceIndicator,
    ToggleAutostart,
    ToggleAutoRestore,
    ToggleTiling,
    ComboTheme,
    ComboInnerGap,
    ComboOuterGap,
    BtnReload,
    BtnCapture,
    BtnRestore,
    BtnReset,
    BtnExport,
    BtnImport,
    Hotkey(HotkeyTarget),
    RuleDelete(usize),
    FloatRuleDelete(usize),
}

/// Trailing (right-hosted) element of a card.
pub enum Trailing {
    None,
    Toggle(ControlId),
    Button(ControlId, String),
    TwoButtons((ControlId, String), (ControlId, String)),
    Hotkey(HotkeyTarget),
    Combo(ControlId, String),
    /// Daemon status pill + reload button (hero card).
    HeroStatus,
}

pub struct CardSpec {
    pub glyph: u16,
    pub header: String,
    pub desc: String,
    pub trailing: Trailing,
    /// Whole-card click target (nav-link cards).
    pub click: Option<ControlId>,
}

pub enum ItemSpec {
    Card(CardSpec),
    Subtitle(String),
}

fn card(glyph: u16, header: &str, desc: &str, trailing: Trailing) -> ItemSpec {
    ItemSpec::Card(CardSpec {
        glyph,
        header: header.to_string(),
        desc: desc.to_string(),
        trailing,
        click: None,
    })
}

fn nav_card(glyph: u16, header: &str, desc: &str, target: Page, ord: u8) -> ItemSpec {
    ItemSpec::Card(CardSpec {
        glyph,
        header: header.to_string(),
        desc: desc.to_string(),
        trailing: Trailing::None,
        click: Some(ControlId::NavCard(target, ord)),
    })
}

/// Rule row summary text for the workspace-rules list.
pub fn rule_texts(config: &Config, index: usize) -> (String, String) {
    let rule = &config.workspace_rules[index];
    let name = if rule.name.is_empty() {
        "Application Rule".to_string()
    } else {
        rule.name.clone()
    };
    let path_desc = if rule.exe_path.is_empty() {
        rule.class_name.clone()
    } else {
        rule.exe_path.clone()
    };
    let sticky_tag = if rule.is_sticky {
        " \u{2022} Sticky"
    } else {
        ""
    };
    let details = format!(
        "Target: Display {} \u{2022} Space {}{} | Path: {}",
        rule.display_index + 1,
        rule.space_index + 1,
        sticky_tag,
        path_desc
    );
    (name, details)
}

/// A card resolved to content-space rectangles.
pub struct LaidCard {
    pub rect: RECT,
    pub glyph: u16,
    pub header: String,
    pub desc: String,
    pub click: Option<ControlId>,
    pub trailing: LaidTrailing,
}

pub enum LaidTrailing {
    None,
    Toggle(ControlId, RECT),
    Button(ControlId, String, RECT),
    Buttons(Vec<(ControlId, String, RECT)>),
    Hotkey(HotkeyTarget, RECT),
    Combo(ControlId, String, RECT),
    Hero { pill: RECT, btn: RECT },
}

pub enum LaidItem {
    Title(RECT, String),
    Banner(RECT),
    Card(LaidCard),
    Subtitle(RECT, String),
    FooterText(RECT),
    FooterButton(RECT),
}

pub struct Layout {
    pub items: Vec<LaidItem>,
    pub content_h: i32,
    /// Interactive controls in focus order, content-space rects.
    pub controls: Vec<(ControlId, RECT)>,
}

pub struct LayoutParams<'a> {
    /// Left edge and width of the content column, in device pixels.
    pub origin_x: i32,
    pub width: i32,
    pub scale: f32,
    pub banner_open: bool,
    pub page: Page,
    pub config: &'a Config,
    pub machine_name: &'a str,
    pub daemon_running: bool,
    pub theme_label: &'a str,
}

const CARD_H: i32 = 68;
const CARD_GAP: i32 = 8;
const CTL_H: i32 = 32;
const FIELD_MIN_W: i32 = 140;

/// Resolve the page item list into rectangles. `measure_body` / `measure_caption`
/// return the pixel width of a string in the body / caption font.
pub fn layout(
    p: &LayoutParams,
    measure_body: impl Fn(&str) -> i32,
    measure_caption: impl Fn(&str) -> i32,
) -> Layout {
    let px = |v: i32| (v as f32 * p.scale).round() as i32;
    let left = p.origin_x;
    let right = p.origin_x + p.width;
    let mut y = px(16);
    let mut items = Vec::new();
    let mut controls: Vec<(ControlId, RECT)> = Vec::new();

    let hrect = |top: i32, h: i32| RECT {
        left,
        top,
        right,
        bottom: top + h,
    };

    items.push(LaidItem::Title(
        hrect(y, px(40)),
        p.page.title().to_string(),
    ));
    y += px(40) + px(12);

    if p.banner_open {
        items.push(LaidItem::Banner(hrect(y, px(56))));
        y += px(56) + px(CARD_GAP);
    }

    let specs = build_page(p.page, p.config, p.machine_name, p.theme_label);
    for spec in specs {
        match spec {
            ItemSpec::Subtitle(text) => {
                y += px(8);
                items.push(LaidItem::Subtitle(hrect(y, px(36)), text));
                y += px(36) + px(4);
            }
            ItemSpec::Card(card) => {
                let rect = hrect(y, px(CARD_H));
                let ctl_y = rect.top + (px(CARD_H) - px(CTL_H)) / 2;
                let mut trail_right = rect.right - px(16);
                let trailing = match card.trailing {
                    Trailing::None => LaidTrailing::None,
                    Trailing::Toggle(id) => {
                        let r = RECT {
                            left: trail_right - px(40),
                            top: rect.top + (px(CARD_H) - px(20)) / 2,
                            right: trail_right,
                            bottom: rect.top + (px(CARD_H) + px(20)) / 2,
                        };
                        controls.push((id, r));
                        LaidTrailing::Toggle(id, r)
                    }
                    Trailing::Button(id, label) => {
                        let w = measure_body(&label) + px(32);
                        let r = RECT {
                            left: trail_right - w,
                            top: ctl_y,
                            right: trail_right,
                            bottom: ctl_y + px(CTL_H),
                        };
                        controls.push((id, r));
                        LaidTrailing::Button(id, label, r)
                    }
                    Trailing::TwoButtons(first, second) => {
                        // Laid right-to-left so both hug the card's right edge.
                        let mut laid = Vec::new();
                        for (id, label) in [second, first] {
                            let w = measure_body(&label) + px(32);
                            let r = RECT {
                                left: trail_right - w,
                                top: ctl_y,
                                right: trail_right,
                                bottom: ctl_y + px(CTL_H),
                            };
                            trail_right = r.left - px(8);
                            laid.push((id, label, r));
                        }
                        laid.reverse();
                        for (id, _, r) in &laid {
                            controls.push((*id, *r));
                        }
                        LaidTrailing::Buttons(laid)
                    }
                    Trailing::Hotkey(target) => {
                        let label = target.display(p.config);
                        let w = (measure_body(&label) + px(40)).max(px(FIELD_MIN_W));
                        let r = RECT {
                            left: trail_right - w,
                            top: ctl_y,
                            right: trail_right,
                            bottom: ctl_y + px(CTL_H),
                        };
                        controls.push((ControlId::Hotkey(target), r));
                        LaidTrailing::Hotkey(target, r)
                    }
                    Trailing::Combo(id, label) => {
                        let w = (measure_body(&label) + px(56)).max(px(FIELD_MIN_W));
                        let r = RECT {
                            left: trail_right - w,
                            top: ctl_y,
                            right: trail_right,
                            bottom: ctl_y + px(CTL_H),
                        };
                        controls.push((id, r));
                        LaidTrailing::Combo(id, label, r)
                    }
                    Trailing::HeroStatus => {
                        let btn_label = "Reload Daemon";
                        let btn_w = measure_body(btn_label) + px(32);
                        let btn = RECT {
                            left: trail_right - btn_w,
                            top: ctl_y,
                            right: trail_right,
                            bottom: ctl_y + px(CTL_H),
                        };
                        let pill_text = if p.daemon_running {
                            "Daemon Active & Running"
                        } else {
                            "Daemon Stopped"
                        };
                        let pill_w = measure_caption(pill_text) + px(56);
                        let pill = RECT {
                            left: btn.left - px(12) - pill_w,
                            top: rect.top + (px(CARD_H) - px(24)) / 2,
                            right: btn.left - px(12),
                            bottom: rect.top + (px(CARD_H) + px(24)) / 2,
                        };
                        controls.push((ControlId::BtnReload, btn));
                        LaidTrailing::Hero { pill, btn }
                    }
                };
                if let Some(id) = card.click {
                    controls.push((id, rect));
                }
                items.push(LaidItem::Card(LaidCard {
                    rect,
                    glyph: card.glyph,
                    header: card.header,
                    desc: card.desc,
                    click: card.click,
                    trailing,
                }));
                y += px(CARD_H) + px(CARD_GAP);
            }
        }
    }

    // Page footer: reset-to-defaults link.
    y += px(16);
    let footer_text = hrect(y, px(20));
    items.push(LaidItem::FooterText(footer_text));
    y += px(20) + px(8);

    let reset_label = "Reset to Defaults";
    let reset_w = measure_body(reset_label) + px(32);
    let footer_btn = RECT {
        left,
        top: y,
        right: left + reset_w,
        bottom: y + px(CTL_H),
    };
    items.push(LaidItem::FooterButton(footer_btn));
    controls.push((ControlId::BtnReset, footer_btn));
    y += px(CTL_H) + px(32);

    Layout {
        items,
        content_h: y,
        controls,
    }
}

pub fn build_page(
    page: Page,
    config: &Config,
    machine_name: &str,
    theme_label: &str,
) -> Vec<ItemSpec> {
    let mut items = Vec::new();

    // Hero card (machine + daemon status) is shared across all pages.
    items.push(ItemSpec::Card(CardSpec {
        glyph: GLYPH_MONITOR,
        header: machine_name.to_string(),
        desc: "WinSpaces Per-Monitor Spaces Manager for Windows 11".to_string(),
        trailing: Trailing::HeroStatus,
        click: None,
    }));

    match page {
        Page::System => {
            items.push(nav_card(
                GLYPH_TASK_VIEW,
                "Tiling Window Manager",
                "Configure automatic dwindle tiling, gaps, and window layout hotkeys",
                Page::Tiling,
                0,
            ));
            items.push(nav_card(
                GLYPH_MONITOR,
                "Space Switching Shortcuts",
                "Configure global key combinations for spaces 1 through 9",
                Page::Hotkeys,
                1,
            ));
            items.push(nav_card(
                GLYPH_MOVE,
                "Move Window Shortcuts",
                "Send active window directly to specific monitor space",
                Page::Hotkeys,
                2,
            ));
            items.push(nav_card(
                GLYPH_WORKSPACES,
                "App Workspaces & Placement",
                "Assign applications to specific displays and spaces",
                Page::Workspaces,
                3,
            ));
            items.push(card(
                GLYPH_TASKBAR,
                "Show All Windows on Taskbar",
                "Keep windows from inactive spaces visible on the taskbar across space switches",
                Trailing::Toggle(ControlId::ToggleShowAll),
            ));
            items.push(card(
                GLYPH_MONITOR,
                "Intercept Win + Tab for Mission Control",
                "Open native WinSpaces Mission Control overlay when pressing Windows + Tab",
                Trailing::Toggle(ControlId::ToggleWinTab),
            ));
            items.push(card(
                GLYPH_TASK_VIEW,
                "Show Space Indicator",
                "Flash the space name near the taskbar of the display that just switched",
                Trailing::Toggle(ControlId::ToggleSpaceIndicator),
            ));
            items.push(card(
                GLYPH_AUTOSTART,
                "Start WinSpaces at Login",
                "Launch the WinSpaces daemon automatically when you sign in to Windows",
                Trailing::Toggle(ControlId::ToggleAutostart),
            ));
            items.push(card(
                GLYPH_THEME,
                "App Theme",
                "Choose how the WinSpaces settings window is themed",
                Trailing::Combo(ControlId::ComboTheme, theme_label.to_string()),
            ));
            items.push(card(
                GLYPH_SNAPSHOT,
                "Export & Import Configuration",
                "Backup your settings, hotkeys, and workspace rules to a JSON file or restore from a backup",
                Trailing::TwoButtons(
                    (ControlId::BtnExport, "Export Settings...".to_string()),
                    (ControlId::BtnImport, "Import Settings...".to_string()),
                ),
            ));
        }
        Page::Tiling => {
            items.push(ItemSpec::Subtitle("General".to_string()));
            items.push(card(
                GLYPH_TASK_VIEW,
                "Enable Dynamic Tiling",
                "Automatically tile windows in a dwindle spiral layout on managed spaces",
                Trailing::Toggle(ControlId::ToggleTiling),
            ));
            items.push(card(
                GLYPH_MONITOR,
                "Inner Gap (between windows)",
                "Spacing in pixels between adjacent tiled windows",
                Trailing::Combo(
                    ControlId::ComboInnerGap,
                    format!("{} px", config.tiling.inner_gap),
                ),
            ));
            items.push(card(
                GLYPH_MONITOR,
                "Outer Gap (screen edge)",
                "Spacing in pixels between window tiles and monitor work area borders",
                Trailing::Combo(
                    ControlId::ComboOuterGap,
                    format!("{} px", config.tiling.outer_gap),
                ),
            ));
            items.push(ItemSpec::Subtitle("Shortcuts".to_string()));
            items.push(card(
                GLYPH_KEYBOARD,
                "Toggle Tiling Global Shortcut",
                "Quickly enable or disable dynamic tiling",
                Trailing::Hotkey(HotkeyTarget::TilingToggle),
            ));
            items.push(card(
                GLYPH_PIN,
                "Toggle Float Active Window",
                "Exempt or restore active window to/from dynamic tiling",
                Trailing::Hotkey(HotkeyTarget::TilingToggleFloat),
            ));
            items.push(card(
                GLYPH_PREV,
                "Focus Left Tile",
                "Move keyboard focus to neighbor tile on the left",
                Trailing::Hotkey(HotkeyTarget::TilingFocusLeft),
            ));
            items.push(card(
                GLYPH_NEXT,
                "Focus Right Tile",
                "Move keyboard focus to neighbor tile on the right",
                Trailing::Hotkey(HotkeyTarget::TilingFocusRight),
            ));
            items.push(card(
                GLYPH_MOVE,
                "Focus Up Tile",
                "Move keyboard focus to neighbor tile above",
                Trailing::Hotkey(HotkeyTarget::TilingFocusUp),
            ));
            items.push(card(
                GLYPH_MOVE,
                "Focus Down Tile",
                "Move keyboard focus to neighbor tile below",
                Trailing::Hotkey(HotkeyTarget::TilingFocusDown),
            ));
            items.push(card(
                GLYPH_PREV,
                "Swap Left Tile",
                "Swap positions with neighbor tile on the left",
                Trailing::Hotkey(HotkeyTarget::TilingSwapLeft),
            ));
            items.push(card(
                GLYPH_NEXT,
                "Swap Right Tile",
                "Swap positions with neighbor tile on the right",
                Trailing::Hotkey(HotkeyTarget::TilingSwapRight),
            ));
            items.push(card(
                GLYPH_MOVE,
                "Swap Up Tile",
                "Swap positions with neighbor tile above",
                Trailing::Hotkey(HotkeyTarget::TilingSwapUp),
            ));
            items.push(card(
                GLYPH_MOVE,
                "Swap Down Tile",
                "Swap positions with neighbor tile below",
                Trailing::Hotkey(HotkeyTarget::TilingSwapDown),
            ));
            items.push(card(
                GLYPH_ADD,
                "Grow Split Ratio",
                "Increase primary split ratio by step amount",
                Trailing::Hotkey(HotkeyTarget::TilingRatioGrow),
            ));
            items.push(card(
                GLYPH_REMOVE,
                "Shrink Split Ratio",
                "Decrease primary split ratio by step amount",
                Trailing::Hotkey(HotkeyTarget::TilingRatioShrink),
            ));
            items.push(ItemSpec::Subtitle("Window Float Rules".to_string()));
            if config.tiling.float_rules.is_empty() {
                items.push(card(
                    GLYPH_PIN,
                    "",
                    "No floating window rules configured. Applications matching a float rule remain floating across daemon restarts.",
                    Trailing::None,
                ));
            } else {
                for (i, rule) in config.tiling.float_rules.iter().enumerate() {
                    let name = if rule.name.is_empty() {
                        "Float Rule".to_string()
                    } else {
                        rule.name.clone()
                    };
                    let path_desc = if !rule.aumid.is_empty() {
                        format!("AUMID: {}", rule.aumid)
                    } else if !rule.exe_path.is_empty() {
                        format!("Path: {}", rule.exe_path)
                    } else if !rule.class_name.is_empty() {
                        format!("Class: {}", rule.class_name)
                    } else {
                        "Always Float".to_string()
                    };
                    items.push(ItemSpec::Card(CardSpec {
                        glyph: GLYPH_PIN,
                        header: name,
                        desc: path_desc,
                        trailing: Trailing::Button(
                            ControlId::FloatRuleDelete(i),
                            "Delete".to_string(),
                        ),
                        click: None,
                    }));
                }
            }
        }

        Page::Hotkeys => {
            items.push(card(
                GLYPH_MONITOR,
                "Mission Control Shortcut",
                "Toggle full-screen Mission Control spaces and live window thumbnails",
                Trailing::Hotkey(HotkeyTarget::Mission),
            ));
            for i in 0..winspaces_common::MAX_SPACES {
                items.push(card(
                    GLYPH_MONITOR,
                    &format!("Switch Space {}", i + 1),
                    &format!("Focus space {} on current display", i + 1),
                    Trailing::Hotkey(HotkeyTarget::Switch(i)),
                ));
                items.push(card(
                    GLYPH_MOVE,
                    &format!("Move Window to Space {}", i + 1),
                    &format!("Move active window to space {} on current display", i + 1),
                    Trailing::Hotkey(HotkeyTarget::Move(i)),
                ));
            }
            items.push(card(
                GLYPH_PREV,
                "Previous Space",
                "Cycle to previous space",
                Trailing::Hotkey(HotkeyTarget::Prev),
            ));
            items.push(card(
                GLYPH_NEXT,
                "Next Space",
                "Cycle to next space",
                Trailing::Hotkey(HotkeyTarget::Next),
            ));
            items.push(card(
                GLYPH_PIN,
                "Pin Window to Every Space",
                "Keep the active window on screen across every space of its display",
                Trailing::Hotkey(HotkeyTarget::ToggleSticky),
            ));
        }
        Page::Workspaces => {
            items.push(card(
                GLYPH_RESTORE,
                "Auto-Restore Spaces Layout on Launch",
                "Automatically restore saved window placement rules when WinSpaces daemon starts",
                Trailing::Toggle(ControlId::ToggleAutoRestore),
            ));
            items.push(card(
                GLYPH_SNAPSHOT,
                "Layout Snapshot",
                "Snapshot active open windows or trigger window restoration across displays",
                Trailing::TwoButtons(
                    (ControlId::BtnCapture, "Capture Current Layout".to_string()),
                    (ControlId::BtnRestore, "Restore Layout Now".to_string()),
                ),
            ));
            items.push(ItemSpec::Subtitle("Configured Window Rules".to_string()));
            if config.workspace_rules.is_empty() {
                items.push(card(
                    GLYPH_WORKSPACES,
                    "",
                    "No workspace window rules captured yet. Click 'Capture Current Layout' \
                     above to record active open application placements.",
                    Trailing::None,
                ));
            } else {
                for i in 0..config.workspace_rules.len() {
                    let (name, details) = rule_texts(config, i);
                    items.push(ItemSpec::Card(CardSpec {
                        glyph: GLYPH_WORKSPACES,
                        header: name,
                        desc: details,
                        trailing: Trailing::Button(ControlId::RuleDelete(i), "Delete".to_string()),
                        click: None,
                    }));
                }
            }
        }
    }

    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_all_has_four_pages() {
        assert_eq!(Page::ALL.len(), 4);
        assert_eq!(Page::ALL[0], Page::System);
        assert_eq!(Page::ALL[1], Page::Tiling);
        assert_eq!(Page::ALL[2], Page::Hotkeys);
        assert_eq!(Page::ALL[3], Page::Workspaces);
    }

    #[test]
    fn build_page_tiling_contains_expected_controls() {
        let config = Config::default();
        let items = build_page(Page::Tiling, &config, "DESKTOP-TEST", "System Default");
        assert!(items.len() >= 15);

        // Check that toggle tiling and combos are present
        let mut has_toggle_tiling = false;
        let mut has_inner_gap = false;
        let mut has_outer_gap = false;
        let mut has_hotkey_toggle = false;
        let mut has_hotkey_float = false;

        for item in &items {
            if let ItemSpec::Card(c) = item {
                match &c.trailing {
                    Trailing::Toggle(ControlId::ToggleTiling) => has_toggle_tiling = true,
                    Trailing::Combo(ControlId::ComboInnerGap, _) => has_inner_gap = true,
                    Trailing::Combo(ControlId::ComboOuterGap, _) => has_outer_gap = true,
                    Trailing::Hotkey(HotkeyTarget::TilingToggle) => has_hotkey_toggle = true,
                    Trailing::Hotkey(HotkeyTarget::TilingToggleFloat) => has_hotkey_float = true,
                    _ => {}
                }
            }
        }

        assert!(has_toggle_tiling);
        assert!(has_inner_gap);
        assert!(has_outer_gap);
        assert!(has_hotkey_toggle);
        assert!(has_hotkey_float);
    }

    #[test]
    fn tiling_hotkey_targets_display_matches_config() {
        let config = Config::default();
        assert_eq!(
            HotkeyTarget::TilingToggle.display(&config),
            "Ctrl+Alt+Shift+T"
        );
        assert_eq!(
            HotkeyTarget::TilingToggleFloat.display(&config),
            "Ctrl+Alt+Shift+F"
        );
        assert_eq!(
            HotkeyTarget::TilingRatioGrow.display(&config),
            "Ctrl+Alt+Shift++"
        );
        assert_eq!(
            HotkeyTarget::TilingRatioShrink.display(&config),
            "Ctrl+Alt+Shift+-"
        );
    }

    #[test]
    fn build_page_tiling_renders_float_rules() {
        let mut config = Config::default();
        config.tiling.float_rules.push(winspaces_common::FloatRule {
            name: "Calculator App".to_string(),
            aumid: "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App".to_string(),
            exe_path: "".to_string(),
            class_name: "".to_string(),
            title_pattern: "".to_string(),
        });

        let items = build_page(Page::Tiling, &config, "DESKTOP-TEST", "System Default");
        let mut found_delete = false;
        for item in items {
            if let ItemSpec::Card(c) = item {
                if let Trailing::Button(ControlId::FloatRuleDelete(0), _) = c.trailing {
                    found_delete = true;
                    assert_eq!(c.header, "Calculator App");
                }
            }
        }
        assert!(found_delete);
    }
}
