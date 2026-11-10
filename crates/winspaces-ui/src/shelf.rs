//! The Shelf strip: one window per display along the edge Shelf reserves,
//! holding a card per hidden set of windows. A card shows a live DWM
//! thumbnail of its window over the app's name and the window's title, or
//! the window's icon when it was hidden by a backend that leaves no surface
//! to thumbnail (forced-minimize, `SW_HIDE`). A set's card lays its windows'
//! thumbnails out as they sat on screen and names each window on its own
//! line, after its app's icon. Clicking a card
//! swaps it onto the stage; dragging one along the strip reorders it, and
//! dragging it out onto the stage (or Ctrl+clicking it) adds it to the
//! windows already there.
//! The daemon owns the roles (`winspaces-core`'s `spaces::shelf`); this module
//! only paints what `sync` hands it and reports gestures through `ShelfHost`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ptr::null_mut;
use std::sync::OnceLock;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{
    DwmQueryThumbnailSourceSize, DwmRegisterThumbnail, DwmUnregisterThumbnail,
    DwmUpdateThumbnailProperties, DWM_THUMBNAIL_PROPERTIES, DWM_TNP_OPACITY,
    DWM_TNP_RECTDESTINATION, DWM_TNP_SOURCECLIENTAREAONLY, DWM_TNP_VISIBLE,
};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, ClientToScreen, CombineRgn, CreateRectRgn, DeleteObject, EndPaint, InvalidateRect,
    SetWindowRgn, DT_END_ELLIPSIS, DT_LEFT, DT_SINGLELINE, DT_VCENTER, HDC, HFONT, PAINTSTRUCT,
    RGN_OR,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, ReleaseCapture, SetCapture, VK_CONTROL, VK_ESCAPE,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DrawIconEx, GetClientRect, GetSystemMetrics,
    GetWindowRect, IsIconic, IsWindowVisible, KillTimer, LoadIconW, SetTimer, SetWindowPos,
    ShowWindow, DI_NORMAL, HICON, HWND_TOPMOST, IDI_APPLICATION, MA_NOACTIVATE, SM_CXDRAG,
    SM_CYDRAG, SWP_NOACTIVATE, SWP_NOZORDER, SW_HIDE, SW_SHOWNOACTIVATE, WA_INACTIVE, WM_ACTIVATE,
    WM_CAPTURECHANGED, WM_COMMAND, WM_ERASEBKGND, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE,
    WM_MOUSEMOVE, WM_PAINT, WM_RBUTTONUP, WM_TIMER, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP,
};
use winspaces_common::i18n::t;
use winspaces_common::{Msg, WindowRect};
use winspaces_core::spaces::is_live_window;
use winspaces_core::workspaces::{get_app_name, get_window_title};
use winspaces_win32::dpi;
use winspaces_win32::dwm;
use winspaces_win32::gdi::color::{rgb, Tint};
use winspaces_win32::gdi::draw::{draw_text_in, fill_round, measure_text};
use winspaces_win32::gdi::font::{create_font, FACE_TEXT};
use winspaces_win32::gdi::surface::paint_surface;
use winspaces_win32::module::app_instance;
use winspaces_win32::text::encode_wide;

use crate::window_icon::get_window_icon;

const CLASS_NAME: &str = "WinSpacesShelfStrip";

/// What the daemon wants one display's strip to show. `rect` is in screen
/// coordinates; a strip taller than wide stacks its cards vertically.
pub struct StripModel {
    pub rect: WindowRect,
    /// The cards, most recent first: each a set of windows, topmost first,
    /// with whether its thumbnail is live.
    pub cards: Vec<Vec<(HWND, bool)>>,
}

/// Gestures the strip reports back. Called after the strip's state borrow is
/// released: the host re-enters `sync`. A card is named by its top window.
pub struct ShelfHost {
    pub promote: fn(hwnd: HWND),
    /// The card joins the stage: dropped at `at` (screen coordinates), or
    /// Ctrl+clicked (`None`).
    pub join: fn(hwnd: HWND, at: Option<POINT>),
    /// While a card is dragged: whether a drop at `at` joins the stage (the
    /// host shows where it would land). `None` ends the drag.
    pub join_preview: fn(hwnd: HWND, at: Option<POINT>) -> bool,
    pub reorder: fn(mon: usize, order: Vec<HWND>),
    /// Right click on a card at `at` (screen coordinates). `owner` is the
    /// strip, which owns the menu so it always stacks above the cards.
    pub context_menu: fn(owner: HWND, card: HWND, at: POINT),
    /// A command the menu posted back to its owner.
    pub command: fn(id: usize),
}

static HOST: OnceLock<&'static ShelfHost> = OnceLock::new();

pub fn install_host(host: &'static ShelfHost) {
    let _ = HOST.set(host);
}

/// The glyph for a strip edge in the strip's menu: an arrow pointing at it.
pub fn side_glyph(side: winspaces_common::ShelfSide) -> u16 {
    use winspaces_common::ShelfSide;
    use winspaces_win32::glyphs::{
        GLYPH_ARROW_DOWN, GLYPH_ARROW_LEFT, GLYPH_ARROW_RIGHT, GLYPH_ARROW_UP,
    };
    match side {
        ShelfSide::Left => GLYPH_ARROW_LEFT,
        ShelfSide::Right => GLYPH_ARROW_RIGHT,
        ShelfSide::Top => GLYPH_ARROW_UP,
        ShelfSide::Bottom => GLYPH_ARROW_DOWN,
    }
}

/// The localized name of a strip edge, for the settings combo and the strip's menu.
pub fn side_label(side: winspaces_common::ShelfSide) -> &'static str {
    use winspaces_common::ShelfSide;
    t(match side {
        ShelfSide::Left => Msg::SettingsShelfSideLeft,
        ShelfSide::Right => Msg::SettingsShelfSideRight,
        ShelfSide::Top => Msg::SettingsShelfSideTop,
        ShelfSide::Bottom => Msg::SettingsShelfSideBottom,
    })
}

/// One thumbnail of a card.
struct Layer {
    hwnd: HWND,
    thumb: isize,
    border: Option<u32>,
    /// The thumbnail's source size, zero when unknown.
    src: SIZE,
    /// The window's rect on screen, empty while minimized.
    frame: RECT,
}

/// Thumbnails a card shows at most; the label still names the rest.
const MAX_LAYERS: usize = 3;

struct Card {
    /// The top window, which names the card to the host.
    hwnd: HWND,
    rect: RECT,
    /// Topmost first.
    layers: Vec<Layer>,
    /// The label lines, see `card_lines`.
    lines: Vec<String>,
    /// The windows' icons, top first, as many as there are layers.
    icons: Vec<HICON>,
}

impl Card {
    fn unregister(&mut self) {
        for layer in &mut self.layers {
            if layer.thumb != 0 {
                unsafe {
                    DwmUnregisterThumbnail(layer.thumb);
                }
                layer.thumb = 0;
            }
        }
    }
}

struct Press {
    card: usize,
    start: POINT,
    dragging: bool,
    /// Offset of the dragged card along the strip axis.
    offset: i32,
    /// Dragged out over the stage: the release joins instead of reordering.
    joining: bool,
}

/// Polls `Esc` while a card is lifted: the strip never has the focus.
const ESC_TIMER: usize = 1;

struct Strip {
    hwnd: HWND,
    vertical: bool,
    scale: f32,
    /// Height of the label under every card's thumbnail: the most lines any
    /// card needs, so the cards stay the same size.
    label: i32,
    font: HFONT,
    bold: HFONT,
    cards: Vec<Card>,
    colored_borders: bool,
    press: Option<Press>,
}

impl Strip {
    fn release_cards(&mut self) {
        for mut card in self.cards.drain(..) {
            card.unregister();
        }
    }

    fn hide(&mut self) {
        self.release_cards();
        self.press = None;
        unsafe {
            ShowWindow(self.hwnd, SW_HIDE);
        }
    }
}

thread_local! {
    static WINDOW_COLORS: RefCell<HashMap<HWND, u32>> = RefCell::new(HashMap::new());
    static STRIPS: RefCell<Vec<Strip>> = const { RefCell::new(Vec::new()) };
}

/// Show one strip per display with a model and hide the others. Index `i` of
/// `models` is display `i`. `above` is a window the strips must stay under
/// (an open Overview), or null.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub fn sync(
    models: &[Option<StripModel>],
    above: HWND,
    colored_borders: bool,
    stage: impl Iterator<Item = HWND>,
) {
    STRIPS.with(|s| {
        let Ok(mut strips) = s.try_borrow_mut() else {
            return;
        };
        while strips.len() > models.len() {
            if let Some(mut strip) = strips.pop() {
                strip.release_cards();
                unsafe {
                    DeleteObject(strip.font as _);
                    DeleteObject(strip.bold as _);
                    DestroyWindow(strip.hwnd);
                }
            }
        }
        for (i, model) in models.iter().enumerate() {
            let model = model.as_ref().filter(|m| !m.cards.is_empty());
            if strips.len() <= i {
                // Created on first use: a daemon that never shows a strip
                // never pays for its window.
                if model.is_none() {
                    continue;
                }
                let Some(strip) = (unsafe { create_strip() }) else {
                    return;
                };
                strips.push(strip);
            }
            let strip = &mut strips[i];
            match model {
                Some(model) => unsafe { show(strip, model, above, colored_borders) },
                None => strip.hide(),
            }
        }
        if colored_borders {
            WINDOW_COLORS.with(|colors| {
                let mut colors = colors.borrow_mut();
                colors.retain(|&hwnd, _| is_live_window(hwnd));
                let windows = strips
                    .iter()
                    .flat_map(|strip| &strip.cards)
                    .flat_map(|card| &card.layers)
                    .map(|layer| layer.hwnd)
                    .chain(stage)
                    .collect();
                assign_border_colors(&mut colors, windows);
                for layer in strips
                    .iter_mut()
                    .flat_map(|strip| &mut strip.cards)
                    .flat_map(|card| &mut card.layers)
                {
                    layer.border = colors.get(&layer.hwnd).copied();
                }
            });
        }
    });
}

const BORDER_COLORS: [u32; 7] = [
    rgb(0x43, 0x85, 0xFF),
    rgb(0xFF, 0xD5, 0x34),
    rgb(0x31, 0xCC, 0x65),
    rgb(0xFF, 0x50, 0x60),
    rgb(0xEC, 0x7C, 0xF1),
    rgb(0xFF, 0xFF, 0xFF),
    rgb(0xB0, 0x68, 0x28),
];

fn assign_border_colors(colors: &mut HashMap<HWND, u32>, mut windows: Vec<HWND>) {
    windows.sort_unstable();
    windows.dedup();

    let mut free = BORDER_COLORS.map(Some);
    for &hwnd in &windows {
        if let Some(&color) = colors.get(&hwnd) {
            if let Some(slot) = free.iter_mut().find(|slot| **slot == Some(color)) {
                *slot = None;
            } else {
                colors.remove(&hwnd);
            }
        }
    }

    // shortcut: use neutral outlines after the palette runs out, add another visual cue if needed.
    let mut available = free.into_iter().flatten();
    for hwnd in windows {
        if let std::collections::hash_map::Entry::Vacant(entry) = colors.entry(hwnd) {
            let Some(color) = available.next() else {
                break;
            };
            entry.insert(color);
        }
    }
}

unsafe fn create_strip() -> Option<Strip> {
    winspaces_win32::window_class::register_class(CLASS_NAME, Some(strip_wnd_proc));
    let class = encode_wide(CLASS_NAME);
    let hwnd = CreateWindowExW(
        WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
        class.as_ptr(),
        std::ptr::null(),
        WS_POPUP,
        0,
        0,
        0,
        0,
        null_mut(),
        null_mut(),
        app_instance(),
        null_mut(),
    );
    if hwnd.is_null() {
        return None;
    }
    dwm::set_dark_mode(hwnd, true);
    // No backdrop: the client area is glass, so the translucent fill shows.
    dwm::extend_frame_full(hwnd);
    dwm::set_corner_rounding(hwnd, false);
    Some(Strip {
        hwnd,
        vertical: true,
        scale: 0.0,
        label: 0,
        font: null_mut(),
        bold: null_mut(),
        cards: Vec::new(),
        colored_borders: false,
        press: None,
    })
}

unsafe fn show(strip: &mut Strip, model: &StripModel, above: HWND, colored_borders: bool) {
    let r = &model.rect;
    let scale = dpi::scale_for_point(POINT {
        x: (r.left + r.right) / 2,
        y: (r.top + r.bottom) / 2,
    });
    let (x, y, w, h) = (r.left, r.top, r.width(), r.height());
    // Beneath an open menu or Overview; otherwise a strip already on screen
    // keeps its rank, so a refresh never jumps over a menu opened since.
    let under = [crate::menu::root_hwnd(), above]
        .into_iter()
        .find(|h| !h.is_null());
    let mut flags = SWP_NOACTIVATE;
    if under.is_none() && IsWindowVisible(strip.hwnd) != 0 {
        flags |= SWP_NOZORDER;
    }
    let insert_after = under.unwrap_or(HWND_TOPMOST);
    SetWindowPos(strip.hwnd, insert_after, x, y, w, h, flags);
    ShowWindow(strip.hwnd, SW_SHOWNOACTIVATE);

    if scale != strip.scale || strip.font.is_null() {
        if !strip.font.is_null() {
            DeleteObject(strip.font as _);
            DeleteObject(strip.bold as _);
        }
        strip.font = create_font(FACE_TEXT, dpi::px(scale, 13), 400);
        strip.bold = create_font(FACE_TEXT, dpi::px(scale, 14), 600);
        strip.scale = scale;
    }
    strip.vertical = h > w;
    strip.colored_borders = colored_borders;
    strip.press = None;

    let lines: Vec<Vec<String>> = model
        .cards
        .iter()
        .map(|set| {
            card_lines(
                &set.iter()
                    .map(|&(h, _)| (get_app_name(h), window_title(h)))
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    strip.label = label_height(scale, lines.iter().map(Vec::len).max().unwrap_or(0).max(2));
    let rects = card_rects(w, h, strip.vertical, model.cards.len(), scale, strip.label);
    let mut kept = std::mem::take(&mut strip.cards);
    for ((set, rect), lines) in model.cards.iter().zip(rects).zip(lines) {
        let Some(&(hwnd, _)) = set.first() else {
            continue;
        };
        let shown = &set[..set.len().min(MAX_LAYERS)];
        let old = kept
            .iter()
            .position(|c| c.hwnd == hwnd)
            .map(|i| kept.remove(i));
        // Thumbnails stack in registration order, so a card reuses them only
        // when its layers are exactly the same; otherwise it registers anew.
        let reuse = old.as_ref().is_some_and(|c| {
            c.layers.len() == shown.len()
                && c.layers
                    .iter()
                    .zip(shown)
                    .all(|(l, &(h, live))| l.hwnd == h && (l.thumb != 0) == live)
        });
        let (mut layers, icons) = match old {
            Some(c) if reuse => (c.layers, c.icons),
            old => {
                if let Some(mut c) = old {
                    c.unregister();
                }
                let mut layers: Vec<Layer> = shown
                    .iter()
                    .map(|&(h, live)| Layer {
                        hwnd: h,
                        thumb: live as isize,
                        border: None,
                        src: SIZE { cx: 0, cy: 0 },
                        frame: EMPTY,
                    })
                    .collect();
                register_layers(strip.hwnd, &mut layers);
                (layers, shown.iter().map(|&(h, _)| app_icon(h)).collect())
            }
        };
        for layer in &mut layers {
            layer.frame = window_frame(layer.hwnd);
        }
        let card = Card {
            hwnd,
            rect,
            layers,
            lines,
            icons,
        };
        place_layers(&card, None, strip);
        strip.cards.push(card);
    }
    for mut card in kept {
        card.unregister();
    }
    clip_to_cards(strip);
    InvalidateRect(strip.hwnd, std::ptr::null(), 1);
}

/// Register the thumbnails of the layers marked live (non-zero `thumb`),
/// back to front so the top one stacks above the rest.
unsafe fn register_layers(host: HWND, layers: &mut [Layer]) {
    for layer in layers.iter_mut().rev() {
        if layer.thumb == 0 {
            continue;
        }
        if DwmRegisterThumbnail(host, layer.hwnd, &mut layer.thumb) != 0 {
            layer.thumb = 0;
        }
        layer.src = SIZE { cx: 0, cy: 0 };
        if layer.thumb != 0 && DwmQueryThumbnailSourceSize(layer.thumb, &mut layer.src) != 0 {
            layer.src = SIZE { cx: 0, cy: 0 };
        }
    }
}

unsafe fn window_title(hwnd: HWND) -> String {
    let title = get_window_title(hwnd);
    if title.is_empty() {
        t(Msg::OverviewUntitled).to_string()
    } else {
        title
    }
}

/// The label lines of a card from its windows' (app, title), top first: the
/// app and the title for one window; for a set, one line per window naming
/// its app, or its title when the app has no name (a UWP frame host) or
/// repeats in the set. A set past `MAX_LAYERS` names the rest on the last
/// line.
fn card_lines(windows: &[(String, String)]) -> Vec<String> {
    match windows {
        [(app, title)] if app.is_empty() || app == title => vec![title.clone(), String::new()],
        [(app, title)] => vec![app.clone(), title.clone()],
        _ => {
            let mut names: Vec<String> = windows
                .iter()
                .map(|(a, t)| {
                    let repeats = windows.iter().filter(|(b, _)| b == a).count() > 1;
                    if a.is_empty() || repeats { t } else { a }.clone()
                })
                .collect();
            if names.len() > MAX_LAYERS {
                let rest = names.split_off(MAX_LAYERS - 1).join(", ");
                names.push(rest);
            }
            names
        }
    }
}

/// The window's icon, or the generic app icon when it has none (a UWP frame
/// host answers no `WM_GETICON`), so every label line is led by one.
unsafe fn app_icon(hwnd: HWND) -> HICON {
    let icon = get_window_icon(hwnd);
    if icon.is_null() {
        LoadIconW(null_mut(), IDI_APPLICATION)
    } else {
        icon
    }
}

const EMPTY: RECT = RECT {
    left: 0,
    top: 0,
    right: 0,
    bottom: 0,
};

unsafe fn window_frame(hwnd: HWND) -> RECT {
    let mut r = EMPTY;
    if IsIconic(hwnd) == 0 {
        GetWindowRect(hwnd, &mut r);
    }
    r
}

/// `frames` scaled together and centred into `bounds`, keeping where each
/// sat relative to the others. `None` when a frame is unknown.
fn composite(frames: &[RECT], bounds: &RECT) -> Option<Vec<RECT>> {
    if frames.is_empty()
        || frames
            .iter()
            .any(|f| f.right <= f.left || f.bottom <= f.top)
    {
        return None;
    }
    let u = frames.iter().fold(frames[0], |u, f| RECT {
        left: u.left.min(f.left),
        top: u.top.min(f.top),
        right: u.right.max(f.right),
        bottom: u.bottom.max(f.bottom),
    });
    let (bw, bh) = (bounds.right - bounds.left, bounds.bottom - bounds.top);
    let (uw, uh) = (u.right - u.left, u.bottom - u.top);
    let k = (bw as f32 / uw as f32).min(bh as f32 / uh as f32);
    let ox = bounds.left + (bw - (uw as f32 * k) as i32) / 2;
    let oy = bounds.top + (bh - (uh as f32 * k) as i32) / 2;
    let x = |v: i32| ox + ((v - u.left) as f32 * k) as i32;
    let y = |v: i32| oy + ((v - u.top) as f32 * k) as i32;
    Some(
        frames
            .iter()
            .map(|f| RECT {
                left: x(f.left),
                top: y(f.top),
                right: x(f.right),
                bottom: y(f.bottom),
            })
            .collect(),
    )
}

/// Height of a label of `lines` lines under a card's thumbnail.
fn label_height(scale: f32, lines: usize) -> i32 {
    dpi::px(scale, 4 + lines as i32 * LINE)
}

/// One label line, logical pixels.
const LINE: i32 = 24;

/// Where each of a card's thumbnails goes inside the card drawn at `r`, top
/// first, above the labels: a set of live thumbnails as its windows sat on
/// screen; otherwise each layer behind peeking out up and to the right,
/// fitted to its window's aspect when the thumbnail reports a size.
fn layer_rects(card: &Card, r: &RECT, scale: f32, label: i32) -> Vec<RECT> {
    let pad = dpi::px(scale, 4);
    if card.layers.len() > 1 && card.layers.iter().all(|l| l.thumb != 0) {
        let bounds = RECT {
            left: r.left + pad,
            top: r.top + pad,
            right: r.right - pad,
            bottom: r.bottom - label - pad,
        };
        let frames: Vec<RECT> = card.layers.iter().map(|l| l.frame).collect();
        if let Some(rects) = composite(&frames, &bounds) {
            return rects;
        }
    }
    let step = dpi::px(scale, 6);
    let depth = card.layers.len().max(1) as i32 - 1;
    let (bw, bh) = (
        r.right - r.left - 2 * pad - depth * step,
        r.bottom - r.top - label - 2 * pad - depth * step,
    );
    card.layers
        .iter()
        .enumerate()
        .map(|(i, layer)| {
            let i = i as i32;
            let (bx, by) = (r.left + pad + i * step, r.top + pad + (depth - i) * step);
            let src = layer.src;
            let (tw, th) = if src.cx > 0 && src.cy > 0 {
                let k = (bw as f32 / src.cx as f32).min(bh as f32 / src.cy as f32);
                ((src.cx as f32 * k) as i32, (src.cy as f32 * k) as i32)
            } else {
                (bw, bh)
            };
            let left = bx + (bw - tw) / 2;
            let top = by + (bh - th) / 2;
            RECT {
                left,
                top,
                right: left + tw,
                bottom: top + th,
            }
        })
        .collect()
}

fn preview_rect(r: RECT, strip: &Strip) -> RECT {
    if !strip.colored_borders {
        return r;
    }
    let inset =
        dpi::px(strip.scale, 2).min(((r.right - r.left).min(r.bottom - r.top) - 1).max(0) / 2);
    RECT {
        left: r.left + inset,
        top: r.top + inset,
        right: r.right - inset,
        bottom: r.bottom - inset,
    }
}

/// Fit the card's thumbnails inside where the card is drawn (see `drawn_rect`).
unsafe fn place_layers(card: &Card, drag: Option<i32>, strip: &Strip) {
    let r = drawn_rect(&card.rect, drag, strip.vertical);
    for (layer, dest) in card
        .layers
        .iter()
        .zip(layer_rects(card, &r, strip.scale, strip.label))
    {
        if layer.thumb == 0 {
            continue;
        }
        let mut props: DWM_THUMBNAIL_PROPERTIES = std::mem::zeroed();
        props.dwFlags = DWM_TNP_RECTDESTINATION
            | DWM_TNP_VISIBLE
            | DWM_TNP_OPACITY
            | DWM_TNP_SOURCECLIENTAREAONLY;
        props.rcDestination = preview_rect(dest, strip);
        props.fVisible = 1;
        props.opacity = 255;
        props.fSourceClientAreaOnly = 0;
        DwmUpdateThumbnailProperties(layer.thumb, &props);
    }
}

fn centre(r: &RECT, vertical: bool) -> i32 {
    if vertical {
        (r.top + r.bottom) / 2
    } else {
        (r.left + r.right) / 2
    }
}

/// Shape the strip to its cards, so clicks between them reach the desktop.
unsafe fn clip_to_cards(strip: &Strip) {
    let rgn = CreateRectRgn(0, 0, 0, 0);
    for card in &strip.cards {
        let r = &card.rect;
        let part = CreateRectRgn(r.left, r.top, r.right, r.bottom);
        CombineRgn(rgn, rgn, part, RGN_OR);
        DeleteObject(part as _);
    }
    // The window owns the region from here on.
    SetWindowRgn(strip.hwnd, rgn, 0);
}

/// DWM stacks a host's thumbnails in registration order, so registering the
/// dragged card's again lifts it above its neighbours.
unsafe fn raise_thumbnails(host: HWND, card: &mut Card) {
    for layer in &mut card.layers {
        if layer.thumb != 0 {
            DwmUnregisterThumbnail(layer.thumb);
            layer.thumb = 1;
        }
    }
    register_layers(host, &mut card.layers);
}

/// A card's slot at rest; while dragged, shifted `offset` along the strip
/// and grown a little so it reads as picked up.
fn drawn_rect(r: &RECT, drag: Option<i32>, vertical: bool) -> RECT {
    let Some(offset) = drag else {
        return *r;
    };
    let s = shifted(r, offset, vertical);
    let grow = (s.right - s.left).min(s.bottom - s.top) / 14;
    RECT {
        left: s.left - grow,
        top: s.top - grow,
        right: s.right + grow,
        bottom: s.bottom + grow,
    }
}

fn shifted(r: &RECT, offset: i32, vertical: bool) -> RECT {
    if vertical {
        RECT {
            top: r.top + offset,
            bottom: r.bottom + offset,
            ..*r
        }
    } else {
        RECT {
            left: r.left + offset,
            right: r.right + offset,
            ..*r
        }
    }
}

/// Card rects along a `w` x `h` strip: a thumbnail as large as the strip's
/// thickness allows plus its labels, shrunk to fit them all, centred along
/// the axis.
// ponytail: cards that still do not fit at the minimum size are dropped from
// the strip (Overview and the taskbar still reach them); scroll if it matters.
fn card_rects(w: i32, h: i32, vertical: bool, n: usize, scale: f32, label: i32) -> Vec<RECT> {
    let px = |v| dpi::px(scale, v);
    let (pad, gap) = (px(10), px(22));
    let (along, across) = if vertical { (h, w) } else { (w, h) };
    let thick = across - 2 * pad;
    let room = along - 2 * pad;
    if n == 0 || thick <= label || room <= 0 {
        return Vec::new();
    }
    let (mut len, min) = if vertical {
        (thick * 3 / 4 + label, px(48) + label)
    } else {
        ((thick - label) * 4 / 3, px(48))
    };
    let mut count = n as i32;
    if count * len + (count - 1) * gap > room {
        len = ((room - (count - 1) * gap) / count).max(min);
        count = ((room + gap) / (len + gap)).min(count);
    }
    let total = count * len + (count - 1).max(0) * gap;
    let start = pad + (room - total) / 2;
    (0..count)
        .map(|i| {
            let a = start + i * (len + gap);
            if vertical {
                RECT {
                    left: pad,
                    top: a,
                    right: pad + thick,
                    bottom: a + len,
                }
            } else {
                RECT {
                    left: a,
                    top: pad,
                    right: a + len,
                    bottom: pad + thick,
                }
            }
        })
        .collect()
}

/// Where card `from` lands when dropped with its centre at `pos` along the
/// axis: past every other card whose centre it crossed.
fn drop_slot(from: usize, pos: i32, rects: &[RECT], vertical: bool) -> usize {
    rects
        .iter()
        .enumerate()
        .filter(|&(i, r)| i != from && centre(r, vertical) < pos)
        .count()
}

fn axis(pt: POINT, vertical: bool) -> i32 {
    if vertical {
        pt.y
    } else {
        pt.x
    }
}

fn point(lparam: LPARAM) -> POINT {
    POINT {
        x: (lparam & 0xFFFF) as i16 as i32,
        y: ((lparam >> 16) & 0xFFFF) as i16 as i32,
    }
}

fn contains(r: &RECT, pt: POINT) -> bool {
    pt.x >= r.left && pt.x < r.right && pt.y >= r.top && pt.y < r.bottom
}

/// What a button release asks the host to do, decided inside the borrow.
enum Release {
    Promote(HWND),
    Join(HWND, Option<POINT>),
    Reorder(usize, Vec<HWND>),
}

fn key_down(vk: u16) -> bool {
    unsafe { GetAsyncKeyState(vk as i32) as u16 & 0x8000 != 0 }
}

unsafe extern "system" fn strip_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let hdc = BeginPaint(hwnd, &mut ps);
            let mut rc: RECT = std::mem::zeroed();
            GetClientRect(hwnd, &mut rc);
            // Alpha 1 rather than 0: invisible, but the surface's alpha fixup
            // can still tell it from the pixels GDI painted.
            let tint = Tint {
                r: 0,
                g: 0,
                b: 0,
                alpha: 1,
            };
            paint_surface(hdc, rc.right - rc.left, rc.bottom - rc.top, &tint, |dc| {
                paint(dc, hwnd)
            });
            EndPaint(hwnd, &ps);
            0
        }
        WM_LBUTTONDOWN => {
            let pt = point(lparam);
            let pressed = with_strip(hwnd, |strip, _| {
                let card = strip.cards.iter().position(|c| contains(&c.rect, pt))?;
                strip.press = Some(Press {
                    card,
                    start: pt,
                    dragging: false,
                    offset: 0,
                    joining: false,
                });
                Some(())
            })
            .flatten();
            if pressed.is_some() {
                SetCapture(hwnd);
            }
            0
        }
        WM_MOUSEMOVE => {
            let pt = point(lparam);
            let lifted = with_strip(hwnd, |strip, _| {
                let vertical = strip.vertical;
                let press = strip.press.as_mut()?;
                let delta = axis(pt, vertical) - axis(press.start, vertical);
                let across = axis(pt, !vertical) - axis(press.start, !vertical);
                let threshold =
                    GetSystemMetrics(if vertical { SM_CYDRAG } else { SM_CXDRAG }).max(4);
                if !press.dragging && delta.abs().max(across.abs()) <= threshold {
                    return None;
                }
                let card = press.card;
                if !press.dragging {
                    press.dragging = true;
                    raise_thumbnails(hwnd, &mut strip.cards[card]);
                    // The whole strip while lifted, so the card can roam it.
                    SetWindowRgn(hwnd, null_mut(), 0);
                    SetTimer(hwnd, ESC_TIMER, 50, None);
                }
                press.offset = delta;
                place_layers(&strip.cards[card], Some(delta), strip);
                InvalidateRect(hwnd, std::ptr::null(), 0);
                Some(strip.cards[card].hwnd)
            })
            .flatten();
            // Asked outside the borrow: the host may re-enter the strip.
            if let (Some(card), Some(host)) = (lifted, HOST.get()) {
                let mut at = pt;
                ClientToScreen(hwnd, &mut at);
                let joining = (host.join_preview)(card, Some(at));
                with_strip(hwnd, |strip, _| {
                    if let Some(press) = strip.press.as_mut() {
                        press.joining = joining;
                    }
                });
            }
            0
        }
        WM_TIMER if wparam == ESC_TIMER => {
            if key_down(VK_ESCAPE) {
                // The capture change puts the card back and ends the preview.
                ReleaseCapture();
            }
            0
        }
        WM_LBUTTONUP => {
            let mut at = point(lparam);
            ClientToScreen(hwnd, &mut at);
            let ctrl = key_down(VK_CONTROL);
            let release = with_strip(hwnd, |strip, mon| {
                let press = strip.press.take()?;
                KillTimer(hwnd, ESC_TIMER);
                let card = &strip.cards[press.card];
                if !press.dragging {
                    return Some(if ctrl {
                        Release::Join(card.hwnd, None)
                    } else {
                        Release::Promote(card.hwnd)
                    });
                }
                if press.joining {
                    return Some(Release::Join(card.hwnd, Some(at)));
                }
                let pos = centre(
                    &shifted(&card.rect, press.offset, strip.vertical),
                    strip.vertical,
                );
                let rects: Vec<RECT> = strip.cards.iter().map(|c| c.rect).collect();
                let slot = drop_slot(press.card, pos, &rects, strip.vertical);
                let mut order: Vec<HWND> = strip.cards.iter().map(|c| c.hwnd).collect();
                let moved = order.remove(press.card);
                order.insert(slot, moved);
                Some(Release::Reorder(mon, order))
            })
            .flatten();
            ReleaseCapture();
            match (release, HOST.get()) {
                (Some(Release::Promote(h)), Some(host)) => (host.promote)(h),
                (Some(Release::Join(h, at)), Some(host)) => (host.join)(h, at),
                (Some(Release::Reorder(mon, order)), Some(host)) => (host.reorder)(mon, order),
                _ => {}
            }
            0
        }
        WM_RBUTTONUP => {
            let pt = point(lparam);
            let card = with_strip(hwnd, |strip, _| {
                strip
                    .cards
                    .iter()
                    .find(|c| contains(&c.rect, pt))
                    .map(|c| c.hwnd)
            })
            .flatten();
            if let (Some(card), Some(host)) = (card, HOST.get()) {
                let mut at = pt;
                ClientToScreen(hwnd, &mut at);
                (host.context_menu)(hwnd, card, at);
            }
            0
        }
        WM_COMMAND => {
            if let Some(host) = HOST.get() {
                (host.command)(wparam & 0xFFFF);
            }
            0
        }
        // The menu activates its owner; losing that is a light dismiss.
        WM_ACTIVATE => {
            if (wparam & 0xFFFF) as u32 == WA_INACTIVE {
                crate::menu::handle_owner_deactivate();
            }
            0
        }
        WM_CAPTURECHANGED => {
            // Arrives re-entrantly from `ReleaseCapture` or a `sync` that hides
            // the strip mid-press; a press still pending here was interrupted.
            let dropped = with_strip(hwnd, |strip, _| {
                let press = strip.press.take()?;
                KillTimer(hwnd, ESC_TIMER);
                let card = &strip.cards[press.card];
                place_layers(card, None, strip);
                clip_to_cards(strip);
                InvalidateRect(hwnd, std::ptr::null(), 0);
                press.dragging.then_some(card.hwnd)
            })
            .flatten();
            if let (Some(card), Some(host)) = (dropped, HOST.get()) {
                (host.join_preview)(card, None);
            }
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// Run `f` on the strip owning `hwnd`, with its display index. `None` when the
/// state is borrowed (a re-entrant message) or the window is unknown.
fn with_strip<R>(hwnd: HWND, f: impl FnOnce(&mut Strip, usize) -> R) -> Option<R> {
    STRIPS.with(|s| {
        let mut strips = s.try_borrow_mut().ok()?;
        let mon = strips.iter().position(|st| st.hwnd == hwnd)?;
        Some(f(&mut strips[mon], mon))
    })
}

unsafe fn paint(hdc: windows_sys::Win32::Graphics::Gdi::HDC, hwnd: HWND) {
    with_strip(hwnd, |strip, _| {
        let px = |v| dpi::px(strip.scale, v);
        let dragged = strip.press.as_ref().filter(|p| p.dragging);
        // The dragged card last, so it paints over the cards it passes.
        let order = (0..strip.cards.len())
            .filter(|&i| dragged.is_none_or(|p| p.card != i))
            .chain(dragged.map(|p| p.card));
        for i in order {
            let card = &strip.cards[i];
            let drag = dragged.filter(|p| p.card == i).map(|p| p.offset);
            let r = drawn_rect(&card.rect, drag, strip.vertical);
            let layers = layer_rects(card, &r, strip.scale, strip.label);
            for (&t, layer) in layers.iter().zip(&card.layers).rev() {
                let edge = RECT {
                    left: t.left - 1,
                    top: t.top - 1,
                    right: t.right + 1,
                    bottom: t.bottom + 1,
                };
                let background = rgb(0x20, 0x20, 0x26);
                let neutral = rgb(0x5A, 0x5A, 0x66);
                if strip.colored_borders {
                    let color = layer.border.unwrap_or(neutral);
                    fill_round(hdc, &edge, px(6), color, color);
                    fill_round(hdc, &preview_rect(t, strip), px(2), background, background);
                } else {
                    fill_round(hdc, &edge, px(8), background, neutral);
                }
            }
            let Some(&t) = layers.first() else {
                continue;
            };
            if card.layers[0].thumb == 0 {
                // No live surface: the window's icon in its place.
                let icon = px(32);
                DrawIconEx(
                    hdc,
                    (t.left + t.right - icon) / 2,
                    (t.top + t.bottom - icon) / 2,
                    card.icons[0],
                    icon,
                    icon,
                    0,
                    null_mut(),
                    DI_NORMAL,
                );
            }
            label(hdc, strip, card, &r);
        }
    });
}

/// A card's label under its thumbnail: a line per window of a set, each led
/// by its icon; one window's title sits under its app name, after the icon's
/// room. The lines are left-aligned in a block centred under the card.
unsafe fn label(hdc: HDC, strip: &Strip, card: &Card, r: &RECT) {
    let px = |v| dpi::px(strip.scale, v);
    let (icon, gap, line) = (px(16), px(6), px(LINE));
    let top = r.bottom - strip.label + px(4);
    let white = (strip.bold, rgb(0xF0, 0xF0, 0xF4));
    let grey = (strip.font, rgb(0xA8, 0xA8, 0xB4));
    // A line with an icon names a window; the one without is its title.
    let lines: Vec<(Option<HICON>, &str, (HFONT, u32))> = card
        .lines
        .iter()
        .enumerate()
        .map(|(k, text)| match card.icons.get(k) {
            Some(&ic) => (Some(ic), text.as_str(), white),
            None => (None, text.as_str(), grey),
        })
        .collect();
    let block = lines
        .iter()
        .map(|(_, text, (font, _))| icon + gap + measure_text(hdc, *font, text))
        .max()
        .unwrap_or(0)
        .min(r.right - r.left - px(8));
    let x0 = (r.left + r.right - block) / 2;
    for (k, (ic, text, (font, color))) in lines.iter().enumerate() {
        let y = top + k as i32 * line;
        if let Some(ic) = *ic {
            DrawIconEx(
                hdc,
                x0,
                y + (line - icon) / 2,
                ic,
                icon,
                icon,
                0,
                null_mut(),
                DI_NORMAL,
            );
        }
        let x = x0 + icon + gap;
        let rect = RECT {
            left: x,
            top: y,
            right: x0 + block,
            bottom: y + line,
        };
        draw_text_in(
            hdc,
            *font,
            *color,
            &rect,
            text,
            DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LABEL: i32 = 4 + 2 * LINE;

    #[test]
    fn cards_keep_their_natural_size_while_they_fit() {
        let rects = card_rects(184, 1000, true, 3, 1.0, LABEL);
        assert_eq!(rects.len(), 3);
        let r = rects[0];
        assert_eq!((r.right - r.left, r.bottom - r.top), (164, 123 + LABEL));
        let (before, after) = (rects[0].top, 1000 - rects[2].bottom);
        assert!((before - after).abs() <= 1, "centred along the axis");
    }

    #[test]
    fn cards_shrink_to_fit_and_overflow_is_dropped() {
        let rects = card_rects(184, 800, true, 6, 1.0, LABEL);
        assert_eq!(rects.len(), 6);
        assert!(rects.iter().all(|r| r.bottom <= 790));
        let crowded = card_rects(184, 300, true, 20, 1.0, LABEL);
        assert!(crowded.len() < 20);
        assert!(crowded
            .iter()
            .all(|r| r.bottom - r.top >= 48 && r.bottom <= 290));
    }

    #[test]
    fn horizontal_strips_lay_cards_out_left_to_right() {
        let rects = card_rects(1000, 134, false, 2, 1.0, LABEL);
        assert_eq!(rects.len(), 2);
        assert!(rects[0].right <= rects[1].left);
        assert_eq!((rects[0].top, rects[0].bottom), (10, 124));
    }

    #[test]
    fn a_dragged_card_follows_the_axis_and_grows() {
        let r = RECT {
            left: 10,
            top: 100,
            right: 170,
            bottom: 220,
        };
        let rest = drawn_rect(&r, None, true);
        assert_eq!(
            (rest.left, rest.top, rest.right, rest.bottom),
            (10, 100, 170, 220)
        );
        let d = drawn_rect(&r, Some(30), true);
        assert!(d.right - d.left > 160 && d.bottom - d.top > 120);
        assert_eq!(centre(&d, true), centre(&r, true) + 30);
        assert_eq!((d.left + d.right) / 2, (r.left + r.right) / 2);
    }

    #[test]
    fn a_set_keeps_its_on_screen_arrangement() {
        let rect = |l, t, r, b| RECT {
            left: l,
            top: t,
            right: r,
            bottom: b,
        };
        // Side by side, 2000 x 1000 in all, into a 200 x 200 box.
        let frames = [rect(0, 0, 1000, 1000), rect(1000, 0, 2000, 1000)];
        let out: Vec<_> = composite(&frames, &rect(0, 0, 200, 200))
            .unwrap()
            .iter()
            .map(|r| (r.left, r.top, r.right, r.bottom))
            .collect();
        assert_eq!(out, vec![(0, 50, 100, 150), (100, 50, 200, 150)]);
        assert!(composite(&[rect(0, 0, 10, 10), EMPTY], &rect(0, 0, 200, 200)).is_none());
    }

    #[test]
    fn a_set_card_names_each_window_on_its_own_line() {
        let w = |a: &str, t: &str| (a.to_string(), t.to_string());
        assert_eq!(
            card_lines(&[w("Discord", "#general")]),
            ["Discord", "#general"]
        );
        assert_eq!(card_lines(&[w("Notepad", "Notepad")]), ["Notepad", ""]);
        assert_eq!(
            card_lines(&[w("Explorer", "Home"), w("", "Settings"), w("Discord", "#x")]),
            ["Explorer", "Settings", "Discord"]
        );
        assert_eq!(
            card_lines(&[w("Code", "a.rs"), w("Code", "b.rs"), w("Fork", "repo")]),
            ["a.rs", "b.rs", "Fork"]
        );
        assert_eq!(
            card_lines(&[w("A", "a"), w("B", "b"), w("C", "c"), w("D", "d")]),
            ["A", "B", "C, D"]
        );
    }

    #[test]
    fn a_set_card_stacks_its_layers_inside_the_thumbnail_area() {
        let layer = |n: usize| Layer {
            hwnd: n as HWND,
            thumb: 0,
            border: None,
            src: SIZE { cx: 0, cy: 0 },
            frame: EMPTY,
        };
        let r = card_rects(184, 1000, true, 1, 1.0, LABEL)[0];
        let card = Card {
            hwnd: 1 as HWND,
            rect: r,
            layers: vec![layer(1), layer(2), layer(3)],
            lines: Vec::new(),
            icons: Vec::new(),
        };
        let rects = layer_rects(&card, &r, 1.0, LABEL);
        assert_eq!(rects.len(), 3);
        let (top, back) = (rects[0], rects[2]);
        assert!(
            back.left > top.left && back.top < top.top,
            "back peeks up-right"
        );
        assert!(rects
            .iter()
            .all(|l| l.left >= r.left && l.right <= r.right && l.bottom <= r.bottom - LABEL));
    }

    #[test]
    fn border_colors_survive_changes_in_input_order() {
        let terminal = 1 as HWND;
        let deepseek = 2 as HWND;
        let codex = 3 as HWND;
        let mut colors = HashMap::new();
        assign_border_colors(&mut colors, vec![deepseek, terminal, codex]);
        let before = colors.clone();
        assert_eq!(
            before
                .values()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            3
        );

        assign_border_colors(&mut colors, vec![codex, deepseek, terminal]);
        assert_eq!(colors, before);
    }

    #[test]
    fn returning_windows_keep_their_color_when_it_is_free() {
        let first = 1 as HWND;
        let second = 2 as HWND;
        let third = 3 as HWND;
        let mut colors = HashMap::new();
        assign_border_colors(&mut colors, vec![first, second]);
        let color = colors[&second];
        assign_border_colors(&mut colors, vec![third]);
        assign_border_colors(&mut colors, vec![second, third]);
        assert_eq!(colors[&second], color);
        assert_ne!(colors[&third], color);
    }

    #[test]
    fn a_drop_lands_past_every_centre_it_crossed() {
        let rects = card_rects(184, 1000, true, 3, 1.0, LABEL);
        let centre = |r: &RECT| (r.top + r.bottom) / 2;
        assert_eq!(drop_slot(0, centre(&rects[0]), &rects, true), 0);
        assert_eq!(drop_slot(0, centre(&rects[1]) + 1, &rects, true), 1);
        assert_eq!(drop_slot(0, 5000, &rects, true), 2);
        assert_eq!(drop_slot(2, -5000, &rects, true), 0);
    }
}
