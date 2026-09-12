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

mod resolve;
mod spec;

pub use resolve::{layout, trailing_left};
pub use spec::{build_page, hero_texts};

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
    Overview,
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
            HotkeyTarget::Overview => &config.overview,
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

/// Text column inset from the card's left edge (glyph + gap) and gap to the
/// trailing control. Shared with the paint pass so measuring and drawing
/// wrap at the same width.
pub const CARD_TEXT_LEFT: i32 = 56;
pub const CARD_TEXT_GAP: i32 = 12;
/// Vertical offsets of the description block in a headed card.
pub const CARD_DESC_TOP: i32 = 36;
pub const CARD_DESC_BOTTOM_PAD: i32 = 8;

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
