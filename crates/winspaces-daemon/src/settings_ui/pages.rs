//! Page content and layout for the settings window. `build_page` describes
//! the current page as a card list; `layout` resolves those into
//! content-space rectangles plus the interactive control map used for
//! hit-testing and focus order. All coordinates are content-space device
//! pixels: the scroll offset is applied by the caller.

use windows_sys::Win32::Foundation::RECT;
use winspaces_common::{hotkey_to_string, Config};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    System,
    Hotkeys,
    Workspaces,
}

impl Page {
    pub const ALL: [Page; 3] = [Page::System, Page::Hotkeys, Page::Workspaces];

    pub fn title(self) -> &'static str {
        match self {
            Page::System => "System",
            Page::Hotkeys => "Hotkeys & Desktops",
            Page::Workspaces => "App Workspaces",
        }
    }

    pub fn nav_label(self) -> &'static str {
        match self {
            Page::System => "System",
            Page::Hotkeys => "Hotkeys & Desktops",
            Page::Workspaces => "App Workspaces",
        }
    }

    pub fn glyph(self) -> u16 {
        match self {
            Page::System => 0xE7F4,
            Page::Hotkeys => 0xE92C,
            Page::Workspaces => 0xEA37,
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
}

impl HotkeyTarget {
    pub fn display(self, config: &Config) -> String {
        let hk = match self {
            HotkeyTarget::Mission => &config.mission_control,
            HotkeyTarget::Switch(i) => &config.switch_desktops[i],
            HotkeyTarget::Move(i) => &config.move_desktops[i],
            HotkeyTarget::Prev => &config.prev,
            HotkeyTarget::Next => &config.next,
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
    ToggleAnimations,
    ToggleAutostart,
    ToggleAutoRestore,
    ComboTheme,
    BtnReload,
    BtnCapture,
    BtnRestore,
    BtnReset,
    Hotkey(HotkeyTarget),
    RuleDelete(usize),
}

/// Trailing (right-hosted) element of a card.
pub enum Trailing {
    None,
    Toggle(ControlId),
    Button(ControlId, String),
    TwoButtons((ControlId, String), (ControlId, String)),
    Hotkey(HotkeyTarget),
    Combo,
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
    let details = format!(
        "Target: Display {} \u{2022} Space {} | Path: {}",
        rule.display_index + 1,
        rule.desktop_index + 1,
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
    Combo(RECT),
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

    let specs = build_page(p.page, p.config, p.machine_name);
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
                    Trailing::Combo => {
                        let w = (measure_body(p.theme_label) + px(56)).max(px(FIELD_MIN_W));
                        let r = RECT {
                            left: trail_right - w,
                            top: ctl_y,
                            right: trail_right,
                            bottom: ctl_y + px(CTL_H),
                        };
                        controls.push((ControlId::ComboTheme, r));
                        LaidTrailing::Combo(r)
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

    // Footer: caption text left, Reset Defaults button right.
    y += px(12);
    let reset_label = "Reset Defaults";
    let reset_w = measure_body(reset_label) + px(32);
    let footer_btn = RECT {
        left: right - reset_w,
        top: y,
        right,
        bottom: y + px(CTL_H),
    };
    items.push(LaidItem::FooterText(RECT {
        left,
        top: y,
        right: footer_btn.left - px(16),
        bottom: y + px(CTL_H),
    }));
    items.push(LaidItem::FooterButton(footer_btn));
    controls.push((ControlId::BtnReset, footer_btn));
    y += px(CTL_H) + px(32);

    Layout {
        items,
        content_h: y,
        controls,
    }
}

pub fn build_page(page: Page, config: &Config, machine_name: &str) -> Vec<ItemSpec> {
    let mut items = Vec::new();

    // Hero card (machine + daemon status) is shared across all pages.
    items.push(ItemSpec::Card(CardSpec {
        glyph: 0xE7F4,
        header: machine_name.to_string(),
        desc: "WinSpaces Per-Monitor Virtual Desktop Manager for Windows 11".to_string(),
        trailing: Trailing::HeroStatus,
        click: None,
    }));

    match page {
        Page::System => {
            items.push(nav_card(
                0xE7F4,
                "Desktop Switching Shortcuts",
                "Configure global key combinations for desktops 1 through 9",
                Page::Hotkeys,
                0,
            ));
            items.push(nav_card(
                0xE898,
                "Move Window Shortcuts",
                "Send active window directly to specific monitor desktop",
                Page::Hotkeys,
                1,
            ));
            items.push(nav_card(
                0xEA37,
                "App Workspaces & Placement",
                "Assign applications to specific displays and desktop spaces",
                Page::Workspaces,
                2,
            ));
            items.push(card(
                0xE737,
                "Show All Windows on Taskbar",
                "Keep inactive desktop windows visible on taskbar across space switches",
                Trailing::Toggle(ControlId::ToggleShowAll),
            ));
            items.push(card(
                0xE7F4,
                "Intercept Win + Tab for Mission Control",
                "Open native WinSpaces Mission Control overlay when pressing Windows + Tab",
                Trailing::Toggle(ControlId::ToggleWinTab),
            ));
            items.push(card(
                0xE916,
                "Animate Mission Control Transitions",
                "Play open, close, and space-switch transitions; honors the system \"Show animations in Windows\" setting",
                Trailing::Toggle(ControlId::ToggleAnimations),
            ));
            items.push(card(
                0xE7E8,
                "Start WinSpaces at Login",
                "Launch the WinSpaces daemon automatically when you sign in to Windows",
                Trailing::Toggle(ControlId::ToggleAutostart),
            ));
            items.push(card(
                0xE790,
                "App Theme",
                "Choose how the WinSpaces settings window is themed",
                Trailing::Combo,
            ));
        }
        Page::Hotkeys => {
            items.push(card(
                0xE7F4,
                "Mission Control Shortcut",
                "Toggle full-screen Mission Control spaces and live window thumbnails",
                Trailing::Hotkey(HotkeyTarget::Mission),
            ));
            for i in 0..winspaces_common::MAX_DESKTOPS {
                items.push(card(
                    0xE7F4,
                    &format!("Switch Desktop {}", i + 1),
                    &format!("Focus desktop {} on current display", i + 1),
                    Trailing::Hotkey(HotkeyTarget::Switch(i)),
                ));
                items.push(card(
                    0xE898,
                    &format!("Move Window to Desk {}", i + 1),
                    &format!("Move active window to desktop {} on current display", i + 1),
                    Trailing::Hotkey(HotkeyTarget::Move(i)),
                ));
            }
            items.push(card(
                0xE892,
                "Previous Desktop",
                "Cycle to previous desktop",
                Trailing::Hotkey(HotkeyTarget::Prev),
            ));
            items.push(card(
                0xE893,
                "Next Desktop",
                "Cycle to next desktop",
                Trailing::Hotkey(HotkeyTarget::Next),
            ));
        }
        Page::Workspaces => {
            items.push(card(
                0xE777,
                "Auto-Restore Desktop Layout on Launch",
                "Automatically restore saved window placement rules when WinSpaces daemon starts",
                Trailing::Toggle(ControlId::ToggleAutoRestore),
            ));
            items.push(card(
                0xE7C5,
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
                    0xEA37,
                    "",
                    "No workspace window rules captured yet. Click 'Capture Current Layout' \
                     above to record active open application placements.",
                    Trailing::None,
                ));
            } else {
                for i in 0..config.workspace_rules.len() {
                    let (name, details) = rule_texts(config, i);
                    items.push(ItemSpec::Card(CardSpec {
                        glyph: 0xEA37,
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
