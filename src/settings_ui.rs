use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::Controls::Dialogs::{
    ChooseColorW, CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW,
};
use windows_sys::Win32::UI::Controls::{SetWindowTheme, DRAWITEMSTRUCT};
use windows_sys::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::config::{
    is_autostart_enabled, set_autostart, ChannelKind, Orientation, Position, Settings,
};
use crate::gfx::{argb, tint, Canvas, ALIGN_CENTER, ALIGN_NEAR};
use crate::{osd, rgb, state, wide};

pub const CLASS_NAME: &str = "VoicemeeterOsdSettingsClass";

// Stable Win32 constants that live outside the enabled windows-sys features.
const SS_LEFT: u32 = 0x0000_0000;
const ODS_SELECTED: u32 = 0x0001;
const ODT_BUTTON: u32 = 4;
const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;

const ID_EDIT_INDEX: i32 = 3003;
const ID_BTN_COLOR: i32 = 3006;
const ID_EDIT_OPACITY: i32 = 3008;
const ID_EDIT_HIDE: i32 = 3009;
const ID_CHK_AUTOSTART: i32 = 3010;
const ID_EDIT_MINDB: i32 = 3011;
const ID_EDIT_MAXDB: i32 = 3012;
const ID_BTN_TEST: i32 = 3013;
const ID_BTN_SAVE: i32 = 3014;
const ID_BTN_CANCEL: i32 = 3015;
const ID_CHK_ACCENT_SYS: i32 = 3016;
const ID_CHK_FULLSCREEN: i32 = 3017;
const ID_BTN_RESET: i32 = 3018;
/// IsDialogMessageW turns Enter/Esc into these.
const ID_OK: i32 = 1;
const ID_CANCEL: i32 = 2;
/// Section captions get the dimmed text color.
const ID_SECTION_FIRST: i32 = 3100;

// Segmented controls: each group owns a contiguous id range.
const ID_SEG_CHANNEL: i32 = 3200;
const ID_SEG_ORIENT: i32 = 3210;
const ID_SEG_POSITION: i32 = 3220;
const ID_SEG_END: i32 = 3230;

const SEG_GROUPS: [(i32, usize); 3] = [
    (ID_SEG_CHANNEL, 2),
    (ID_SEG_ORIENT, 2),
    (ID_SEG_POSITION, 2),
];

const BG: (u8, u8, u8) = (26, 26, 30);
const FIELD: (u8, u8, u8) = (44, 44, 50);
const TEXT: (u8, u8, u8) = (236, 236, 241);
const TEXT_DIM: (u8, u8, u8) = (142, 142, 154);
const DIVIDER: (u8, u8, u8) = (54, 54, 62);

static mut PENDING_ACCENT: (u8, u8, u8) = (70, 160, 255);
/// Selected index per segmented group, in SEG_GROUPS order.
static mut SEG_SELECTION: [usize; 3] = [0, 0, 0];
static mut AUTOSTART_ON: bool = false;
static mut ACCENT_SYS_ON: bool = false;
static mut FULLSCREEN_ON: bool = true;

/// Colour the UI should draw with right now, honouring follow-system.
unsafe fn current_accent() -> (u8, u8, u8) {
    if ACCENT_SYS_ON {
        crate::config::system_accent().unwrap_or(PENDING_ACCENT)
    } else {
        PENDING_ACCENT
    }
}
static mut UI_FONT: HGDIOBJ = null_mut();
static mut BG_BRUSH: HBRUSH = null_mut();
static mut FIELD_BRUSH: HBRUSH = null_mut();
static mut DPI: i32 = 96;
static mut PREVIEW_RECT: RECT = RECT {
    left: 0,
    top: 0,
    right: 0,
    bottom: 0,
};

const CLIENT_W: i32 = 470;
const WINDOW_STYLE: u32 = (WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX) as u32;

fn px(v: i32) -> i32 {
    unsafe { v * DPI / 96 }
}

pub fn open(hinstance: windows_sys::Win32::Foundation::HINSTANCE) {
    unsafe {
        let app = state();
        if !app.is_null() && !(*app).hwnd_settings.is_null() {
            let existing = (*app).hwnd_settings;
            ShowWindow(existing, SW_RESTORE);
            SetForegroundWindow(existing);
            return;
        }

        let hwnd = CreateWindowExW(
            0,
            wide(CLASS_NAME).as_ptr(),
            wide("Voicemeeter OSD").as_ptr(),
            WINDOW_STYLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CLIENT_W,
            600,
            null_mut(),
            null_mut(),
            hinstance,
            null_mut(),
        );
        if hwnd.is_null() {
            return;
        }

        let dark: u32 = 1;
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark as *const u32 as *const _,
            4,
        );

        if !app.is_null() {
            (*app).hwnd_settings = hwnd;
            let icon = (*app).icon_big as LPARAM;
            SendMessageW(hwnd, WM_SETICON, ICON_SMALL as WPARAM, icon);
            SendMessageW(hwnd, WM_SETICON, ICON_BIG as WPARAM, icon);
        }
        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
    }
}

unsafe fn themed(hwnd: HWND, theme: &str) {
    SetWindowTheme(hwnd, wide(theme).as_ptr(), null_mut());
}

unsafe fn label(parent: HWND, text: &str, x: i32, y: i32, w: i32, id: i32) -> HWND {
    let h = CreateWindowExW(
        0,
        wide("STATIC").as_ptr(),
        wide(text).as_ptr(),
        WS_CHILD | WS_VISIBLE | SS_LEFT,
        px(x),
        px(y),
        px(w),
        px(20),
        parent,
        id as isize as *mut _,
        null_mut(),
        null_mut(),
    );
    SendMessageW(h, WM_SETFONT, UI_FONT as WPARAM, 1);
    h
}

#[allow(clippy::too_many_arguments)]
unsafe fn control(
    parent: HWND,
    class: &str,
    text: &str,
    style: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    id: i32,
) -> HWND {
    let hwnd = CreateWindowExW(
        0,
        wide(class).as_ptr(),
        wide(text).as_ptr(),
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | style,
        px(x),
        px(y),
        px(w),
        px(h),
        parent,
        id as isize as *mut _,
        null_mut(),
        null_mut(),
    );
    SendMessageW(hwnd, WM_SETFONT, UI_FONT as WPARAM, 1);
    hwnd
}

/// A row of pill buttons acting as one mutually-exclusive choice.
unsafe fn segmented(
    parent: HWND,
    labels: &[&str],
    base_id: i32,
    x: i32,
    y: i32,
    total_w: i32,
    h: i32,
) {
    let gap = 6;
    let n = labels.len() as i32;
    let seg_w = (total_w - gap * (n - 1)) / n;
    for (i, text) in labels.iter().enumerate() {
        control(
            parent,
            "BUTTON",
            text,
            BS_OWNERDRAW as u32,
            x + (seg_w + gap) * i as i32,
            y,
            seg_w,
            h,
            base_id + i as i32,
        );
    }
}

fn seg_group_of(id: i32) -> Option<(usize, i32, usize)> {
    SEG_GROUPS
        .iter()
        .enumerate()
        .find(|(_, (base, count))| id >= *base && id < *base + *count as i32)
        .map(|(group, (base, count))| (group, *base, *count))
}

unsafe fn build_controls(hwnd: HWND) {
    DPI = GetDpiForWindow(hwnd).max(96) as i32;
    UI_FONT = CreateFontW(
        px(16),
        0,
        0,
        0,
        FW_NORMAL as i32,
        0,
        0,
        0,
        DEFAULT_CHARSET as u32,
        OUT_DEFAULT_PRECIS as u32,
        CLIP_DEFAULT_PRECIS as u32,
        CLEARTYPE_QUALITY as u32,
        (DEFAULT_PITCH | FF_SWISS) as u32,
        wide("Segoe UI").as_ptr(),
    ) as HGDIOBJ;
    BG_BRUSH = CreateSolidBrush(rgb(BG.0, BG.1, BG.2));
    FIELD_BRUSH = CreateSolidBrush(rgb(FIELD.0, FIELD.1, FIELD.2));

    let settings = {
        let app = state();
        if app.is_null() {
            Settings::default()
        } else {
            (*app).settings.clone()
        }
    };
    PENDING_ACCENT = settings.accent;

    let lx = 24;
    let cx = 186;
    let cw = 260;
    let row = 32;
    let field_h = 26;
    let mut section = ID_SECTION_FIRST;

    // --- Channel -------------------------------------------------------
    let mut y = 92;
    label(hwnd, "CHANNEL", lx, y, 200, section);
    section += 1;
    y += 26;

    SEG_SELECTION[0] = (settings.kind == ChannelKind::Bus) as usize;
    segmented(hwnd, &["Strip", "Bus (A1, A2...)"], ID_SEG_CHANNEL, lx, y, 300, 30);

    y += 40;
    label(hwnd, "Channel index", lx, y + 3, 150, 0);
    let index = control(
        hwnd,
        "EDIT",
        &settings.index.to_string(),
        (ES_NUMBER | ES_AUTOHSCROLL) as u32,
        cx,
        y,
        cw,
        field_h,
        ID_EDIT_INDEX,
    );
    themed(index, "DarkMode_CFD");

    // --- Appearance ----------------------------------------------------
    y += 42;
    label(hwnd, "APPEARANCE", lx, y, 200, section);
    section += 1;
    y += 26;

    label(hwnd, "Orientation", lx, y + 5, 150, 0);
    SEG_SELECTION[1] = (settings.orientation == Orientation::Horizontal) as usize;
    segmented(hwnd, &["Vertical", "Horizontal"], ID_SEG_ORIENT, cx, y, cw, 30);

    y += 38;
    label(hwnd, "Position", lx, y + 5, 150, 0);
    SEG_SELECTION[2] = (settings.position == Position::Centre) as usize;
    segmented(
        hwnd,
        &["Above tray", "Bottom centre"],
        ID_SEG_POSITION,
        cx,
        y,
        cw,
        30,
    );

    y += 38;
    label(hwnd, "Accent color", lx, y + 3, 150, 0);
    control(
        hwnd,
        "BUTTON",
        "Choose color",
        BS_OWNERDRAW as u32,
        cx,
        y,
        cw,
        field_h,
        ID_BTN_COLOR,
    );

    y += row;
    label(hwnd, "Match Windows accent", lx, y + 5, 200, 0);
    ACCENT_SYS_ON = settings.accent_follow_system;
    control(
        hwnd,
        "BUTTON",
        "",
        BS_OWNERDRAW as u32,
        cx,
        y,
        46,
        26,
        ID_CHK_ACCENT_SYS,
    );

    y += row;
    label(hwnd, "Opacity", lx, y + 3, 150, 0);
    let opacity = control(
        hwnd,
        "EDIT",
        &settings.opacity_pct.to_string(),
        (ES_NUMBER | ES_AUTOHSCROLL) as u32,
        cx,
        y,
        cw,
        field_h,
        ID_EDIT_OPACITY,
    );
    themed(opacity, "DarkMode_CFD");

    // --- Behavior ------------------------------------------------------
    y += 42;
    label(hwnd, "BEHAVIOR", lx, y, 200, section);
    y += 26;

    label(hwnd, "Hide after (ms)", lx, y + 3, 150, 0);
    let hide = control(
        hwnd,
        "EDIT",
        &settings.hide_ms.to_string(),
        (ES_NUMBER | ES_AUTOHSCROLL) as u32,
        cx,
        y,
        cw,
        field_h,
        ID_EDIT_HIDE,
    );
    themed(hide, "DarkMode_CFD");

    y += row;
    label(hwnd, "Fader range (dB)", lx, y + 3, 150, 0);
    let min_db = control(
        hwnd,
        "EDIT",
        &settings.min_db.to_string(),
        ES_AUTOHSCROLL as u32,
        cx,
        y,
        (cw - 16) / 2,
        field_h,
        ID_EDIT_MINDB,
    );
    let max_db = control(
        hwnd,
        "EDIT",
        &settings.max_db.to_string(),
        ES_AUTOHSCROLL as u32,
        cx + (cw - 16) / 2 + 16,
        y,
        (cw - 16) / 2,
        field_h,
        ID_EDIT_MAXDB,
    );
    themed(min_db, "DarkMode_CFD");
    themed(max_db, "DarkMode_CFD");

    y += row + 6;
    label(hwnd, "Hide during fullscreen", lx, y + 5, 200, 0);
    FULLSCREEN_ON = settings.hide_in_fullscreen;
    control(
        hwnd,
        "BUTTON",
        "",
        BS_OWNERDRAW as u32,
        cx,
        y,
        46,
        26,
        ID_CHK_FULLSCREEN,
    );

    y += row;
    label(hwnd, "Start with Windows", lx, y + 5, 200, 0);
    AUTOSTART_ON = is_autostart_enabled();
    control(
        hwnd,
        "BUTTON",
        "",
        BS_OWNERDRAW as u32,
        cx,
        y,
        46,
        26,
        ID_CHK_AUTOSTART,
    );

    // --- Preview -------------------------------------------------------
    y += 40;
    label(hwnd, "PREVIEW", lx, y, 200, section + 1);
    y += 24;
    PREVIEW_RECT = RECT {
        left: px(lx),
        top: px(y),
        right: px(CLIENT_W - lx),
        bottom: px(y + 112),
    };

    // --- Footer --------------------------------------------------------
    y += 130;
    control(hwnd, "BUTTON", "Reset", BS_OWNERDRAW as u32, lx, y, 96, 34, ID_BTN_RESET);
    control(hwnd, "BUTTON", "Test", BS_OWNERDRAW as u32, lx + 104, y, 96, 34, ID_BTN_TEST);
    control(
        hwnd,
        "BUTTON",
        "Cancel",
        BS_OWNERDRAW as u32,
        CLIENT_W - lx - 200,
        y,
        96,
        34,
        ID_BTN_CANCEL,
    );
    control(
        hwnd,
        "BUTTON",
        "Save",
        BS_OWNERDRAW as u32,
        CLIENT_W - lx - 96,
        y,
        96,
        34,
        ID_BTN_SAVE,
    );

    let mut rc = RECT {
        left: 0,
        top: 0,
        right: px(CLIENT_W),
        bottom: px(y + 34 + 20),
    };
    AdjustWindowRectExForDpi(&mut rc, WINDOW_STYLE, 0, 0, DPI as u32);
    SetWindowPos(
        hwnd,
        null_mut(),
        0,
        0,
        rc.right - rc.left,
        rc.bottom - rc.top,
        SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
    );
}

/// Pushes a Settings back into the live controls (used by Reset).
unsafe fn repopulate(hwnd: HWND, s: &Settings) {
    SetWindowTextW(GetDlgItem(hwnd, ID_EDIT_INDEX), wide(&s.index.to_string()).as_ptr());
    SetWindowTextW(
        GetDlgItem(hwnd, ID_EDIT_OPACITY),
        wide(&s.opacity_pct.to_string()).as_ptr(),
    );
    SetWindowTextW(
        GetDlgItem(hwnd, ID_EDIT_HIDE),
        wide(&s.hide_ms.to_string()).as_ptr(),
    );
    SetWindowTextW(
        GetDlgItem(hwnd, ID_EDIT_MINDB),
        wide(&s.min_db.to_string()).as_ptr(),
    );
    SetWindowTextW(
        GetDlgItem(hwnd, ID_EDIT_MAXDB),
        wide(&s.max_db.to_string()).as_ptr(),
    );
    SEG_SELECTION[0] = (s.kind == ChannelKind::Bus) as usize;
    SEG_SELECTION[1] = (s.orientation == Orientation::Horizontal) as usize;
    SEG_SELECTION[2] = (s.position == Position::Centre) as usize;
    PENDING_ACCENT = s.accent;
    ACCENT_SYS_ON = s.accent_follow_system;
    FULLSCREEN_ON = s.hide_in_fullscreen;
    InvalidateRect(hwnd, null_mut(), 1);
    for (base, count) in SEG_GROUPS {
        for i in 0..count as i32 {
            InvalidateRect(GetDlgItem(hwnd, base + i), null_mut(), 0);
        }
    }
}

unsafe fn get_text(hwnd: HWND, id: i32) -> String {
    let mut buf = [0u16; 64];
    let len = GetWindowTextW(GetDlgItem(hwnd, id), buf.as_mut_ptr(), buf.len() as i32);
    String::from_utf16_lossy(&buf[..len.max(0) as usize])
}

unsafe fn read_settings(hwnd: HWND) -> Settings {
    let mut s = {
        let app = state();
        if app.is_null() {
            Settings::default()
        } else {
            (*app).settings.clone()
        }
    };

    s.kind = if SEG_SELECTION[0] == 1 {
        ChannelKind::Bus
    } else {
        ChannelKind::Strip
    };

    if let Ok(v) = get_text(hwnd, ID_EDIT_INDEX).trim().parse::<i32>() {
        s.index = v.clamp(0, 63);
    }
    if let Ok(v) = get_text(hwnd, ID_EDIT_OPACITY).trim().parse::<u8>() {
        s.opacity_pct = v.clamp(20, 100);
    }
    if let Ok(v) = get_text(hwnd, ID_EDIT_HIDE).trim().parse::<u32>() {
        s.hide_ms = v.clamp(200, 10_000);
    }
    if let Ok(v) = get_text(hwnd, ID_EDIT_MINDB).trim().parse::<f32>() {
        s.min_db = v;
    }
    if let Ok(v) = get_text(hwnd, ID_EDIT_MAXDB).trim().parse::<f32>() {
        s.max_db = v;
    }
    if s.max_db <= s.min_db {
        s.max_db = s.min_db + 1.0;
    }

    s.orientation = if SEG_SELECTION[1] == 1 {
        Orientation::Horizontal
    } else {
        Orientation::Vertical
    };
    s.position = if SEG_SELECTION[2] == 1 {
        Position::Centre
    } else {
        Position::AboveTray
    };
    s.accent = PENDING_ACCENT;
    s.accent_follow_system = ACCENT_SYS_ON;
    s.hide_in_fullscreen = FULLSCREEN_ON;
    s
}

unsafe fn apply(hwnd: HWND) {
    let new_settings = read_settings(hwnd);
    let app = state();
    if app.is_null() {
        return;
    }
    let app = &mut *app;
    let channel_changed =
        app.settings.kind != new_settings.kind || app.settings.index != new_settings.index;
    app.settings = new_settings;
    if channel_changed {
        app.last_gain = None;
        app.last_mute = None;
        app.refresh_channel();
    }
    crate::relayout_osd();
    set_autostart(AUTOSTART_ON);
}

unsafe fn paint(hwnd: HWND) {
    let mut ps: PAINTSTRUCT = std::mem::zeroed();
    let hdc = BeginPaint(hwnd, &mut ps);

    let mut client = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    GetClientRect(hwnd, &mut client);
    let w = client.right;
    let h = client.bottom;

    let mem_dc = CreateCompatibleDC(hdc);
    let mem_bmp = CreateCompatibleBitmap(hdc, w, h);
    let old = SelectObject(mem_dc, mem_bmp);
    FillRect(mem_dc, &client, BG_BRUSH);

    {
        let canvas = Canvas::from_hdc(mem_dc);
        let s = |v: f32| v * DPI as f32 / 96.0;

        // Header: app icon, title, subtitle.
        let app = state();
        if !app.is_null() {
            let icon = (*app).icon_big;
            if !icon.is_null() {
                DrawIconEx(
                    mem_dc,
                    px(24),
                    px(20),
                    icon,
                    px(38),
                    px(38),
                    0,
                    null_mut(),
                    DI_NORMAL,
                );
            }
        }
        canvas.draw_text(
            "Voicemeeter OSD",
            s(74.0),
            s(22.0),
            s(300.0),
            s(22.0),
            s(17.0),
            tint(TEXT, 255),
            ALIGN_NEAR,
            true,
        );
        canvas.draw_text(
            "On-screen volume display",
            s(74.0),
            s(42.0),
            s(300.0),
            s(18.0),
            s(12.5),
            tint(TEXT_DIM, 255),
            ALIGN_NEAR,
            false,
        );
        canvas.fill_round_rect(
            s(0.0),
            s(76.0),
            w as f32,
            s(1.0),
            0.0,
            tint(DIVIDER, 255),
        );

        // Preview panel with a live miniature of the bar.
        // A lighter "desktop" backdrop so the dark OSD card reads against it.
        let pr = PREVIEW_RECT;
        canvas.fill_round_rect_gradient(
            pr.left as f32,
            pr.top as f32,
            (pr.right - pr.left) as f32,
            (pr.bottom - pr.top) as f32,
            s(10.0),
            argb(255, 58, 60, 74),
            argb(255, 34, 35, 44),
        );
        canvas.stroke_round_rect(
            pr.left as f32 + 0.5,
            pr.top as f32 + 0.5,
            (pr.right - pr.left) as f32 - 1.0,
            (pr.bottom - pr.top) as f32 - 1.0,
            s(10.0),
            tint(DIVIDER, 255),
            1.0,
        );

        let settings = read_settings(hwnd);
        let panel_cx = (pr.left + pr.right) as f32 / 2.0;
        let panel_cy = (pr.top + pr.bottom) as f32 / 2.0;
        let (pw, ph) = if settings.orientation == Orientation::Vertical {
            (s(54.0), s(104.0))
        } else {
            (s(252.0), s(62.0))
        };
        osd::draw_preview(
            &canvas,
            &settings,
            panel_cx - pw / 2.0,
            panel_cy - ph / 2.0,
            pw,
            ph,
        );
    }

    BitBlt(hdc, 0, 0, w, h, mem_dc, 0, 0, SRCCOPY);
    SelectObject(mem_dc, old);
    DeleteObject(mem_bmp);
    DeleteDC(mem_dc);
    EndPaint(hwnd, &ps);
}

unsafe fn button_label(dis: *const DRAWITEMSTRUCT) -> String {
    let mut text = [0u16; 64];
    let len = GetWindowTextW((*dis).hwndItem, text.as_mut_ptr(), 64);
    String::from_utf16_lossy(&text[..len.max(0) as usize])
}

unsafe fn draw_segment(dis: *const DRAWITEMSTRUCT, selected: bool) {
    let rc = (*dis).rcItem;
    let w = (rc.right - rc.left) as f32;
    let h = (rc.bottom - rc.top) as f32;
    let canvas = Canvas::from_hdc((*dis).hDC);
    let scale = DPI as f32 / 96.0;

    FillRect((*dis).hDC, &rc, BG_BRUSH);

    let (fill, text_color) = if selected {
        (tint(current_accent(), 255), argb(255, 255, 255, 255))
    } else {
        (tint(FIELD, 255), tint(TEXT_DIM, 255))
    };
    canvas.fill_round_rect(0.0, 0.0, w, h, h / 2.0, fill);
    if !selected {
        canvas.stroke_round_rect(0.5, 0.5, w - 1.0, h - 1.0, h / 2.0, tint(DIVIDER, 255), 1.0);
    }
    canvas.draw_text(
        &button_label(dis),
        0.0,
        0.0,
        w,
        h,
        13.5 * scale,
        text_color,
        ALIGN_CENTER,
        selected,
    );
}

unsafe fn draw_toggle(dis: *const DRAWITEMSTRUCT, on: bool) {
    let rc = (*dis).rcItem;
    let w = (rc.right - rc.left) as f32;
    let h = (rc.bottom - rc.top) as f32;
    let canvas = Canvas::from_hdc((*dis).hDC);

    FillRect((*dis).hDC, &rc, BG_BRUSH);

    let track_h = h * 0.78;
    let track_y = (h - track_h) / 2.0;
    let fill = if on {
        tint(current_accent(), 255)
    } else {
        tint(FIELD, 255)
    };
    canvas.fill_round_rect(0.0, track_y, w, track_h, track_h / 2.0, fill);
    if !on {
        canvas.stroke_round_rect(
            0.5,
            track_y + 0.5,
            w - 1.0,
            track_h - 1.0,
            track_h / 2.0,
            tint(DIVIDER, 255),
            1.0,
        );
    }

    let knob = track_h - 6.0;
    let knob_x = if on { w - knob - 3.0 } else { 3.0 };
    canvas.fill_ellipse(
        knob_x,
        track_y + 3.0,
        knob,
        knob,
        argb(255, 255, 255, 255),
    );
}

unsafe fn draw_button(dis: *const DRAWITEMSTRUCT) {
    let rc = (*dis).rcItem;
    let id = (*dis).CtlID as i32;
    let pressed = (*dis).itemState & ODS_SELECTED != 0;
    let w = (rc.right - rc.left) as f32;
    let h = (rc.bottom - rc.top) as f32;
    let radius = (6 * DPI / 96) as f32;

    let canvas = Canvas::from_hdc((*dis).hDC);
    let primary = id == ID_BTN_SAVE;

    let accent = current_accent();
    let (fill, border, text_color) = if primary {
        let base = if pressed {
            crate::gfx::shade(accent, -0.18)
        } else {
            accent
        };
        (tint(base, 255), tint(base, 255), argb(255, 255, 255, 255))
    } else {
        let base = if pressed { (58, 58, 66) } else { FIELD };
        (tint(base, 255), tint(DIVIDER, 255), tint(TEXT, 255))
    };

    canvas.fill_round_rect(0.0, 0.0, w, h, radius, fill);
    canvas.stroke_round_rect(0.5, 0.5, w - 1.0, h - 1.0, radius, border, 1.0);

    let label = button_label(dis);

    if id == ID_BTN_COLOR {
        // Color chip plus caption.
        let chip = h - (10 * DPI / 96) as f32;
        canvas.fill_round_rect(
            (8 * DPI / 96) as f32,
            (5 * DPI / 96) as f32,
            chip * 1.6,
            chip,
            (4 * DPI / 96) as f32,
            tint(accent, 255),
        );
        canvas.draw_text(
            &label,
            chip * 1.6 + (18 * DPI / 96) as f32,
            0.0,
            w,
            h,
            (14 * DPI / 96) as f32,
            text_color,
            ALIGN_NEAR,
            false,
        );
    } else {
        canvas.draw_text(
            &label,
            0.0,
            0.0,
            w,
            h,
            (14 * DPI / 96) as f32,
            text_color,
            ALIGN_CENTER,
            primary,
        );
    }
}

pub unsafe extern "system" fn wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_CREATE => {
            build_controls(hwnd);
            0
        }
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            paint(hwnd);
            0
        }
        WM_CTLCOLORSTATIC | WM_CTLCOLORBTN => {
            let hdc = wparam as HDC;
            let id = GetDlgCtrlID(lparam as HWND);
            let color = if id >= ID_SECTION_FIRST {
                TEXT_DIM
            } else {
                TEXT
            };
            SetTextColor(hdc, rgb(color.0, color.1, color.2));
            SetBkColor(hdc, rgb(BG.0, BG.1, BG.2));
            BG_BRUSH as LRESULT
        }
        WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX => {
            let hdc = wparam as HDC;
            SetTextColor(hdc, rgb(TEXT.0, TEXT.1, TEXT.2));
            SetBkColor(hdc, rgb(FIELD.0, FIELD.1, FIELD.2));
            FIELD_BRUSH as LRESULT
        }
        WM_DRAWITEM => {
            let dis = lparam as *const DRAWITEMSTRUCT;
            if dis.is_null() || (*dis).CtlType != ODT_BUTTON {
                return 0;
            }
            let id = (*dis).CtlID as i32;
            if let Some((group, base, _)) = seg_group_of(id) {
                draw_segment(dis, SEG_SELECTION[group] == (id - base) as usize);
            } else if id == ID_CHK_AUTOSTART {
                draw_toggle(dis, AUTOSTART_ON);
            } else if id == ID_CHK_ACCENT_SYS {
                draw_toggle(dis, ACCENT_SYS_ON);
            } else if id == ID_CHK_FULLSCREEN {
                draw_toggle(dis, FULLSCREEN_ON);
            } else {
                draw_button(dis);
            }
            1
        }
        WM_COMMAND => {
            let id = (wparam & 0xFFFF) as i32;
            let notification = ((wparam >> 16) & 0xFFFF) as u32;
            match id {
                ID_BTN_COLOR if !ACCENT_SYS_ON => {
                    let mut custom = [0u32; 16];
                    let mut cc: CHOOSECOLORW = std::mem::zeroed();
                    cc.lStructSize = std::mem::size_of::<CHOOSECOLORW>() as u32;
                    cc.hwndOwner = hwnd;
                    cc.rgbResult = rgb(PENDING_ACCENT.0, PENDING_ACCENT.1, PENDING_ACCENT.2);
                    cc.lpCustColors = custom.as_mut_ptr();
                    cc.Flags = CC_FULLOPEN | CC_RGBINIT;
                    if ChooseColorW(&mut cc) != 0 {
                        let c = cc.rgbResult;
                        PENDING_ACCENT = (c as u8, (c >> 8) as u8, (c >> 16) as u8);
                        InvalidateRect(hwnd, null_mut(), 0);
                    }
                }
                ID_BTN_TEST => {
                    apply(hwnd);
                    crate::show_preview();
                }
                ID_BTN_SAVE | ID_OK => {
                    apply(hwnd);
                    let app = state();
                    if !app.is_null() {
                        (*app).settings.save();
                    }
                    DestroyWindow(hwnd);
                }
                ID_BTN_CANCEL | ID_CANCEL => {
                    DestroyWindow(hwnd);
                }
                ID_BTN_RESET => {
                    repopulate(hwnd, &Settings::default());
                }
                ID_CHK_ACCENT_SYS => {
                    ACCENT_SYS_ON = !ACCENT_SYS_ON;
                    InvalidateRect(hwnd, null_mut(), 0);
                }
                ID_CHK_FULLSCREEN => {
                    FULLSCREEN_ON = !FULLSCREEN_ON;
                    InvalidateRect(GetDlgItem(hwnd, ID_CHK_FULLSCREEN), null_mut(), 0);
                }
                ID_CHK_AUTOSTART => {
                    AUTOSTART_ON = !AUTOSTART_ON;
                    InvalidateRect(GetDlgItem(hwnd, ID_CHK_AUTOSTART), null_mut(), 0);
                }
                _ if id >= ID_SEG_CHANNEL && id < ID_SEG_END => {
                    if let Some((group, base, count)) = seg_group_of(id) {
                        SEG_SELECTION[group] = (id - base) as usize;
                        for i in 0..count as i32 {
                            InvalidateRect(GetDlgItem(hwnd, base + i), null_mut(), 0);
                        }
                        let preview = PREVIEW_RECT;
                        InvalidateRect(hwnd, &preview, 0);
                    }
                }
                _ => {
                    // Live-update the preview as fields change.
                    if notification == EN_CHANGE {
                        let preview = PREVIEW_RECT;
                        InvalidateRect(hwnd, &preview, 0);
                    }
                }
            }
            0
        }
        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            let app = state();
            if !app.is_null() {
                (*app).hwnd_settings = null_mut();
            }
            if !UI_FONT.is_null() {
                DeleteObject(UI_FONT);
                UI_FONT = null_mut();
            }
            if !BG_BRUSH.is_null() {
                DeleteObject(BG_BRUSH as HGDIOBJ);
                BG_BRUSH = null_mut();
            }
            if !FIELD_BRUSH.is_null() {
                DeleteObject(FIELD_BRUSH as HGDIOBJ);
                FIELD_BRUSH = null_mut();
            }
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
