//! What each settings page contains: the ordered card specs, their texts
//! and trailing controls. Pure over `Config` and the UI language; no
//! geometry.

use super::*;

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
                t(Msg::SettingsHotkeysOverviewTitle),
                t(Msg::SettingsHotkeysOverviewDesc),
                Trailing::Hotkey(HotkeyTarget::Overview),
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
