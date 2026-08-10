# Windows Desktop Window Manager (DWM) & Window Snapping Reference

This document records technical findings, API behaviors, edge cases, and mathematical formulas for programmatic window placement, snapping, DPI awareness, and Desktop Window Manager (DWM) frame bounds in Windows 10 & 11.

---

## 1. Overview & Problem Statement

Programmatically restoring or moving Win32 windows in Windows 11 (via `SetWindowPos` or `SetWindowPlacement`) often introduces subtle layout bugs:
- **Wallpaper Gaps / Padding**: ~7px gaps between adjacent windows, screen edges, or the taskbar.
- **Rounded Corners on Snapped Windows**: Windows 11 DWM renders 12px rounded corners on normal windows, leaving visible background gaps when windows are snapped side-by-side.
- **DPI Scaling Shift**: Moving windows across displays with different scaling factors (e.g. 150% vs 125%) shifts window coordinates by scaling ratios unless process DPI context is initialized.
- **Chromium Restored Minimum Width**: Chromium applications (Brave, Chrome, WhatsApp Web) enforce an un-snapped minimum window width (~800px), causing windows to swell and overlap adjacent windows if placement flags are incorrect.

---

## 2. Key Technical Findings

### 2.1 Invisible DWM Shadow Margins

Every standard Win32 desktop window (including Chromium and Qt apps) has invisible resize borders and drop-shadow margins managed by DWM:
- `GetWindowRect(hwnd)` returns the outer window bounding rectangle **including** invisible shadow borders.
- `DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, ...)` returns the actual **visible content frame**.

Typically, the shadow margins are:
- `Margin Left`: 7px
- `Margin Top`: 0px (or caption height offset)
- `Margin Right`: 7px
- `Margin Bottom`: 7px

> **Crucial Concept**: Passing exact screen work area coordinates (e.g. `[0, 0, 1920, 1080]`) to `SetWindowPos` or `SetWindowPlacement` causes DWM to inset the visible frame by 7px on all sides, resulting in a **7px gap** around the window.

### 2.2 Windows 11 Corner Preference (`DWMWA_WINDOW_CORNER_PREFERENCE`)

Windows 11 DWM applies 12px rounded corners to normal windows. When windows are snapped natively (`Win + Left`, `Win + Right`, Snap Layouts), DWM disables corner curvature on snapped edges (flat 90° square corners).

Programmatically, corner curvature is controlled via `DwmSetWindowAttribute`:

```rust
const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;

enum DWM_WINDOW_CORNER_PREFERENCE {
    DWMWCP_DEFAULT = 0,     // System default (rounded corners)
    DWMWCP_DONOTROUND = 1,  // Never round window corners (flat 90° square corners)
    DWMWCP_ROUND = 2,       // Force rounded corners
    DWMWCP_ROUNDSMALL = 3,  // Small radius rounded corners
}
```

- When restoring a **snapped** window rule, setting `DWMWCP_DONOTROUND` (`1`) removes rounded corners.
- When restoring a **freeform** window rule, setting `DWMWCP_DEFAULT` (`0`) preserves standard rounded corners.
- Calling `SetWindowPos` with `SWP_FRAMECHANGED` (`0x0037`) forces DWM to immediately redraw the non-client frame.

### 2.3 Per-Monitor V2 DPI Awareness (`DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2`)

In multi-monitor configurations with mixed DPI scaling (e.g. Monitor 1 at 150%, Monitor 2 at 125%):
- Without explicit Per-Monitor V2 DPI awareness, Windows treats the background process as System DPI Aware (scaled relative to Primary Monitor).
- Win32 calls (`GetWindowRect`, `SetWindowPos`) on secondary monitors automatically undergo virtual coordinate scaling (`1.5 / 1.25 = 1.2x`), introducing ~5-10px coordinate padding/shifts.

**Fix**: Must be called at process entry in `main()`:
```rust
windows_sys::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
    windows_sys::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
);
```

---

## 3. Mathematical Formulas for Flush Window Snapping

To ensure that a window's **visible content frame** (`DWMWA_EXTENDED_FRAME_BOUNDS`) matches a target screen rectangle `[target_left, target_top, target_right, target_bottom]` with **0 gap** and **0 overlap**:

### Step 1: Extract DWM Shadow Margins
```rust
let mut win_rect: RECT = std::mem::zeroed();
let mut frame_rect: RECT = std::mem::zeroed();

GetWindowRect(hwnd, &mut win_rect);
DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, &mut frame_rect, size_of::<RECT>());

let m_left   = (frame_rect.left - win_rect.left).max(0);
let m_top    = (frame_rect.top - win_rect.top).max(0);
let m_right  = (win_rect.right - frame_rect.right).max(0);
let m_bottom = (win_rect.bottom - frame_rect.bottom).max(0);
```

### Step 2: Calculate Expanded Placement Rectangle
For snapped windows (`is_snapped = true`):
```rust
let final_left   = target_left - m_left;
let final_top    = target_top - m_top;
let final_right  = target_right + m_right;
let final_bottom = target_bottom + m_bottom;
```

### Step 3: Apply Placement and Frame Refresh
```rust
// 1. Disable rounded corners for snapped window
let corner_pref: u32 = 1; // DWMWCP_DONOTROUND
DwmSetWindowAttribute(hwnd, 33, &corner_pref, sizeof(u32));

// 2. Apply window placement
let mut wp: WINDOWPLACEMENT = std::mem::zeroed();
wp.length = sizeof::<WINDOWPLACEMENT>();
wp.showCmd = SW_SHOWNORMAL;
wp.rcNormalPosition = RECT {
    left: final_left,
    top: final_top,
    right: final_right,
    bottom: final_bottom,
};
SetWindowPlacement(hwnd, &wp);

// 3. Set window position with frame changed signal
SetWindowPos(
    hwnd,
    null_mut(),
    final_left,
    final_top,
    final_right - final_left,
    final_bottom - final_top,
    SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
);
```

### Adjacent Window Alignment Example (50% Split)

Target Work Area on Display 2 (`-1536` to `0`, Top `534`, Bottom `1494`):
- **Left Half Target**: `[-1536, 534, -768, 1494]`
- **Right Half Target**: `[-768, 534, 0, 1494]`

Applying DWM Compensation (`m_left = 7, m_right = 7, m_bottom = 7`):
- **Left Window (`WhatsApp Web`) Outer Bounds**: `[-1543, 534, -761, 1501]`
- **Right Window (`Telegram`) Outer Bounds**: `[-775, 534, 7, 1501]`

> **Resulting Visible Frames**:
> - Left Window Visible Frame: `[-1536, 534, -768, 1494]`
> - Right Window Visible Frame: `[-768, 534, 0, 1494]`
> - **Divider Contact Point**: Both windows meet at `-768` with **0 gap** and **0 overlap**.

---

## 4. Window Identification & Differentiation (AUMID)

### 4.1 The Window Identity Problem in Win32
- **HWND Ephemerality**: In Win32, `HWND` (Window Handle) is dynamically assigned by `user32.dll` per session and changes upon restart.
- **Shared Executables**: Progressive Web Apps (PWAs), Chrome/Brave web applications (e.g. WhatsApp Web, Slack PWA), and multiple browser instances all share the same executable (`brave.exe` or `chrome.exe`) and class name (`Chrome_WidgetWin_1`).
- **Dynamic Titles**: Relying exclusively on window titles (`GetWindowTextW`) is fragile because titles change dynamically with web page navigation, chat changes, or notification counters (e.g., `WhatsApp Web` -> `(3) WhatsApp Web`).

### 4.2 `AppUserModelID` (`PKEY_AppUserModel_ID`)
Windows Shell provides the `IPropertyStore` interface on top-level windows via `SHGetPropertyStoreForWindow`:
- Property Key: `PKEY_AppUserModel_ID` (`{9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3}, 5`).
- **Chromium PWAs**: Each installed web app is assigned a permanent, unique App ID (e.g. `BraveOrigin._crx_hnpfjngllnfapefoaidbinmjnm` for WhatsApp Web).
- **Packaged / UWP Apps**: Store explicit package family names (e.g. `windows.immersivecontrolpanel_...`).

### 4.3 WinSpaces Multi-Attribute Fingerprint Hierarchy
WinSpaces matches windows using a weighted scoring model:

| Attribute | Points | Purpose |
|---|---|---|
| **`AUMID`** | `+100` | Uniquely identifies PWAs, Chrome apps, and packaged apps regardless of title changes. |
| **`TitlePattern`** | `+30` | Matches contextual titles or user-configured patterns. |
| **`ExePath` / Image** | `+20` | Differentiates distinct applications (`Telegram.exe`, `Orca.exe`). |
| **`ClassName`** | `+10` | Differentiates internal window classes within the same process. |

Rules with a specified `AUMID` or `TitlePattern` that do not match the target window are disqualified.

`AUMID` matching is **exact** (case-insensitive) — never substring. Chromium browsers assign `Brave` to the default profile and `Brave.<profile>` to other profiles; a substring match would let the default-profile rule claim every profile's windows. Hand-authored rules must therefore contain the complete AUMID. `TitlePattern` matching is one-directional: the window title must contain the pattern.
