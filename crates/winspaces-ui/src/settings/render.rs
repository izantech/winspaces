//! `WM_PAINT`: the nav rail, the laid-out content items, the status banner,
//! and the overlay scrollbar.

use super::layout::{focused_control, px_of, scrollbar_rects};
use super::pages::{ControlId, LaidItem, LaidTrailing, Page};
use super::{controls, Vis, Win};
use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateCompatibleDC, DeleteDC, EndPaint, GetDC, ReleaseDC, DT_CENTER,
    DT_END_ELLIPSIS, DT_LEFT, DT_SINGLELINE, DT_VCENTER, HDC, PAINTSTRUCT,
};
use winspaces_win32::gdi::draw::{draw_glyph_in, draw_text_in, fill_round, measure_text};
use winspaces_win32::gdi::surface::paint_surface;
use winspaces_win32::glyphs::{GLYPH_COMPLETED, GLYPH_ERROR};

pub(crate) unsafe fn on_paint(hwnd: HWND) {
    let mut ps: PAINTSTRUCT = std::mem::zeroed();
    let hdc = BeginPaint(hwnd, &mut ps);
    super::WIN.with(|s| {
        if let Ok(borrow) = s.try_borrow() {
            if let Some(win) = borrow.as_ref() {
                paint_surface(hdc, win.client_w, win.client_h, &win.pal.tint(), |mem| {
                    draw_all(mem, win);
                });
            }
        }
    });
    EndPaint(hwnd, &ps);
}

unsafe fn draw_all(hdc: HDC, win: &Win) {
    let px = px_of(win.scale);
    let pal = &win.pal;
    let fonts = &win.fonts;
    let scale_px = px(1).max(1);

    // Nav rail.
    for (i, page) in Page::ALL.iter().enumerate() {
        let r = win.nav_rects[i];
        let selected = win.state.page == *page;
        let hovered = win.hover == Some(ControlId::Nav(*page));
        if selected || hovered {
            fill_round(
                hdc,
                &r,
                px(4),
                if selected { pal.card } else { pal.card_hover },
                if selected {
                    pal.card_stroke
                } else {
                    pal.card_hover
                },
            );
        }
        if selected {
            let pill = RECT {
                left: r.left,
                top: r.top + (r.bottom - r.top - px(16)) / 2,
                right: r.left + px(3),
                bottom: r.top + (r.bottom - r.top + px(16)) / 2,
            };
            fill_round(hdc, &pill, px(3), pal.accent, pal.accent);
        }
        let glyph_rect = RECT {
            left: r.left + px(10),
            top: r.top,
            right: r.left + px(38),
            bottom: r.bottom,
        };
        draw_glyph_in(
            hdc,
            fonts.glyph,
            pal.text,
            &glyph_rect,
            page.glyph(),
            DT_SINGLELINE | DT_VCENTER | DT_CENTER,
        );
        let text_rect = RECT {
            left: r.left + px(44),
            top: r.top,
            right: r.right - px(8),
            bottom: r.bottom,
        };
        draw_text_in(
            hdc,
            if selected {
                fonts.body_strong
            } else {
                fonts.body
            },
            pal.text,
            &text_rect,
            page.nav_label(),
            DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_END_ELLIPSIS,
        );
        if win.keyboard_nav && focused_control(win) == Some(ControlId::Nav(*page)) {
            controls::draw_focus_ring(hdc, &r, pal, scale_px);
        }
    }

    // Content items (content-space rects shifted by scroll).
    let shift = |r: &RECT| RECT {
        left: r.left,
        top: r.top - win.scroll,
        right: r.right,
        bottom: r.bottom - win.scroll,
    };
    let visible = |r: &RECT| r.bottom - win.scroll > 0 && r.top - win.scroll < win.client_h;

    let focused = if win.keyboard_nav {
        focused_control(win)
    } else {
        None
    };
    let vis_for = |id: ControlId| Vis {
        hover: win.hover == Some(id),
        pressed: win.pressed == Some(id),
        focus: focused == Some(id),
    };

    for item in &win.layout.items {
        match item {
            LaidItem::Title(r, text) => {
                if visible(r) {
                    draw_text_in(
                        hdc,
                        fonts.title,
                        pal.text,
                        &shift(r),
                        text,
                        DT_SINGLELINE | DT_VCENTER | DT_LEFT,
                    );
                }
            }
            LaidItem::Subtitle(r, text) => {
                if visible(r) {
                    draw_text_in(
                        hdc,
                        fonts.subtitle,
                        pal.text,
                        &shift(r),
                        text,
                        DT_SINGLELINE | DT_VCENTER | DT_LEFT,
                    );
                }
            }
            LaidItem::Banner(r) => {
                if visible(r) {
                    draw_banner(hdc, win, &shift(r));
                }
            }
            LaidItem::FooterText(r) => {
                if visible(r) {
                    draw_text_in(
                        hdc,
                        fonts.caption,
                        pal.text_dim,
                        &shift(r),
                        "Changes are saved automatically",
                        DT_SINGLELINE | DT_VCENTER | DT_LEFT,
                    );
                }
            }
            LaidItem::FooterButton(r) => {
                if visible(r) {
                    controls::draw_button(
                        hdc,
                        &shift(r),
                        "Reset Defaults",
                        false,
                        vis_for(ControlId::BtnReset),
                        pal,
                        fonts,
                        scale_px,
                    );
                }
            }
            LaidItem::Card(card) => {
                if !visible(&card.rect) {
                    continue;
                }
                let r = shift(&card.rect);
                let clickable = card.click.is_some();
                let card_vis = card.click.map(vis_for).unwrap_or_default();
                let fill = if clickable && card_vis.pressed {
                    pal.card_pressed
                } else if clickable && card_vis.hover {
                    pal.card_hover
                } else {
                    pal.card
                };
                fill_round(hdc, &r, px(4), fill, pal.card_stroke);
                if card_vis.focus {
                    controls::draw_focus_ring(hdc, &r, pal, scale_px);
                }

                // Leading glyph.
                let glyph_rect = RECT {
                    left: r.left + px(16),
                    top: r.top,
                    right: r.left + px(44),
                    bottom: r.bottom,
                };
                draw_glyph_in(
                    hdc,
                    fonts.glyph,
                    pal.text,
                    &glyph_rect,
                    card.glyph,
                    DT_SINGLELINE | DT_VCENTER | DT_CENTER,
                );

                // Text block, clipped at the trailing control.
                let trail_left = match &card.trailing {
                    LaidTrailing::None => r.right - px(16),
                    LaidTrailing::Toggle(_, tr) => tr.left,
                    LaidTrailing::Button(_, _, tr) => tr.left,
                    LaidTrailing::Buttons(list) => list
                        .iter()
                        .map(|(_, _, tr)| tr.left)
                        .min()
                        .unwrap_or(r.right),
                    LaidTrailing::Hotkey(_, tr) => tr.left,
                    LaidTrailing::Combo(tr) => tr.left,
                    LaidTrailing::Hero { pill, .. } => pill.left,
                };
                let text_right = trail_left - px(12);
                if card.header.is_empty() {
                    let text_rect = RECT {
                        left: r.left + px(56),
                        top: r.top,
                        right: text_right,
                        bottom: r.bottom,
                    };
                    draw_text_in(
                        hdc,
                        fonts.caption,
                        pal.text_dim,
                        &text_rect,
                        &card.desc,
                        DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_END_ELLIPSIS,
                    );
                } else {
                    let header_rect = RECT {
                        left: r.left + px(56),
                        top: r.top + px(14),
                        right: text_right,
                        bottom: r.top + px(36),
                    };
                    draw_text_in(
                        hdc,
                        fonts.body,
                        pal.text,
                        &header_rect,
                        &card.header,
                        DT_SINGLELINE | DT_LEFT | DT_END_ELLIPSIS,
                    );
                    let desc_rect = RECT {
                        left: r.left + px(56),
                        top: r.top + px(36),
                        right: text_right,
                        bottom: r.bottom - px(8),
                    };
                    draw_text_in(
                        hdc,
                        fonts.caption,
                        pal.text_dim,
                        &desc_rect,
                        &card.desc,
                        DT_SINGLELINE | DT_LEFT | DT_END_ELLIPSIS,
                    );
                }

                // Trailing control.
                match &card.trailing {
                    LaidTrailing::None => {}
                    LaidTrailing::Toggle(id, tr) => {
                        let on = match id {
                            ControlId::ToggleShowAll => win.state.config.show_all_taskbar,
                            ControlId::ToggleWinTab => win.state.config.intercept_win_tab,
                            ControlId::ToggleSpaceIndicator => win.state.config.space_indicator,
                            ControlId::ToggleAutoRestore => {
                                win.state.config.auto_restore_workspaces
                            }
                            ControlId::ToggleAutostart => win.state.autostart,
                            _ => false,
                        };
                        controls::draw_toggle(hdc, &shift(tr), on, vis_for(*id), pal, scale_px);
                    }
                    LaidTrailing::Button(id, label, tr) => {
                        controls::draw_button(
                            hdc,
                            &shift(tr),
                            label,
                            false,
                            vis_for(*id),
                            pal,
                            fonts,
                            scale_px,
                        );
                    }
                    LaidTrailing::Buttons(list) => {
                        for (id, label, tr) in list {
                            controls::draw_button(
                                hdc,
                                &shift(tr),
                                label,
                                false,
                                vis_for(*id),
                                pal,
                                fonts,
                                scale_px,
                            );
                        }
                    }
                    LaidTrailing::Hotkey(target, tr) => {
                        let capturing = win.state.capturing == Some(*target);
                        let label = if capturing {
                            "Press keys...".to_string()
                        } else {
                            target.display(&win.state.config)
                        };
                        controls::draw_field(
                            hdc,
                            &shift(tr),
                            &label,
                            capturing,
                            false,
                            vis_for(ControlId::Hotkey(*target)),
                            pal,
                            fonts,
                            scale_px,
                        );
                    }
                    LaidTrailing::Combo(tr) => {
                        controls::draw_field(
                            hdc,
                            &shift(tr),
                            win.state.theme_pref.label(),
                            false,
                            true,
                            vis_for(ControlId::ComboTheme),
                            pal,
                            fonts,
                            scale_px,
                        );
                    }
                    LaidTrailing::Hero { pill, btn } => {
                        let (text, dot, bg) = if win.state.daemon_running {
                            ("Daemon Active & Running", pal.success, pal.success_bg)
                        } else {
                            ("Daemon Stopped", pal.critical, pal.critical_bg)
                        };
                        controls::draw_pill(hdc, &shift(pill), text, dot, bg, pal, fonts, scale_px);
                        controls::draw_button(
                            hdc,
                            &shift(btn),
                            "Reload Daemon",
                            false,
                            vis_for(ControlId::BtnReload),
                            pal,
                            fonts,
                            scale_px,
                        );
                    }
                }
            }
        }
    }

    // Overlay scrollbar.
    if let Some((_track, thumb)) = scrollbar_rects(win) {
        let active = win.scrollbar_drag.is_some();
        fill_round(
            hdc,
            &thumb,
            px(4),
            if active { pal.text_dim } else { pal.ctl_stroke },
            if active { pal.text_dim } else { pal.ctl_stroke },
        );
    }
}

unsafe fn draw_banner(hdc: HDC, win: &Win, r: &RECT) {
    let Some(banner) = &win.state.banner else {
        return;
    };
    let px = px_of(win.scale);
    let pal = &win.pal;
    let (bg, fg, glyph) = if banner.success {
        (pal.success_bg, pal.success, GLYPH_COMPLETED)
    } else {
        (pal.critical_bg, pal.critical, GLYPH_ERROR)
    };
    fill_round(hdc, r, px(4), bg, pal.card_stroke);
    let glyph_rect = RECT {
        left: r.left + px(14),
        top: r.top,
        right: r.left + px(40),
        bottom: r.bottom,
    };
    draw_glyph_in(
        hdc,
        win.fonts.glyph,
        fg,
        &glyph_rect,
        glyph,
        DT_SINGLELINE | DT_VCENTER | DT_CENTER,
    );
    let title_w = {
        let screen = GetDC(std::ptr::null_mut());
        let mem = CreateCompatibleDC(screen);
        let w = measure_text(mem, win.fonts.body_strong, &banner.title);
        DeleteDC(mem);
        ReleaseDC(std::ptr::null_mut(), screen);
        w
    };
    let title_rect = RECT {
        left: r.left + px(48),
        top: r.top,
        right: r.left + px(48) + title_w + px(4),
        bottom: r.bottom,
    };
    draw_text_in(
        hdc,
        win.fonts.body_strong,
        pal.text,
        &title_rect,
        &banner.title,
        DT_SINGLELINE | DT_VCENTER | DT_LEFT,
    );
    let msg_rect = RECT {
        left: title_rect.right + px(10),
        top: r.top,
        right: r.right - px(12),
        bottom: r.bottom,
    };
    draw_text_in(
        hdc,
        win.fonts.body,
        pal.text_dim,
        &msg_rect,
        &banner.message,
        DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_END_ELLIPSIS,
    );
}
