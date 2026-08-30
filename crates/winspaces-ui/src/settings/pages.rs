//! Page content and layout for the settings window. `build_page` describes
//! the current page as a card list; `layout` resolves those into
//! content-space rectangles plus the interactive control map used for
//! hit-testing and focus order. All coordinates are content-space device
//! pixels: the scroll offset is applied by the caller.

use windows_sys::Win32::Foundation::RECT;
use winspaces_common::i18n::t;
use winspaces_common::{hotkey_to_string, tr, Config, Msg};
use winspaces_win32::glyphs::{
    GLYPH_ADD, GLYPH_AUTOSTART, GLYPH_GLOBE, GLYPH_KEYBOARD, GLYPH_MONITOR, GLYPH_MOVE, GLYPH_NEXT,
    GLYPH_PIN, GLYPH_PREV, GLYPH_REMOVE, GLYPH_RESTORE, GLYPH_SHIELD, GLYPH_SNAPSHOT,
    GLYPH_TASKBAR, GLYPH_TASK_VIEW, GLYPH_THEME, GLYPH_WORKSPACES,
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
        t(match self {
            Page::System => Msg::SettingsPageSystemTitle,
            Page::Tiling => Msg::SettingsPageTilingTitle,
            Page::Hotkeys => Msg::SettingsPageHotkeysTitle,
            Page::Workspaces => Msg::SettingsPageWorkspacesTitle,
        })
    }

    pub fn nav_label(self) -> &'static str {
        t(match self {
            Page::System => Msg::SettingsPageSystemNav,
            Page::Tiling => Msg::SettingsPageTilingNav,
            Page::Hotkeys => Msg::SettingsPageHotkeysNav,
            Page::Workspaces => Msg::SettingsPageWorkspacesNav,
        })
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
    TilingToggleSplit,
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
            HotkeyTarget::TilingToggleSplit => &config.tiling.toggle_split,
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
    ToggleElevated,
    ToggleAutoRestore,
    ToggleTiling,
    ComboTheme,
    ComboLanguage,
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
        t(Msg::SettingsWorkspacesRuleDefaultName).to_string()
    } else {
        rule.name.clone()
    };
    let path_desc = if rule.exe_path.is_empty() {
        rule.class_name.clone()
    } else {
        rule.exe_path.clone()
    };
    let sticky_tag = if rule.is_sticky {
        t(Msg::SettingsWorkspacesRuleStickyTag)
    } else {
        ""
    };
    let details = tr!(
        Msg::SettingsWorkspacesRuleDetails,
        display = rule.display_index + 1,
        space = rule.space_index + 1,
        sticky = sticky_tag,
        path = path_desc
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
    pub daemon_elevated: bool,
    pub labels: ComboLabels<'a>,
}

/// Current values shown in the System page combos. Resolved by the caller
/// (theme from the registry pref, language from config) so `build_page`
/// stays pure.
#[derive(Clone, Copy)]
pub struct ComboLabels<'a> {
    pub theme: &'a str,
    pub language: &'a str,
}

/// Hero card status pill and button texts. One source for both the layout
/// pass (which measures them) and the paint pass (which draws them), so a
/// translation can never desync the two.
pub fn hero_texts(daemon_running: bool, daemon_elevated: bool) -> (&'static str, &'static str) {
    let pill = if daemon_running {
        if daemon_elevated {
            t(Msg::SettingsHeroPillAdmin)
        } else {
            t(Msg::SettingsHeroPillRunning)
        }
    } else {
        t(Msg::SettingsHeroPillStopped)
    };
    let btn = if daemon_running {
        t(Msg::SettingsHeroBtnRestart)
    } else {
        t(Msg::SettingsHeroBtnStart)
    };
    (pill, btn)
}

/// Minimum card height. A description that wraps (up to `DESC_MAX_LINES`)
/// grows the card; nothing else does.
const CARD_H: i32 = 68;
const CARD_GAP: i32 = 8;
const CTL_H: i32 = 32;
const FIELD_MIN_W: i32 = 140;
/// Longer descriptions still ellipsize — a card is a summary, not a manual.
const DESC_MAX_LINES: i32 = 2;
/// Text column inset from the card's left edge (glyph + gap) and gap to the
/// trailing control. Shared with the paint pass so measuring and drawing
/// wrap at the same width.
pub const CARD_TEXT_LEFT: i32 = 56;
pub const CARD_TEXT_GAP: i32 = 12;
/// Vertical offsets of the description block in a headed card.
pub const CARD_DESC_TOP: i32 = 36;
pub const CARD_DESC_BOTTOM_PAD: i32 = 8;

fn offset_rect(r: &mut RECT, dy: i32) {
    r.top += dy;
    r.bottom += dy;
}

/// Left edge of the trailing control cluster; the description column ends
/// `CARD_TEXT_GAP` before it.
pub fn trailing_left(trailing: &LaidTrailing, card: &RECT, gap: i32) -> i32 {
    match trailing {
        LaidTrailing::None => card.right - gap,
        LaidTrailing::Toggle(_, r)
        | LaidTrailing::Button(_, _, r)
        | LaidTrailing::Hotkey(_, r)
        | LaidTrailing::Combo(_, _, r) => r.left,
        LaidTrailing::Buttons(list) => list
            .iter()
            .map(|(_, _, r)| r.left)
            .min()
            .unwrap_or(card.right),
        LaidTrailing::Hero { pill, .. } => pill.left,
    }
}

/// Resolve the page item list into rectangles. `measure_body` /
/// `measure_caption` return the pixel width of a string in the body / caption
/// font; `measure_desc(text, width)` returns the height of `text` wrapped to
/// `width` in the caption font.
pub fn layout(
    p: &LayoutParams,
    measure_body: impl Fn(&str) -> i32,
    measure_caption: impl Fn(&str) -> i32,
    measure_desc: impl Fn(&str, i32) -> i32,
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

    let specs = build_page(p.page, p.config, p.machine_name, p.labels);
    for spec in specs {
        match spec {
            ItemSpec::Subtitle(text) => {
                y += px(8);
                items.push(LaidItem::Subtitle(hrect(y, px(36)), text));
                y += px(36) + px(4);
            }
            ItemSpec::Card(card) => {
                let mut rect = hrect(y, px(CARD_H));
                let ctl_y = rect.top + (px(CARD_H) - px(CTL_H)) / 2;
                let mut trail_right = rect.right - px(16);
                let controls_before = controls.len();
                let mut trailing = match card.trailing {
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
                        let (pill_text, btn_label) =
                            hero_texts(p.daemon_running, p.daemon_elevated);
                        let btn_w = measure_body(btn_label) + px(32);
                        let btn = RECT {
                            left: trail_right - btn_w,
                            top: ctl_y,
                            right: trail_right,
                            bottom: ctl_y + px(CTL_H),
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
                // Wrapped description: grow the card to fit up to
                // DESC_MAX_LINES, then re-centre the trailing controls (laid
                // out above against the minimum height) on the taller card.
                let text_left = rect.left + px(CARD_TEXT_LEFT);
                let text_right = trailing_left(&trailing, &rect, px(16)) - px(CARD_TEXT_GAP);
                let text_w = (text_right - text_left).max(px(40));
                let card_h = if card.desc.is_empty() {
                    px(CARD_H)
                } else {
                    let line_h = measure_desc("Xg", i32::MAX / 4).max(1);
                    let desc_h = measure_desc(&card.desc, text_w).min(line_h * DESC_MAX_LINES);
                    let needed = if card.header.is_empty() {
                        desc_h + px(24)
                    } else {
                        px(CARD_DESC_TOP) + desc_h + px(CARD_DESC_BOTTOM_PAD) + px(4)
                    };
                    needed.max(px(CARD_H))
                };
                let dy = (card_h - px(CARD_H)) / 2;
                if dy > 0 {
                    rect.bottom = rect.top + card_h;
                    match &mut trailing {
                        LaidTrailing::None => {}
                        LaidTrailing::Toggle(_, r)
                        | LaidTrailing::Button(_, _, r)
                        | LaidTrailing::Hotkey(_, r)
                        | LaidTrailing::Combo(_, _, r) => offset_rect(r, dy),
                        LaidTrailing::Buttons(list) => {
                            for (_, _, r) in list.iter_mut() {
                                offset_rect(r, dy);
                            }
                        }
                        LaidTrailing::Hero { pill, btn } => {
                            offset_rect(pill, dy);
                            offset_rect(btn, dy);
                        }
                    }
                    for (_, r) in controls.iter_mut().skip(controls_before) {
                        offset_rect(r, dy);
                    }
                }
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
                y += card_h + px(CARD_GAP);
            }
        }
    }

    // Page footer: reset-to-defaults link.
    y += px(16);
    let footer_text = hrect(y, px(20));
    items.push(LaidItem::FooterText(footer_text));
    y += px(20) + px(8);

    let reset_w = measure_body(t(Msg::SettingsFooterReset)) + px(32);
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
    labels: ComboLabels,
) -> Vec<ItemSpec> {
    let mut items = Vec::new();

    // Hero card (machine + daemon status) is shared across all pages.
    items.push(ItemSpec::Card(CardSpec {
        glyph: GLYPH_MONITOR,
        header: machine_name.to_string(),
        desc: t(Msg::SettingsHeroDesc).to_string(),
        trailing: Trailing::HeroStatus,
        click: None,
    }));

    match page {
        Page::System => {
            items.push(nav_card(
                GLYPH_TASK_VIEW,
                t(Msg::SettingsNavTilingTitle),
                t(Msg::SettingsNavTilingDesc),
                Page::Tiling,
                0,
            ));
            items.push(nav_card(
                GLYPH_MONITOR,
                t(Msg::SettingsNavSwitchTitle),
                t(Msg::SettingsNavSwitchDesc),
                Page::Hotkeys,
                1,
            ));
            items.push(nav_card(
                GLYPH_MOVE,
                t(Msg::SettingsNavMoveTitle),
                t(Msg::SettingsNavMoveDesc),
                Page::Hotkeys,
                2,
            ));
            items.push(nav_card(
                GLYPH_WORKSPACES,
                t(Msg::SettingsNavWorkspacesTitle),
                t(Msg::SettingsNavWorkspacesDesc),
                Page::Workspaces,
                3,
            ));
            items.push(card(
                GLYPH_TASKBAR,
                t(Msg::SettingsSystemShowAllTitle),
                t(Msg::SettingsSystemShowAllDesc),
                Trailing::Toggle(ControlId::ToggleShowAll),
            ));
            items.push(card(
                GLYPH_MONITOR,
                t(Msg::SettingsSystemWinTabTitle),
                t(Msg::SettingsSystemWinTabDesc),
                Trailing::Toggle(ControlId::ToggleWinTab),
            ));
            items.push(card(
                GLYPH_TASK_VIEW,
                t(Msg::SettingsSystemIndicatorTitle),
                t(Msg::SettingsSystemIndicatorDesc),
                Trailing::Toggle(ControlId::ToggleSpaceIndicator),
            ));
            items.push(card(
                GLYPH_AUTOSTART,
                t(Msg::SettingsSystemAutostartTitle),
                t(Msg::SettingsSystemAutostartDesc),
                Trailing::Toggle(ControlId::ToggleAutostart),
            ));
            items.push(card(
                GLYPH_SHIELD,
                t(Msg::SettingsSystemElevatedTitle),
                t(Msg::SettingsSystemElevatedDesc),
                Trailing::Toggle(ControlId::ToggleElevated),
            ));
            items.push(card(
                GLYPH_THEME,
                t(Msg::SettingsSystemThemeTitle),
                t(Msg::SettingsSystemThemeDesc),
                Trailing::Combo(ControlId::ComboTheme, labels.theme.to_string()),
            ));
            items.push(card(
                GLYPH_GLOBE,
                t(Msg::SettingsSystemLanguageTitle),
                t(Msg::SettingsSystemLanguageDesc),
                Trailing::Combo(ControlId::ComboLanguage, labels.language.to_string()),
            ));
            items.push(card(
                GLYPH_SNAPSHOT,
                t(Msg::SettingsSystemExportImportTitle),
                t(Msg::SettingsSystemExportImportDesc),
                Trailing::TwoButtons(
                    (
                        ControlId::BtnExport,
                        t(Msg::SettingsSystemBtnExport).to_string(),
                    ),
                    (
                        ControlId::BtnImport,
                        t(Msg::SettingsSystemBtnImport).to_string(),
                    ),
                ),
            ));
        }
        Page::Tiling => {
            items.push(ItemSpec::Subtitle(
                t(Msg::SettingsTilingGeneral).to_string(),
            ));
            items.push(card(
                GLYPH_TASK_VIEW,
                t(Msg::SettingsTilingEnableTitle),
                t(Msg::SettingsTilingEnableDesc),
                Trailing::Toggle(ControlId::ToggleTiling),
            ));
            items.push(card(
                GLYPH_MONITOR,
                t(Msg::SettingsTilingInnerGapTitle),
                t(Msg::SettingsTilingInnerGapDesc),
                Trailing::Combo(
                    ControlId::ComboInnerGap,
                    tr!(Msg::SettingsUnitPx, n = config.tiling.inner_gap),
                ),
            ));
            items.push(card(
                GLYPH_MONITOR,
                t(Msg::SettingsTilingOuterGapTitle),
                t(Msg::SettingsTilingOuterGapDesc),
                Trailing::Combo(
                    ControlId::ComboOuterGap,
                    tr!(Msg::SettingsUnitPx, n = config.tiling.outer_gap),
                ),
            ));
            items.push(ItemSpec::Subtitle(
                t(Msg::SettingsTilingShortcuts).to_string(),
            ));
            let hotkeys: [(u16, Msg, Msg, HotkeyTarget); 13] = [
                (
                    GLYPH_KEYBOARD,
                    Msg::SettingsTilingToggleTitle,
                    Msg::SettingsTilingToggleDesc,
                    HotkeyTarget::TilingToggle,
                ),
                (
                    GLYPH_PIN,
                    Msg::SettingsTilingFloatTitle,
                    Msg::SettingsTilingFloatDesc,
                    HotkeyTarget::TilingToggleFloat,
                ),
                (
                    GLYPH_MOVE,
                    Msg::SettingsTilingSplitTitle,
                    Msg::SettingsTilingSplitDesc,
                    HotkeyTarget::TilingToggleSplit,
                ),
                (
                    GLYPH_PREV,
                    Msg::SettingsTilingFocusLeftTitle,
                    Msg::SettingsTilingFocusLeftDesc,
                    HotkeyTarget::TilingFocusLeft,
                ),
                (
                    GLYPH_NEXT,
                    Msg::SettingsTilingFocusRightTitle,
                    Msg::SettingsTilingFocusRightDesc,
                    HotkeyTarget::TilingFocusRight,
                ),
                (
                    GLYPH_MOVE,
                    Msg::SettingsTilingFocusUpTitle,
                    Msg::SettingsTilingFocusUpDesc,
                    HotkeyTarget::TilingFocusUp,
                ),
                (
                    GLYPH_MOVE,
                    Msg::SettingsTilingFocusDownTitle,
                    Msg::SettingsTilingFocusDownDesc,
                    HotkeyTarget::TilingFocusDown,
                ),
                (
                    GLYPH_PREV,
                    Msg::SettingsTilingSwapLeftTitle,
                    Msg::SettingsTilingSwapLeftDesc,
                    HotkeyTarget::TilingSwapLeft,
                ),
                (
                    GLYPH_NEXT,
                    Msg::SettingsTilingSwapRightTitle,
                    Msg::SettingsTilingSwapRightDesc,
                    HotkeyTarget::TilingSwapRight,
                ),
                (
                    GLYPH_MOVE,
                    Msg::SettingsTilingSwapUpTitle,
                    Msg::SettingsTilingSwapUpDesc,
                    HotkeyTarget::TilingSwapUp,
                ),
                (
                    GLYPH_MOVE,
                    Msg::SettingsTilingSwapDownTitle,
                    Msg::SettingsTilingSwapDownDesc,
                    HotkeyTarget::TilingSwapDown,
                ),
                (
                    GLYPH_ADD,
                    Msg::SettingsTilingGrowTitle,
                    Msg::SettingsTilingGrowDesc,
                    HotkeyTarget::TilingRatioGrow,
                ),
                (
                    GLYPH_REMOVE,
                    Msg::SettingsTilingShrinkTitle,
                    Msg::SettingsTilingShrinkDesc,
                    HotkeyTarget::TilingRatioShrink,
                ),
            ];
            for &(glyph, title, desc, target) in hotkeys.iter() {
                items.push(card(glyph, t(title), t(desc), Trailing::Hotkey(target)));
            }
            items.push(ItemSpec::Subtitle(
                t(Msg::SettingsTilingFloatRules).to_string(),
            ));
            if config.tiling.float_rules.is_empty() {
                items.push(card(
                    GLYPH_PIN,
                    "",
                    t(Msg::SettingsTilingFloatRulesEmpty),
                    Trailing::None,
                ));
            } else {
                for (i, rule) in config.tiling.float_rules.iter().enumerate() {
                    let name = if rule.name.is_empty() {
                        t(Msg::SettingsTilingFloatRuleDefaultName).to_string()
                    } else {
                        rule.name.clone()
                    };
                    let path_desc = if !rule.aumid.is_empty() {
                        tr!(Msg::SettingsTilingFloatRuleAumid, value = rule.aumid)
                    } else if !rule.exe_path.is_empty() {
                        tr!(Msg::SettingsTilingFloatRulePath, value = rule.exe_path)
                    } else if !rule.class_name.is_empty() {
                        tr!(Msg::SettingsTilingFloatRuleClass, value = rule.class_name)
                    } else {
                        t(Msg::SettingsTilingFloatRuleAlways).to_string()
                    };
                    items.push(ItemSpec::Card(CardSpec {
                        glyph: GLYPH_PIN,
                        header: name,
                        desc: path_desc,
                        trailing: Trailing::Button(
                            ControlId::FloatRuleDelete(i),
                            t(Msg::SettingsDelete).to_string(),
                        ),
                        click: None,
                    }));
                }
            }
        }

        Page::Hotkeys => {
            items.push(card(
                GLYPH_MONITOR,
                t(Msg::SettingsHotkeysMissionTitle),
                t(Msg::SettingsHotkeysMissionDesc),
                Trailing::Hotkey(HotkeyTarget::Mission),
            ));
            for i in 0..winspaces_common::MAX_SPACES {
                items.push(card(
                    GLYPH_MONITOR,
                    &tr!(Msg::SettingsHotkeysSwitchTitle, n = i + 1),
                    &tr!(Msg::SettingsHotkeysSwitchDesc, n = i + 1),
                    Trailing::Hotkey(HotkeyTarget::Switch(i)),
                ));
                items.push(card(
                    GLYPH_MOVE,
                    &tr!(Msg::SettingsHotkeysMoveTitle, n = i + 1),
                    &tr!(Msg::SettingsHotkeysMoveDesc, n = i + 1),
                    Trailing::Hotkey(HotkeyTarget::Move(i)),
                ));
            }
            items.push(card(
                GLYPH_PREV,
                t(Msg::SettingsHotkeysPrevTitle),
                t(Msg::SettingsHotkeysPrevDesc),
                Trailing::Hotkey(HotkeyTarget::Prev),
            ));
            items.push(card(
                GLYPH_NEXT,
                t(Msg::SettingsHotkeysNextTitle),
                t(Msg::SettingsHotkeysNextDesc),
                Trailing::Hotkey(HotkeyTarget::Next),
            ));
            items.push(card(
                GLYPH_PIN,
                t(Msg::SettingsHotkeysStickyTitle),
                t(Msg::SettingsHotkeysStickyDesc),
                Trailing::Hotkey(HotkeyTarget::ToggleSticky),
            ));
        }
        Page::Workspaces => {
            items.push(card(
                GLYPH_RESTORE,
                t(Msg::SettingsWorkspacesAutoRestoreTitle),
                t(Msg::SettingsWorkspacesAutoRestoreDesc),
                Trailing::Toggle(ControlId::ToggleAutoRestore),
            ));
            items.push(card(
                GLYPH_SNAPSHOT,
                t(Msg::SettingsWorkspacesSnapshotTitle),
                t(Msg::SettingsWorkspacesSnapshotDesc),
                Trailing::TwoButtons(
                    (
                        ControlId::BtnCapture,
                        t(Msg::SettingsWorkspacesBtnCapture).to_string(),
                    ),
                    (
                        ControlId::BtnRestore,
                        t(Msg::SettingsWorkspacesBtnRestore).to_string(),
                    ),
                ),
            ));
            items.push(ItemSpec::Subtitle(
                t(Msg::SettingsWorkspacesRules).to_string(),
            ));
            if config.workspace_rules.is_empty() {
                items.push(card(
                    GLYPH_WORKSPACES,
                    "",
                    t(Msg::SettingsWorkspacesRulesEmpty),
                    Trailing::None,
                ));
            } else {
                for i in 0..config.workspace_rules.len() {
                    let (name, details) = rule_texts(config, i);
                    items.push(ItemSpec::Card(CardSpec {
                        glyph: GLYPH_WORKSPACES,
                        header: name,
                        desc: details,
                        trailing: Trailing::Button(
                            ControlId::RuleDelete(i),
                            t(Msg::SettingsDelete).to_string(),
                        ),
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

    const TEST_LABELS: ComboLabels<'static> = ComboLabels {
        theme: "System Default",
        language: "English",
    };

    #[test]
    fn build_page_system_contains_language_combo() {
        let config = Config::default();
        let items = build_page(Page::System, &config, "DESKTOP-TEST", TEST_LABELS);
        let found = items.iter().any(|item| {
            matches!(
                item,
                ItemSpec::Card(c)
                    if matches!(&c.trailing, Trailing::Combo(ControlId::ComboLanguage, l) if l == "English")
            )
        });
        assert!(found);
    }

    /// Lay the System page out the way `relayout` does, with a description
    /// measurement that always wraps to two lines so every card grows.
    fn laid_system_page(config: &Config) -> Layout {
        let params = LayoutParams {
            origin_x: 900,
            width: 1000,
            scale: 1.0,
            banner_open: false,
            page: Page::System,
            config,
            machine_name: "DESKTOP-TEST",
            daemon_running: true,
            daemon_elevated: false,
            labels: TEST_LABELS,
        };
        layout(
            &params,
            |s| s.chars().count() as i32 * 8,
            |s| s.chars().count() as i32 * 7,
            |s, w| {
                let line = 18;
                if w >= i32::MAX / 4 {
                    line
                } else {
                    // Two lines for anything that does not fit on one.
                    let one = s.chars().count() as i32 * 7;
                    if one > w {
                        line * 2
                    } else {
                        line
                    }
                }
            },
        )
    }

    /// A click is hit-tested against `Layout::controls` but lands on what
    /// `Layout::items` drew: if a grown card offsets one and not the other,
    /// the control silently stops responding.
    #[test]
    fn every_trailing_control_is_registered_where_it_is_drawn() {
        let config = Config::default();
        let laid = laid_system_page(&config);
        let mut checked = 0;
        for item in &laid.items {
            let LaidItem::Card(card) = item else { continue };
            let mut drawn: Vec<(ControlId, RECT)> = Vec::new();
            match &card.trailing {
                LaidTrailing::None => {}
                LaidTrailing::Toggle(id, r)
                | LaidTrailing::Button(id, _, r)
                | LaidTrailing::Combo(id, _, r) => drawn.push((*id, *r)),
                LaidTrailing::Hotkey(target, r) => drawn.push((ControlId::Hotkey(*target), *r)),
                LaidTrailing::Buttons(list) => {
                    for (id, _, r) in list {
                        drawn.push((*id, *r));
                    }
                }
                LaidTrailing::Hero { btn, .. } => drawn.push((ControlId::BtnReload, *btn)),
            }
            for (id, r) in drawn {
                let registered = laid
                    .controls
                    .iter()
                    .find(|(cid, _)| *cid == id)
                    .unwrap_or_else(|| panic!("{:?} is drawn but never registered", id));
                assert_eq!(
                    (
                        registered.1.left,
                        registered.1.top,
                        registered.1.right,
                        registered.1.bottom
                    ),
                    (r.left, r.top, r.right, r.bottom),
                    "{:?} is hit-tested somewhere else than it is drawn",
                    id
                );
                assert!(
                    r.top >= card.rect.top && r.bottom <= card.rect.bottom,
                    "{:?} sits outside its card ({}..{} vs {}..{})",
                    id,
                    r.top,
                    r.bottom,
                    card.rect.top,
                    card.rect.bottom
                );
                checked += 1;
            }
        }
        assert!(
            checked >= 6,
            "expected the System page cards, got {}",
            checked
        );
    }

    /// The language combo must be reachable: registered, non-degenerate and
    /// not shadowed by an earlier control that overlaps it.
    #[test]
    fn language_combo_is_the_topmost_control_at_its_own_centre() {
        let config = Config::default();
        let laid = laid_system_page(&config);
        let (_, r) = laid
            .controls
            .iter()
            .find(|(id, _)| *id == ControlId::ComboLanguage)
            .expect("language combo is not registered");
        assert!(r.right - r.left >= 140 && r.bottom - r.top > 0);
        let (cx, cy) = ((r.left + r.right) / 2, (r.top + r.bottom) / 2);
        let first = laid
            .controls
            .iter()
            .find(|(id, rect)| {
                !matches!(id, ControlId::NavCard(..))
                    && cx >= rect.left
                    && cx < rect.right
                    && cy >= rect.top
                    && cy < rect.bottom
            })
            .map(|(id, _)| *id);
        assert_eq!(first, Some(ControlId::ComboLanguage));
    }

    #[test]
    fn hero_texts_are_one_source_for_measure_and_draw() {
        assert_eq!(hero_texts(true, false).0, "Daemon Active & Running");
        assert_eq!(hero_texts(true, true).0, "Daemon Active (Admin)");
        assert_eq!(hero_texts(false, false), ("Daemon Stopped", "Start Daemon"));
        assert_eq!(hero_texts(true, true).1, "Restart Daemon");
    }

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
        let items = build_page(Page::Tiling, &config, "DESKTOP-TEST", TEST_LABELS);
        assert!(items.len() >= 15);

        // Check that toggle tiling and combos are present
        let mut has_toggle_tiling = false;
        let mut has_inner_gap = false;
        let mut has_outer_gap = false;
        let mut has_hotkey_toggle = false;
        let mut has_hotkey_float = false;
        let mut has_hotkey_split = false;

        for item in &items {
            if let ItemSpec::Card(c) = item {
                match &c.trailing {
                    Trailing::Toggle(ControlId::ToggleTiling) => has_toggle_tiling = true,
                    Trailing::Combo(ControlId::ComboInnerGap, _) => has_inner_gap = true,
                    Trailing::Combo(ControlId::ComboOuterGap, _) => has_outer_gap = true,
                    Trailing::Hotkey(HotkeyTarget::TilingToggle) => has_hotkey_toggle = true,
                    Trailing::Hotkey(HotkeyTarget::TilingToggleFloat) => has_hotkey_float = true,
                    Trailing::Hotkey(HotkeyTarget::TilingToggleSplit) => has_hotkey_split = true,
                    _ => {}
                }
            }
        }

        assert!(has_toggle_tiling);
        assert!(has_inner_gap);
        assert!(has_outer_gap);
        assert!(has_hotkey_toggle);
        assert!(has_hotkey_float);
        assert!(has_hotkey_split);
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
        assert_eq!(
            HotkeyTarget::TilingToggleSplit.display(&config),
            "Ctrl+Alt+Shift+O"
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

        let items = build_page(Page::Tiling, &config, "DESKTOP-TEST", TEST_LABELS);
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
