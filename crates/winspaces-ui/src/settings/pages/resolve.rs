//! Resolving a page's item specs into laid-out rectangles: card heights
//! from wrapped descriptions, trailing-control placement, and the scroll
//! extent. Depends on `RECT` and the caller's text measurer only.

use super::*;

/// Minimum card height. A description that wraps (up to `DESC_MAX_LINES`)
/// grows the card; nothing else does.
pub(super) const CARD_H: i32 = 68;

pub(super) const CARD_GAP: i32 = 8;

pub(super) const CTL_H: i32 = 32;

pub(super) const FIELD_MIN_W: i32 = 140;

/// Longer descriptions still ellipsize — a card is a summary, not a manual.
pub(super) const DESC_MAX_LINES: i32 = 2;

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
