//! Icon probing, per-DPI font (re)creation, and rebuilding the spaces bar +
//! window-card grid (including DWM live-thumbnail register/reuse/unregister).

use super::geometry::spaces_bar_metrics;
use super::{MissionControl, SpaceCard, WindowCard, HICON};
use std::collections::HashMap;
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::{HWND, RECT, SIZE};
use windows_sys::Win32::Graphics::Dwm::{
    DwmQueryThumbnailSourceSize, DwmRegisterThumbnail, DwmUnregisterThumbnail,
    DwmUpdateThumbnailProperties, DWM_THUMBNAIL_PROPERTIES, DWM_TNP_OPACITY,
    DWM_TNP_RECTDESTINATION, DWM_TNP_SOURCECLIENTAREAONLY, DWM_TNP_VISIBLE,
};
use windows_sys::Win32::Graphics::Gdi::DeleteObject;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetWindowTextW, GCLP_HICON, GCLP_HICONSM, ICON_BIG, ICON_SMALL, ICON_SMALL2, WM_GETICON,
};
use winspaces_common::MAX_SPACES;
use winspaces_core::spaces::{is_valid_window, SpaceManager};
use winspaces_win32::dpi;
use winspaces_win32::gdi::font::{create_font, FACE_DISPLAY, FACE_ICONS};

/// `SendMessageW(WM_GETICON)` would block the daemon indefinitely on a hung
/// target; use a short abort-if-hung timeout instead.
unsafe fn send_geticon_timeout(hwnd: HWND, icon_kind: u32) -> HICON {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SendMessageTimeoutW, SMTO_ABORTIFHUNG, SMTO_BLOCK,
    };
    let mut result: usize = 0;
    let ok = SendMessageTimeoutW(
        hwnd,
        WM_GETICON,
        icon_kind as _,
        0,
        SMTO_ABORTIFHUNG | SMTO_BLOCK,
        100,
        &mut result,
    );
    if ok != 0 {
        result as HICON
    } else {
        null_mut()
    }
}

unsafe fn get_window_icon(hwnd: HWND) -> HICON {
    let mut hicon = send_geticon_timeout(hwnd, ICON_SMALL2);
    if hicon.is_null() {
        hicon = send_geticon_timeout(hwnd, ICON_SMALL);
    }
    if hicon.is_null() {
        hicon = send_geticon_timeout(hwnd, ICON_BIG);
    }
    if hicon.is_null() {
        #[cfg(target_pointer_width = "64")]
        {
            hicon =
                windows_sys::Win32::UI::WindowsAndMessaging::GetClassLongPtrW(hwnd, GCLP_HICONSM)
                    as HICON;
        }
        #[cfg(not(target_pointer_width = "64"))]
        {
            hicon = windows_sys::Win32::UI::WindowsAndMessaging::GetClassLongW(hwnd, GCLP_HICONSM)
                as HICON;
        }
    }
    if hicon.is_null() {
        #[cfg(target_pointer_width = "64")]
        {
            hicon = windows_sys::Win32::UI::WindowsAndMessaging::GetClassLongPtrW(hwnd, GCLP_HICON)
                as HICON;
        }
        #[cfg(not(target_pointer_width = "64"))]
        {
            hicon = windows_sys::Win32::UI::WindowsAndMessaging::GetClassLongW(hwnd, GCLP_HICON)
                as HICON;
        }
    }
    hicon
}

/// Delete the five overlay fonts and null the handles. Safe to call with the
/// fonts already released. Called from `hide_mission_control`: every show
/// recreates the set via `update_fonts_for_dpi` anyway, so holding them while
/// the overlay is closed was pure GDI retention with no reopen benefit.
pub(crate) unsafe fn release_fonts(mc: &mut MissionControl) {
    if !mc.h_font_title.is_null() {
        DeleteObject(mc.h_font_title);
        DeleteObject(mc.h_font_card);
        DeleteObject(mc.h_font_small);
        DeleteObject(mc.h_font_close);
        DeleteObject(mc.h_font_glyph);
        DeleteObject(mc.h_font_pin);
        mc.h_font_title = std::ptr::null_mut();
        mc.h_font_card = std::ptr::null_mut();
        mc.h_font_small = std::ptr::null_mut();
        mc.h_font_close = std::ptr::null_mut();
        mc.h_font_glyph = std::ptr::null_mut();
        mc.h_font_pin = std::ptr::null_mut();
    }
}

pub(crate) unsafe fn update_fonts_for_dpi(mc: &mut MissionControl, scale: f32) {
    release_fonts(mc);

    mc.scale = scale;

    let px = |pt: i32| dpi::px(scale, pt);

    // Heights stay POSITIVE (cell height) here, unlike every other surface's
    // negated (character height) sign — the shared `create_font` passes the
    // sign through unchanged.
    mc.h_font_title = create_font(FACE_DISPLAY, px(20), 700); // Spaces Card Title
    mc.h_font_card = create_font(FACE_DISPLAY, px(15), 600); // Window Card Title
    mc.h_font_small = create_font(FACE_DISPLAY, px(18), 500); // Card subtitle + tile label
    mc.h_font_close = create_font(FACE_DISPLAY, px(13), 500); // ✕ in the close button
                                                              // Same icon family the tray menu draws with, so the "add" affordance is
                                                              // the same mark in both surfaces.
    mc.h_font_glyph = create_font(FACE_ICONS, px(24), 400);
    mc.h_font_pin = create_font(FACE_ICONS, px(11), 400); // Pin icon in header
}

/// Rebuild the spaces bar and window-card grid (unregistering any existing
/// DWM thumbnails first). Shared by `show_mission_control` and
/// `refresh_mission_control`; assumes the overlay window and fonts exist.
pub(crate) unsafe fn rebuild_cards(
    mc: &mut MissionControl,
    mgr: &SpaceManager,
    mon_idx: usize,
    space_idx: usize,
    width: i32,
    height: i32,
) {
    // Keep existing registrations keyed by source window: a kept thumbnail
    // never leaves DWM composition, so a refresh glides cards to their new
    // rects instead of blinking them out and back in. Whatever is left over
    // after the grid is rebuilt belongs to windows no longer on this space
    // and is unregistered at the end.
    let mut kept_thumbs: HashMap<HWND, isize> = HashMap::new();
    for card in &mc.window_cards {
        if card.h_thumb != 0 {
            kept_thumbs.insert(card.hwnd, card.h_thumb);
        }
    }
    mc.window_cards.clear();
    mc.space_cards.clear();

    let scale = mc.scale;
    let px = |val: i32| dpi::px(scale, val);

    // Build Spaces Bar Layout (Top)
    let spaces_count = mgr.monitors[mon_idx].spaces.len();
    let has_plus = spaces_count < MAX_SPACES;
    let bar = spaces_bar_metrics(spaces_count, has_plus, width, scale);
    let (card_w, card_h, gap, start_x, top_y) =
        (bar.card_w, bar.card_h, bar.gap, bar.start_x, bar.top_y);
    mc.plus_visible = has_plus;
    mc.plus_rect = bar.plus_rect;

    for s_idx in 0..spaces_count {
        let x = start_x + (s_idx as i32 * (card_w + gap));
        let card_rect = RECT {
            left: x,
            top: top_y,
            right: x + card_w,
            bottom: top_y + card_h,
        };
        // Count what the grid would actually show. The raw tracked list can
        // hold handles the Exposé grid filters out below, which showed up as a
        // card reading "10 windows" above two thumbnails.
        let count = mgr
            .windows_for_space(mon_idx, s_idx)
            .into_iter()
            .filter(|&h| is_valid_window(h))
            .count();
        mc.space_cards.push(SpaceCard {
            space_idx: s_idx,
            rect: card_rect,
            window_count: count,
            is_active: s_idx == space_idx,
        });
    }

    // 5. Build Exposé Window Grid Layout & Register DWM Live Thumbnails
    let visible_hwnds = mgr.windows_for_space(mon_idx, space_idx);
    let valid_hwnds: Vec<HWND> = visible_hwnds
        .into_iter()
        .filter(|&h| is_valid_window(h))
        .collect();

    let grid_top = top_y + card_h + px(36);
    let grid_bottom = height - px(40);
    let grid_left = px(60);
    let grid_right = width - px(60);
    let grid_w = grid_right - grid_left;
    let grid_h = grid_bottom - grid_top;

    let num_wins = valid_hwnds.len();
    if num_wins > 0 {
        let (cols, rows) = match num_wins {
            1 => (1, 1),
            2 => (2, 1),
            3 => (3, 1),
            4 => (2, 2),
            5..=6 => (3, 2),
            7..=8 => (4, 2),
            9..=12 => (4, 3),
            13..=16 => (4, 4),
            _ => (5, ((num_wins as i32 + 4) / 5).max(1)),
        };

        let win_slot_w = (grid_w - (cols - 1) * px(24)) / cols;
        let win_slot_h = (grid_h - (rows - 1) * px(24)) / rows;
        let header_h = px(38);
        let thumb_margin = px(8);
        let card_min_w = px(180);
        let max_thumb_w = (win_slot_w - 2 * thumb_margin).max(px(100));
        let max_thumb_h = (win_slot_h - header_h - 2 * thumb_margin).max(px(100));

        for (idx, &target_hwnd) in valid_hwnds.iter().enumerate() {
            let r = (idx as i32) / cols;
            let c = (idx as i32) % cols;

            let items_in_row = if r == rows - 1 {
                num_wins as i32 - r * cols
            } else {
                cols
            };
            let row_offset_x = ((cols - items_in_row) * (win_slot_w + px(24))) / 2;

            let slot_left = grid_left + row_offset_x + c * (win_slot_w + px(24));
            let slot_top = grid_top + r * (win_slot_h + px(24));

            // Reuse the live registration when one exists, else register.
            let mut h_thumb: isize = kept_thumbs.remove(&target_hwnd).unwrap_or(0);
            let reused = h_thumb != 0;
            let hr = if reused {
                0
            } else {
                DwmRegisterThumbnail(mc.hwnd, target_hwnd, &mut h_thumb)
            };

            let (src_w, src_h) = if hr == 0 && h_thumb != 0 {
                let mut src_size: SIZE = std::mem::zeroed();
                let hr_size = DwmQueryThumbnailSourceSize(h_thumb, &mut src_size);
                if hr_size == 0 && src_size.cx > 0 && src_size.cy > 0 {
                    (src_size.cx as f32, src_size.cy as f32)
                } else {
                    let mut wr: RECT = std::mem::zeroed();
                    windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect(
                        target_hwnd,
                        &mut wr,
                    );
                    let w = (wr.right - wr.left).max(1);
                    let h = (wr.bottom - wr.top).max(1);
                    (w as f32, h as f32)
                }
            } else {
                let mut wr: RECT = std::mem::zeroed();
                windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect(target_hwnd, &mut wr);
                let w = (wr.right - wr.left).max(1);
                let h = (wr.bottom - wr.top).max(1);
                (w as f32, h as f32)
            };

            let src_aspect = (src_w / src_h).max(0.1);
            let max_aspect = max_thumb_w as f32 / max_thumb_h as f32;

            let (thumb_w, thumb_h) = if src_aspect > max_aspect {
                let tw = max_thumb_w;
                let th = ((max_thumb_w as f32 / src_aspect).round() as i32).max(px(40));
                (tw, th)
            } else {
                let th = max_thumb_h;
                let tw = ((max_thumb_h as f32 * src_aspect).round() as i32).max(px(40));
                (tw, th)
            };

            let card_w = (thumb_w + 2 * thumb_margin).max(card_min_w);
            let card_h = thumb_h + header_h + thumb_margin;

            let card_left = slot_left + (win_slot_w - card_w) / 2;
            let card_top = slot_top + (win_slot_h - card_h) / 2;
            let card_rect = RECT {
                left: card_left,
                top: card_top,
                right: card_left + card_w,
                bottom: card_top + card_h,
            };

            let thumb_left = card_left + (card_w - thumb_w) / 2;
            let thumb_top = card_top + header_h;
            let thumb_rect = RECT {
                left: thumb_left,
                top: thumb_top,
                right: thumb_left + thumb_w,
                bottom: thumb_top + thumb_h,
            };

            if hr == 0 && h_thumb != 0 {
                let mut props: DWM_THUMBNAIL_PROPERTIES = std::mem::zeroed();
                props.dwFlags = DWM_TNP_RECTDESTINATION
                    | DWM_TNP_VISIBLE
                    | DWM_TNP_OPACITY
                    | DWM_TNP_SOURCECLIENTAREAONLY;
                props.rcDestination = thumb_rect;
                props.fVisible = 1;
                props.opacity = 255;
                props.fSourceClientAreaOnly = 0;
                if DwmUpdateThumbnailProperties(h_thumb, &props) != 0 && reused {
                    // The kept handle went stale; demote to a fresh
                    // registration.
                    DwmUnregisterThumbnail(h_thumb);
                    h_thumb = 0;
                    if DwmRegisterThumbnail(mc.hwnd, target_hwnd, &mut h_thumb) == 0 && h_thumb != 0
                    {
                        DwmUpdateThumbnailProperties(h_thumb, &props);
                    }
                }
            }

            let mut title_buf = [0u16; 256];
            let len = GetWindowTextW(target_hwnd, title_buf.as_mut_ptr(), 256);
            let title = if len > 0 {
                String::from_utf16_lossy(&title_buf[..len as usize])
            } else {
                "Application Window".to_string()
            };

            let h_icon = get_window_icon(target_hwnd);

            mc.window_cards.push(WindowCard {
                hwnd: target_hwnd,
                h_thumb,
                h_icon,
                card_rect,
                thumb_rect,
                title,
                is_sticky: mgr.is_sticky(target_hwnd),
            });
        }
    }

    // Windows no longer on this space keep no registration behind.
    for (_, h_thumb) in kept_thumbs {
        DwmUnregisterThumbnail(h_thumb);
    }
}
