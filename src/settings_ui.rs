use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::Controls::Dialogs::{
    ChooseColorW, CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW,
};
use windows_sys::Win32::UI::Controls::{
    SetWindowTheme, BST_CHECKED, BST_UNCHECKED, CDDS_PREPAINT, CDIS_FOCUS, CDIS_SHOWKEYBOARDCUES,
    CDRF_DODEFAULT, CDRF_SKIPDEFAULT, DRAWITEMSTRUCT, NMCUSTOMDRAW, NMHDR, NM_CUSTOMDRAW,
};
use windows_sys::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetFocus, SetFocus};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::config::{
    self, is_autostart_enabled, set_autostart, ChannelKind, Hotkey, Orientation, Position, Settings,
};
use crate::gfx::{argb, tint, Canvas, ALIGN_CENTER, ALIGN_NEAR};
use crate::{osd, rgb, state, wide};

pub const CLASS_NAME: &str = "VoicemeeterOsdSettingsClass";

// Stable Win32 constants that live outside the enabled windows-sys features.
const SS_LEFT: u32 = 0x0000_0000;
const ODS_SELECTED: u32 = 0x0001;
const ODS_FOCUS: u32 = 0x0010;
const ODS_NOFOCUSRECT: u32 = 0x0200;
const ODT_BUTTON: u32 = 4;
const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
const EM_SETSEL: u32 = 0x00B1;
const HOTKEY_CLASS: &str = "msctls_hotkey32";
const HKM_SETHOTKEY: u32 = WM_USER + 1;
const HKM_GETHOTKEY: u32 = WM_USER + 2;
const HKM_SETRULES: u32 = WM_USER + 3;
const HKCOMB_NONE: usize = 0x0001;
const HKCOMB_S: usize = 0x0002;
const HOTKEYF_CONTROL_ALT: isize = 0x02 | 0x04;

const ID_COMBO_CHANNEL: i32 = 3001;
const ID_BTN_COLOR: i32 = 3006;
const ID_EDIT_OPACITY: i32 = 3008;
const ID_EDIT_HIDE: i32 = 3009;
const ID_EDIT_MINDB: i32 = 3011;
const ID_EDIT_MAXDB: i32 = 3012;
const ID_BTN_TEST: i32 = 3013;
const ID_BTN_SAVE: i32 = 3014;
const ID_BTN_CANCEL: i32 = 3015;
const ID_BTN_RESET: i32 = 3018;
const ID_EDIT_STEP: i32 = 3022;
const ID_HK_UP: i32 = 3030;
const ID_HK_DOWN: i32 = 3031;
const ID_HK_MUTE: i32 = 3032;
/// IsDialogMessageW turns Enter/Esc into these.
const ID_OK: i32 = 1;
const ID_CANCEL: i32 = 2;

// Toggle switches, one id per boolean.
const ID_CHK_AUTOSTART: i32 = 3010;
const ID_CHK_ACCENT_SYS: i32 = 3016;
const ID_CHK_FULLSCREEN: i32 = 3017;
const ID_CHK_WATCH: i32 = 3019;
const ID_CHK_HOTKEYS: i32 = 3020;
const ID_CHK_DEBUG: i32 = 3021;
const ID_CHK_SCROLL: i32 = 3023;
const TOGGLE_IDS: [i32; 7] = [
    ID_CHK_AUTOSTART,
    ID_CHK_ACCENT_SYS,
    ID_CHK_FULLSCREEN,
    ID_CHK_WATCH,
    ID_CHK_HOTKEYS,
    ID_CHK_DEBUG,
    ID_CHK_SCROLL,
];

/// Section captions get the dimmed text color.
const ID_SECTION_FIRST: i32 = 3100;
const ID_SECTION_END: i32 = 3200;

// Segmented controls: each group owns a contiguous id range.
const ID_SEG_ORIENT: i32 = 3210;
const ID_SEG_POSITION: i32 = 3220;
const ID_SEG_END: i32 = 3230;

const SEG_GROUPS: [(i32, usize); 2] = [(ID_SEG_ORIENT, 2), (ID_SEG_POSITION, 3)];

const BG: (u8, u8, u8) = (26, 26, 30);
const FIELD: (u8, u8, u8) = (44, 44, 50);
const TEXT: (u8, u8, u8) = (236, 236, 241);
const TEXT_DIM: (u8, u8, u8) = (142, 142, 154);
const DIVIDER: (u8, u8, u8) = (54, 54, 62);

const CLIENT_W: i32 = 896;
const WINDOW_STYLE: u32 = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;

/// Everything the open settings window owns, attached to it through
/// GWLP_USERDATA and freed when it's destroyed.
struct Ui {
    /// Choices made in the window but not yet applied. Text fields are read
    /// into it on demand; toggles and segments write to it directly.
    pending: Settings,
    /// Live settings as they were when the window opened, so Cancel can undo
    /// anything the Test button pushed live.
    snapshot: Settings,
    tested: bool,
    autostart: bool,
    /// Channels in the dropdown, in item order.
    channels: Vec<(ChannelKind, i32)>,
    dpi: i32,
    font: HGDIOBJ,
    bg_brush: HBRUSH,
    field_brush: HBRUSH,
    preview_rect: RECT,
}

impl Ui {
    fn px(&self, v: i32) -> i32 {
        v * self.dpi / 96
    }

    fn s(&self, v: f32) -> f32 {
        v * self.dpi as f32 / 96.0
    }

    fn toggle(&mut self, id: i32) -> Option<&mut bool> {
        Some(match id {
            ID_CHK_AUTOSTART => &mut self.autostart,
            ID_CHK_ACCENT_SYS => &mut self.pending.accent_follow_system,
            ID_CHK_FULLSCREEN => &mut self.pending.hide_in_fullscreen,
            ID_CHK_WATCH => &mut self.pending.watch_all,
            ID_CHK_HOTKEYS => &mut self.pending.hotkeys_enabled,
            ID_CHK_DEBUG => &mut self.pending.debug_log,
            ID_CHK_SCROLL => &mut self.pending.tray_scroll,
            _ => return None,
        })
    }

    fn segment(&self, group: usize) -> usize {
        match group {
            0 => (self.pending.orientation == Orientation::Horizontal) as usize,
            _ => match self.pending.position {
                Position::AboveTray => 0,
                Position::Centre => 1,
                Position::Custom => 2,
            },
        }
    }

    fn set_segment(&mut self, group: usize, choice: usize) {
        match group {
            0 => {
                self.pending.orientation = if choice == 1 {
                    Orientation::Horizontal
                } else {
                    Orientation::Vertical
                }
            }
            _ => {
                self.pending.position = match choice {
                    1 => Position::Centre,
                    2 => Position::Custom,
                    _ => Position::AboveTray,
                }
            }
        }
    }

    /// Colour the UI should draw with right now, honouring follow-system.
    fn accent(&self) -> (u8, u8, u8) {
        self.pending.effective_accent()
    }
}

unsafe fn ui<'a>(hwnd: HWND) -> Option<&'a mut Ui> {
    (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Ui).as_mut()
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

/// How the message loop should treat a message while settings are open.
pub enum KeyRoute {
    /// Fully handled here; don't dispatch it.
    Handled,
    /// Dispatch straight to the control, bypassing dialog navigation, so a
    /// shortcut picker can record Enter instead of it pressing Save.
    Raw,
    /// Normal dialog handling (Tab, Enter, Esc).
    Dialog,
}

pub fn route_key(settings: HWND, msg: &MSG) -> KeyRoute {
    unsafe {
        if msg.message != WM_KEYDOWN && msg.message != WM_SYSKEYDOWN {
            return KeyRoute::Dialog;
        }
        let focus = GetFocus();
        if focus.is_null() || GetParent(focus) != settings {
            return KeyRoute::Dialog;
        }
        if !matches!(GetDlgCtrlID(focus), ID_HK_UP | ID_HK_DOWN | ID_HK_MUTE) {
            return KeyRoute::Dialog;
        }
        const VK_BACK: usize = 0x08;
        const VK_RETURN: usize = 0x0D;
        const VK_DELETE: usize = 0x2E;
        match msg.wParam {
            // Backspace or Delete on its own unassigns the shortcut.
            VK_BACK | VK_DELETE if msg.message == WM_KEYDOWN && !modifier_down() => {
                SendMessageW(focus, HKM_SETHOTKEY, 0, 0);
                KeyRoute::Handled
            }
            VK_RETURN => KeyRoute::Raw,
            _ => KeyRoute::Dialog,
        }
    }
}

unsafe fn modifier_down() -> bool {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetKeyState, VK_CONTROL, VK_MENU, VK_SHIFT,
    };
    [VK_CONTROL, VK_MENU, VK_SHIFT]
        .iter()
        .any(|vk| GetKeyState(*vk as i32) < 0)
}

/// A channel was picked from the tray menu while this window is open: show
/// it, so saving the window later doesn't switch back.
pub fn channel_chosen(hwnd: HWND, kind: ChannelKind, index: i32) {
    unsafe {
        if hwnd.is_null() {
            return;
        }
        let Some(ui) = ui(hwnd) else { return };
        for s in [&mut ui.pending, &mut ui.snapshot] {
            s.kind = kind;
            s.index = index;
        }
        if let Some(item) = ui.channels.iter().position(|c| *c == (kind, index)) {
            SendMessageW(GetDlgItem(hwnd, ID_COMBO_CHANNEL), CB_SETCURSEL, item, 0);
        }
    }
}

/// The bar was dragged while this window is open: adopt the new spot so a
/// later Save or Cancel doesn't put the bar back where it was.
pub fn bar_dragged(hwnd: HWND, x: i32, y: i32) {
    unsafe {
        if hwnd.is_null() {
            return;
        }
        let Some(ui) = ui(hwnd) else { return };
        for s in [&mut ui.pending, &mut ui.snapshot] {
            s.position = Position::Custom;
            s.custom_x = x;
            s.custom_y = y;
        }
        invalidate_segments(hwnd);
        let preview = ui.preview_rect;
        InvalidateRect(hwnd, &preview, 0);
    }
}

unsafe fn themed(hwnd: HWND, theme: &str) {
    SetWindowTheme(hwnd, wide(theme).as_ptr(), null_mut());
}

unsafe fn label(parent: HWND, ui: &Ui, text: &str, x: i32, y: i32, w: i32, id: i32) -> HWND {
    let h = CreateWindowExW(
        0,
        wide("STATIC").as_ptr(),
        wide(text).as_ptr(),
        WS_CHILD | WS_VISIBLE | SS_LEFT,
        ui.px(x),
        ui.px(y),
        ui.px(w),
        ui.px(20),
        parent,
        id as isize as *mut _,
        null_mut(),
        null_mut(),
    );
    SendMessageW(h, WM_SETFONT, ui.font as WPARAM, 1);
    h
}

#[allow(clippy::too_many_arguments)]
unsafe fn control(
    parent: HWND,
    ui: &Ui,
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
        ui.px(x),
        ui.px(y),
        ui.px(w),
        ui.px(h),
        parent,
        id as isize as *mut _,
        null_mut(),
        null_mut(),
    );
    SendMessageW(hwnd, WM_SETFONT, ui.font as WPARAM, 1);
    hwnd
}

/// A row of pill buttons acting as one mutually-exclusive choice.
#[allow(clippy::too_many_arguments)]
unsafe fn segmented(
    parent: HWND,
    ui: &Ui,
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
            ui,
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

/// One column of label + control rows.
struct Column {
    x: i32,
    label_w: i32,
    control_w: i32,
}

impl Column {
    fn cx(&self) -> i32 {
        self.x + self.label_w
    }
}

/// Dropdown text for a channel: Voicemeeter's name plus the user's label.
pub unsafe fn channel_caption(kind: ChannelKind, index: i32, edition: i32) -> String {
    let name = Settings::channel_name(kind, index, edition);
    let app = state();
    let custom = if app.is_null() {
        None
    } else {
        (*app).vmr.get_string(&Settings::label_param(kind, index))
    };
    match custom {
        Some(text) if text != name => format!("{name}  —  {text}"),
        _ => name,
    }
}

/// Creates every child control for the current DPI and returns the client
/// height the layout needs.
unsafe fn build_controls(hwnd: HWND, ui: &mut Ui) -> i32 {
    ui.font = CreateFontW(
        ui.px(16),
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

    let s = ui.pending.clone();
    let row = 32;
    let field_h = 26;
    let section_gap = 42;
    let mut section = ID_SECTION_FIRST;
    let top = 92;

    // ================= Left column: channel, appearance, preview ==========
    let left = Column {
        x: 24,
        label_w: 162,
        control_w: 248,
    };
    let mut y = top;
    label(hwnd, ui, "CHANNEL", left.x, y, 200, section);
    section += 1;
    y += 26;

    let combo = control(
        hwnd,
        ui,
        "COMBOBOX",
        "",
        CBS_DROPDOWNLIST as u32 | WS_VSCROLL,
        left.x,
        y,
        left.label_w + left.control_w,
        320,
        ID_COMBO_CHANNEL,
    );
    themed(combo, "DarkMode_CFD");
    let edition = if state().is_null() {
        1
    } else {
        (*state()).edition
    };
    ui.channels = config::all_channels(edition);
    // A configured channel this edition lacks stays selectable, flagged,
    // rather than silently turning into a different one.
    if !ui.channels.contains(&(s.kind, s.index)) {
        ui.channels.push((s.kind, s.index));
    }
    let mut selected = 0;
    for (item, &(kind, index)) in ui.channels.iter().enumerate() {
        let mut caption = channel_caption(kind, index, edition);
        if item >= config::strip_count(edition) as usize + config::bus_count(edition) as usize {
            caption.push_str("  (not in this edition)");
        }
        SendMessageW(combo, CB_ADDSTRING, 0, wide(&caption).as_ptr() as LPARAM);
        if (kind, index) == (s.kind, s.index) {
            selected = item;
        }
    }
    SendMessageW(combo, CB_SETCURSEL, selected, 0);

    y += section_gap;
    label(hwnd, ui, "APPEARANCE", left.x, y, 200, section);
    section += 1;
    y += 26;

    label(hwnd, ui, "Orientation", left.x, y + 5, left.label_w, 0);
    segmented(
        hwnd,
        ui,
        &["Vertical", "Horizontal"],
        ID_SEG_ORIENT,
        left.cx(),
        y,
        left.control_w,
        30,
    );

    y += 38;
    label(hwnd, ui, "Position", left.x, y + 5, left.label_w, 0);
    segmented(
        hwnd,
        ui,
        &["Above tray", "Floating", "Custom"],
        ID_SEG_POSITION,
        left.cx(),
        y,
        left.control_w,
        30,
    );

    y += 38;
    label(hwnd, ui, "Accent color", left.x, y + 3, left.label_w, 0);
    control(
        hwnd,
        ui,
        "BUTTON",
        "Choose color",
        BS_OWNERDRAW as u32,
        left.cx(),
        y,
        left.control_w,
        field_h,
        ID_BTN_COLOR,
    );

    y += row;
    label(
        hwnd,
        ui,
        "Match Windows accent",
        left.x,
        y + 5,
        left.label_w,
        0,
    );
    toggle_control(
        hwnd,
        ui,
        "Match Windows accent",
        left.cx(),
        y,
        ID_CHK_ACCENT_SYS,
    );

    y += row;
    label(hwnd, ui, "Opacity (%)", left.x, y + 3, left.label_w, 0);
    edit(
        hwnd,
        ui,
        &s.opacity_pct.to_string(),
        true,
        left.cx(),
        y,
        left.control_w,
        ID_EDIT_OPACITY,
    );

    y += section_gap;
    label(hwnd, ui, "PREVIEW", left.x, y, 200, section);
    section += 1;
    y += 24;
    let preview_h = 124;
    ui.preview_rect = RECT {
        left: ui.px(left.x),
        top: ui.px(y),
        right: ui.px(left.x + left.label_w + left.control_w),
        bottom: ui.px(y + preview_h),
    };
    let left_bottom = y + preview_h;

    // ================= Right column: behavior, hotkeys =====================
    let right = Column {
        x: 472,
        label_w: 196,
        control_w: 204,
    };
    let mut y = top;
    label(hwnd, ui, "BEHAVIOR", right.x, y, 200, section);
    section += 1;
    y += 26;

    label(
        hwnd,
        ui,
        "Hide after (ms)",
        right.x,
        y + 3,
        right.label_w,
        0,
    );
    edit(
        hwnd,
        ui,
        &s.hide_ms.to_string(),
        true,
        right.cx(),
        y,
        right.control_w,
        ID_EDIT_HIDE,
    );

    y += row;
    label(
        hwnd,
        ui,
        "Fader range (dB)",
        right.x,
        y + 3,
        right.label_w,
        0,
    );
    let half = (right.control_w - 12) / 2;
    edit(
        hwnd,
        ui,
        &s.min_db.to_string(),
        false,
        right.cx(),
        y,
        half,
        ID_EDIT_MINDB,
    );
    edit(
        hwnd,
        ui,
        &s.max_db.to_string(),
        false,
        right.cx() + half + 12,
        y,
        half,
        ID_EDIT_MAXDB,
    );

    y += row;
    label(
        hwnd,
        ui,
        "Step per notch (dB)",
        right.x,
        y + 3,
        right.label_w,
        0,
    );
    edit(
        hwnd,
        ui,
        &s.step_db.to_string(),
        false,
        right.cx(),
        y,
        right.control_w,
        ID_EDIT_STEP,
    );

    for (text, id) in [
        ("Hide during fullscreen", ID_CHK_FULLSCREEN),
        ("Follow any channel that moves", ID_CHK_WATCH),
        ("Scroll tray icon for volume", ID_CHK_SCROLL),
        ("Start with Windows", ID_CHK_AUTOSTART),
        ("Debug log", ID_CHK_DEBUG),
    ] {
        y += row;
        label(hwnd, ui, text, right.x, y + 5, right.label_w, 0);
        toggle_control(hwnd, ui, text, right.cx(), y, id);
    }

    y += section_gap;
    label(hwnd, ui, "HOTKEYS", right.x, y, 200, section);
    y += 26;

    label(
        hwnd,
        ui,
        "Enable global hotkeys",
        right.x,
        y + 5,
        right.label_w,
        0,
    );
    toggle_control(
        hwnd,
        ui,
        "Enable global hotkeys",
        right.cx(),
        y,
        ID_CHK_HOTKEYS,
    );

    for (text, id, key) in [
        ("Volume up", ID_HK_UP, s.hotkey_up),
        ("Volume down", ID_HK_DOWN, s.hotkey_down),
        ("Mute", ID_HK_MUTE, s.hotkey_mute),
    ] {
        y += row;
        label(hwnd, ui, text, right.x, y + 3, right.label_w, 0);
        let hk = control(
            hwnd,
            ui,
            HOTKEY_CLASS,
            "",
            0,
            right.cx(),
            y,
            right.control_w,
            field_h,
            id,
        );
        // A bare key would hijack normal typing; promote it to Ctrl+Alt.
        SendMessageW(
            hk,
            HKM_SETRULES,
            HKCOMB_NONE | HKCOMB_S,
            HOTKEYF_CONTROL_ALT,
        );
        SendMessageW(hk, HKM_SETHOTKEY, key.to_control() as WPARAM, 0);
    }
    let right_bottom = y + field_h;

    // ================= Footer ==============================================
    let y = left_bottom.max(right_bottom) + 24;
    let footer_right = right.x + right.label_w + right.control_w;
    for (text, id, x) in [
        ("Reset", ID_BTN_RESET, left.x),
        ("Test", ID_BTN_TEST, left.x + 104),
        ("Cancel", ID_BTN_CANCEL, footer_right - 200),
        ("Save", ID_BTN_SAVE, footer_right - 96),
    ] {
        control(
            hwnd,
            ui,
            "BUTTON",
            text,
            BS_OWNERDRAW as u32,
            x,
            y,
            96,
            34,
            id,
        );
    }

    sync_toggle_checks(hwnd, ui);
    ui.px(y + 34 + 20)
}

#[allow(clippy::too_many_arguments)]
unsafe fn edit(parent: HWND, ui: &Ui, text: &str, numeric: bool, x: i32, y: i32, w: i32, id: i32) {
    let style = if numeric {
        ES_NUMBER | ES_AUTOHSCROLL
    } else {
        ES_AUTOHSCROLL
    };
    let h = control(parent, ui, "EDIT", text, style as u32, x, y, w, 26, id);
    themed(h, "DarkMode_CFD");
}

/// A switch. It's a real checkbox underneath, painted over in custom draw,
/// so screen readers announce it as a checkbox with its on/off state.
/// `name` isn't drawn (the label beside it is), but it's what they read out.
unsafe fn toggle_control(parent: HWND, ui: &Ui, name: &str, x: i32, y: i32, id: i32) {
    control(
        parent,
        ui,
        "BUTTON",
        name,
        BS_CHECKBOX as u32,
        x,
        y,
        46,
        26,
        id,
    );
}

/// Sizes the window to fit `client_h` at the current DPI, optionally moving
/// its top-left corner.
unsafe fn fit_window(hwnd: HWND, ui: &Ui, client_h: i32, origin: Option<(i32, i32)>) {
    let mut rc = RECT {
        left: 0,
        top: 0,
        right: ui.px(CLIENT_W),
        bottom: client_h,
    };
    AdjustWindowRectExForDpi(&mut rc, WINDOW_STYLE, 0, 0, ui.dpi as u32);
    let (x, y, flags) = match origin {
        Some((x, y)) => (x, y, SWP_NOZORDER | SWP_NOACTIVATE),
        None => (0, 0, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE),
    };
    SetWindowPos(
        hwnd,
        null_mut(),
        x,
        y,
        rc.right - rc.left,
        rc.bottom - rc.top,
        flags,
    );
}

unsafe fn destroy_controls(hwnd: HWND, ui: &mut Ui) {
    let mut child = GetWindow(hwnd, GW_CHILD);
    while !child.is_null() {
        let next = GetWindow(child, GW_HWNDNEXT);
        DestroyWindow(child);
        child = next;
    }
    if !ui.font.is_null() {
        DeleteObject(ui.font);
        ui.font = null_mut();
    }
}

/// Mirrors the pending toggle values into the checkboxes' checked state,
/// which is what accessibility tools read.
unsafe fn sync_toggle_checks(hwnd: HWND, ui: &mut Ui) {
    for id in TOGGLE_IDS {
        if let Some(on) = ui.toggle(id).map(|flag| *flag) {
            let state = if on { BST_CHECKED } else { BST_UNCHECKED };
            SendMessageW(GetDlgItem(hwnd, id), BM_SETCHECK, state as WPARAM, 0);
        }
    }
}

unsafe fn invalidate_segments(hwnd: HWND) {
    for (base, count) in SEG_GROUPS {
        for i in 0..count as i32 {
            InvalidateRect(GetDlgItem(hwnd, base + i), null_mut(), 0);
        }
    }
}

/// Pushes a Settings back into the live controls (used by Reset).
unsafe fn repopulate(hwnd: HWND, ui: &mut Ui, s: &Settings) {
    // Reset shouldn't forget where the bar was dragged to.
    let (cx, cy) = (ui.pending.custom_x, ui.pending.custom_y);
    ui.pending = s.clone();
    ui.pending.custom_x = cx;
    ui.pending.custom_y = cy;
    for (id, text) in [
        (ID_EDIT_OPACITY, s.opacity_pct.to_string()),
        (ID_EDIT_HIDE, s.hide_ms.to_string()),
        (ID_EDIT_MINDB, s.min_db.to_string()),
        (ID_EDIT_MAXDB, s.max_db.to_string()),
        (ID_EDIT_STEP, s.step_db.to_string()),
    ] {
        SetWindowTextW(GetDlgItem(hwnd, id), wide(&text).as_ptr());
    }
    for (id, key) in [
        (ID_HK_UP, s.hotkey_up),
        (ID_HK_DOWN, s.hotkey_down),
        (ID_HK_MUTE, s.hotkey_mute),
    ] {
        SendMessageW(
            GetDlgItem(hwnd, id),
            HKM_SETHOTKEY,
            key.to_control() as WPARAM,
            0,
        );
    }
    if let Some(item) = ui.channels.iter().position(|c| *c == (s.kind, s.index)) {
        SendMessageW(GetDlgItem(hwnd, ID_COMBO_CHANNEL), CB_SETCURSEL, item, 0);
    }
    sync_toggle_checks(hwnd, ui);
    InvalidateRect(hwnd, null_mut(), 1);
    let mut child = GetWindow(hwnd, GW_CHILD);
    while !child.is_null() {
        InvalidateRect(child, null_mut(), 0);
        child = GetWindow(child, GW_HWNDNEXT);
    }
}

unsafe fn get_text(hwnd: HWND, id: i32) -> String {
    let mut buf = [0u16; 64];
    let len = GetWindowTextW(GetDlgItem(hwnd, id), buf.as_mut_ptr(), buf.len() as i32);
    String::from_utf16_lossy(&buf[..len.max(0) as usize])
        .trim()
        .to_string()
}

/// A field that didn't parse: which control, and what to tell the user.
struct FieldError {
    id: i32,
    message: &'static str,
}

/// Reads the text fields, dropdown and hotkey pickers into `ui.pending`.
/// With `strict`, the first unparseable field is reported instead of being
/// skipped, so Save and Test can refuse bad input rather than ignore it.
unsafe fn read_controls(hwnd: HWND, ui: &mut Ui, strict: bool) -> Result<(), FieldError> {
    let item = SendMessageW(GetDlgItem(hwnd, ID_COMBO_CHANNEL), CB_GETCURSEL, 0, 0);
    if let Some(&(kind, index)) = usize::try_from(item).ok().and_then(|i| ui.channels.get(i)) {
        ui.pending.kind = kind;
        ui.pending.index = index;
    }

    let s = &mut ui.pending;
    macro_rules! field {
        ($id:expr, $ty:ty, $target:expr, $message:literal) => {
            match get_text(hwnd, $id).parse::<$ty>() {
                Ok(v) if (v as f64).is_finite() => $target = v as _,
                _ if strict => {
                    return Err(FieldError {
                        id: $id,
                        message: $message,
                    })
                }
                _ => {}
            }
        };
    }
    // Parsed wide, then clamped, so "300" becomes 100 rather than wrapping.
    let mut opacity = s.opacity_pct as u32;
    field!(
        ID_EDIT_OPACITY,
        u32,
        opacity,
        "Opacity must be a whole number from 20 to 100."
    );
    s.opacity_pct = opacity.min(100) as u8;
    field!(
        ID_EDIT_HIDE,
        u32,
        s.hide_ms,
        "Hide after must be a whole number of milliseconds (200 to 10000)."
    );
    field!(
        ID_EDIT_MINDB,
        f32,
        s.min_db,
        "The fader range minimum must be a number of dB, e.g. -60."
    );
    field!(
        ID_EDIT_MAXDB,
        f32,
        s.max_db,
        "The fader range maximum must be a number of dB, e.g. 12."
    );
    field!(
        ID_EDIT_STEP,
        f32,
        s.step_db,
        "Step per notch must be a number of dB, e.g. 1 or 0.5."
    );

    for (id, slot) in [
        (ID_HK_UP, &mut s.hotkey_up),
        (ID_HK_DOWN, &mut s.hotkey_down),
        (ID_HK_MUTE, &mut s.hotkey_mute),
    ] {
        let word = SendMessageW(GetDlgItem(hwnd, id), HKM_GETHOTKEY, 0, 0);
        *slot = Hotkey::from_control(word as u32);
    }
    Ok(())
}

/// Validates and clamps the fields. On error, explains and focuses the field.
/// Clamped values are written back so the user sees what was used.
unsafe fn commit_fields(hwnd: HWND, ui: &mut Ui) -> bool {
    if let Err(e) = read_controls(hwnd, ui, true) {
        MessageBoxW(
            hwnd,
            wide(e.message).as_ptr(),
            wide("Voicemeeter OSD").as_ptr(),
            MB_OK | MB_ICONWARNING,
        );
        let field = GetDlgItem(hwnd, e.id);
        SetFocus(field);
        SendMessageW(field, EM_SETSEL, 0, -1);
        return false;
    }
    let before = ui.pending.clone();
    ui.pending.normalize();
    let s = &ui.pending;
    for (id, changed, text) in [
        (
            ID_EDIT_OPACITY,
            before.opacity_pct != s.opacity_pct,
            s.opacity_pct.to_string(),
        ),
        (
            ID_EDIT_HIDE,
            before.hide_ms != s.hide_ms,
            s.hide_ms.to_string(),
        ),
        (
            ID_EDIT_MINDB,
            before.min_db != s.min_db,
            s.min_db.to_string(),
        ),
        (
            ID_EDIT_MAXDB,
            before.max_db != s.max_db,
            s.max_db.to_string(),
        ),
        (
            ID_EDIT_STEP,
            before.step_db != s.step_db,
            s.step_db.to_string(),
        ),
    ] {
        if changed {
            SetWindowTextW(GetDlgItem(hwnd, id), wide(&text).as_ptr());
        }
    }
    true
}

/// Tells the user which shortcuts another app already owns.
unsafe fn warn_taken(hwnd: HWND, taken: &[String]) {
    if taken.is_empty() {
        return;
    }
    let text = format!(
        "These shortcuts are already in use by another app, so they won't work here:\n\n{}\n\nPick different keys, or close the app that's using them.",
        taken.join("\n")
    );
    MessageBoxW(
        hwnd,
        wide(&text).as_ptr(),
        wide("Voicemeeter OSD").as_ptr(),
        MB_OK | MB_ICONWARNING,
    );
}

/// Makes `new_settings` the live configuration. Returns any hotkeys that
/// couldn't be registered.
unsafe fn apply_live(mut new_settings: Settings) -> Vec<String> {
    let app = state();
    if app.is_null() {
        return Vec::new();
    }
    let app = &mut *app;
    // Picking Custom without ever having dragged needs somewhere to land:
    // seed it from the floating spot on the current monitor.
    if new_settings.position == Position::Custom
        && new_settings.custom_x == 0
        && new_settings.custom_y == 0
    {
        let mut probe = new_settings.clone();
        probe.position = Position::Centre;
        let monitor = osd::resolve_monitor(&probe, None);
        let (x, y) = osd::origin_for(&probe, monitor.dpi, &monitor, None);
        new_settings.custom_x = x;
        new_settings.custom_y = y;
    }
    app.settings = new_settings;
    app.sync_watch_targets();
    // Always come back to the configured channel; the range may also have
    // changed, which moves the percentage even for the same channel.
    app.set_watch(app.settings.kind, app.settings.index);
    let taken = crate::sync_input(app);
    crate::log::set_enabled(app.settings.debug_log);
    crate::relayout_osd();
    taken
}

unsafe fn paint(hwnd: HWND, ui: &mut Ui) {
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
    FillRect(mem_dc, &client, ui.bg_brush);

    {
        let canvas = Canvas::from_hdc(mem_dc);

        // Header: app icon, title, subtitle.
        let app = state();
        if !app.is_null() {
            let icon = (*app).icon_big;
            if !icon.is_null() {
                DrawIconEx(
                    mem_dc,
                    ui.px(24),
                    ui.px(20),
                    icon,
                    ui.px(38),
                    ui.px(38),
                    0,
                    null_mut(),
                    DI_NORMAL,
                );
            }
        }
        canvas.draw_text(
            "Voicemeeter OSD",
            ui.s(74.0),
            ui.s(22.0),
            ui.s(300.0),
            ui.s(22.0),
            ui.s(17.0),
            tint(TEXT, 255),
            ALIGN_NEAR,
            true,
        );
        canvas.draw_text(
            "On-screen volume display",
            ui.s(74.0),
            ui.s(42.0),
            ui.s(300.0),
            ui.s(18.0),
            ui.s(12.5),
            tint(TEXT_DIM, 255),
            ALIGN_NEAR,
            false,
        );
        canvas.fill_round_rect(
            0.0,
            ui.s(76.0),
            w as f32,
            ui.s(1.0),
            0.0,
            tint(DIVIDER, 255),
        );
        // Column divider.
        canvas.fill_round_rect(
            ui.s(448.0),
            ui.s(96.0),
            ui.s(1.0),
            (ui.preview_rect.bottom as f32 - ui.s(96.0)).max(0.0),
            0.0,
            tint(DIVIDER, 255),
        );

        // Preview panel with a live miniature of the bar.
        // A lighter "desktop" backdrop so the dark OSD card reads against it.
        let pr = ui.preview_rect;
        canvas.fill_round_rect_gradient(
            pr.left as f32,
            pr.top as f32,
            (pr.right - pr.left) as f32,
            (pr.bottom - pr.top) as f32,
            ui.s(10.0),
            argb(255, 58, 60, 74),
            argb(255, 34, 35, 44),
        );
        canvas.stroke_round_rect(
            pr.left as f32 + 0.5,
            pr.top as f32 + 0.5,
            (pr.right - pr.left) as f32 - 1.0,
            (pr.bottom - pr.top) as f32 - 1.0,
            ui.s(10.0),
            tint(DIVIDER, 255),
            1.0,
        );

        // Lenient: a half-typed field shouldn't blank the preview.
        let _ = read_controls(hwnd, ui, false);
        let mut settings = ui.pending.clone();
        settings.normalize();
        let panel_cx = (pr.left + pr.right) as f32 / 2.0;
        let panel_cy = (pr.top + pr.bottom) as f32 / 2.0;
        let (pw, ph) = if settings.orientation == Orientation::Vertical {
            (ui.s(54.0), ui.s(104.0))
        } else {
            (ui.s(252.0), ui.s(62.0))
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

/// Whether an owner-drawn control should show its focus outline: it has
/// focus, and keyboard cues aren't hidden (ODS_NOFOCUSRECT), like native
/// controls that only show focus once the keyboard is used.
unsafe fn owner_focus_visible(dis: *const DRAWITEMSTRUCT) -> bool {
    let state = (*dis).itemState;
    state & ODS_FOCUS != 0 && state & ODS_NOFOCUSRECT == 0
}

/// Outline for the control that has keyboard focus. Custom-drawn controls
/// get no focus indication from Windows, so without this, tabbing through
/// the window shows nothing.
#[allow(clippy::too_many_arguments)]
unsafe fn draw_focus(
    ui: &Ui,
    canvas: &Canvas,
    visible: bool,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    radius: f32,
) {
    if !visible {
        return;
    }
    let inset = ui.s(1.0);
    canvas.stroke_round_rect(
        x + inset,
        y + inset,
        w - inset * 2.0,
        h - inset * 2.0,
        (radius - inset).max(0.0),
        argb(230, 255, 255, 255),
        ui.s(1.5),
    );
}

unsafe fn draw_segment(ui: &Ui, dis: *const DRAWITEMSTRUCT, selected: bool) {
    let rc = (*dis).rcItem;
    let w = (rc.right - rc.left) as f32;
    let h = (rc.bottom - rc.top) as f32;
    let canvas = Canvas::from_hdc((*dis).hDC);

    FillRect((*dis).hDC, &rc, ui.bg_brush);

    let (fill, text_color) = if selected {
        (tint(ui.accent(), 255), argb(255, 255, 255, 255))
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
        ui.s(13.5),
        text_color,
        ALIGN_CENTER,
        selected,
    );
    draw_focus(
        ui,
        &canvas,
        owner_focus_visible(dis),
        0.0,
        0.0,
        w,
        h,
        h / 2.0,
    );
}

/// Paints a switch over a checkbox during its custom draw.
unsafe fn draw_toggle(ui: &Ui, hdc: HDC, rc: RECT, on: bool, focus_visible: bool) {
    let w = (rc.right - rc.left) as f32;
    let h = (rc.bottom - rc.top) as f32;
    let canvas = Canvas::from_hdc(hdc);

    FillRect(hdc, &rc, ui.bg_brush);

    let track_h = h * 0.78;
    let track_y = (h - track_h) / 2.0;
    let fill = if on {
        tint(ui.accent(), 255)
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

    let inset = ui.s(3.0);
    let knob = track_h - inset * 2.0;
    let knob_x = if on { w - knob - inset } else { inset };
    canvas.fill_ellipse(
        knob_x,
        track_y + inset,
        knob,
        knob,
        argb(255, 255, 255, 255),
    );
    draw_focus(
        ui,
        &canvas,
        focus_visible,
        0.0,
        track_y,
        w,
        track_h,
        track_h / 2.0,
    );
}

unsafe fn draw_button(ui: &Ui, dis: *const DRAWITEMSTRUCT) {
    let rc = (*dis).rcItem;
    let id = (*dis).CtlID as i32;
    let pressed = (*dis).itemState & ODS_SELECTED != 0;
    let w = (rc.right - rc.left) as f32;
    let h = (rc.bottom - rc.top) as f32;
    let radius = ui.s(6.0);

    let canvas = Canvas::from_hdc((*dis).hDC);
    let primary = id == ID_BTN_SAVE;

    let accent = ui.accent();
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
        let chip = h - ui.s(10.0);
        canvas.fill_round_rect(
            ui.s(8.0),
            ui.s(5.0),
            chip * 1.6,
            chip,
            ui.s(4.0),
            tint(accent, 255),
        );
        canvas.draw_text(
            &label,
            chip * 1.6 + ui.s(18.0),
            0.0,
            w,
            h,
            ui.s(14.0),
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
            ui.s(14.0),
            text_color,
            ALIGN_CENTER,
            primary,
        );
    }
    draw_focus(
        ui,
        &canvas,
        owner_focus_visible(dis),
        0.0,
        0.0,
        w,
        h,
        radius,
    );
}

unsafe fn on_command(hwnd: HWND, ui: &mut Ui, id: i32, notification: u32) {
    match id {
        ID_BTN_COLOR if !ui.pending.accent_follow_system => {
            let mut custom = [0u32; 16];
            let mut cc: CHOOSECOLORW = std::mem::zeroed();
            cc.lStructSize = std::mem::size_of::<CHOOSECOLORW>() as u32;
            cc.hwndOwner = hwnd;
            let a = ui.pending.accent;
            cc.rgbResult = rgb(a.0, a.1, a.2);
            cc.lpCustColors = custom.as_mut_ptr();
            cc.Flags = CC_FULLOPEN | CC_RGBINIT;
            if ChooseColorW(&mut cc) != 0 {
                let c = cc.rgbResult;
                ui.pending.accent = (c as u8, (c >> 8) as u8, (c >> 16) as u8);
                InvalidateRect(hwnd, null_mut(), 0);
                invalidate_all_children(hwnd);
            }
        }
        ID_BTN_TEST => {
            // Test previews live but commits nothing: no file write and no
            // autostart change, and Cancel rolls it back.
            if commit_fields(hwnd, ui) {
                let taken = apply_live(ui.pending.clone());
                ui.tested = true;
                crate::show_preview();
                warn_taken(hwnd, &taken);
            }
        }
        ID_BTN_SAVE | ID_OK => {
            if !commit_fields(hwnd, ui) {
                return;
            }
            // Still saves: the rest of the settings are fine, and the keys
            // may free up once the other app closes.
            let taken = apply_live(ui.pending.clone());
            warn_taken(hwnd, &taken);
            let app = state();
            if !app.is_null() {
                (*app).settings.save();
            }
            if ui.autostart != is_autostart_enabled() {
                set_autostart(ui.autostart);
            }
            ui.tested = false;
            DestroyWindow(hwnd);
        }
        ID_BTN_CANCEL | ID_CANCEL => {
            DestroyWindow(hwnd);
        }
        ID_BTN_RESET => {
            repopulate(hwnd, ui, &Settings::default());
        }
        _ if ui.toggle(id).is_some() => {
            if let Some(flag) = ui.toggle(id) {
                *flag = !*flag;
            }
            sync_toggle_checks(hwnd, ui);
            if id == ID_CHK_ACCENT_SYS {
                // Every accent-coloured control changes with this one.
                InvalidateRect(hwnd, null_mut(), 0);
                invalidate_all_children(hwnd);
            } else {
                InvalidateRect(GetDlgItem(hwnd, id), null_mut(), 0);
            }
        }
        _ if (ID_SEG_ORIENT..ID_SEG_END).contains(&id) => {
            if let Some((group, base, _)) = seg_group_of(id) {
                ui.set_segment(group, (id - base) as usize);
                invalidate_segments(hwnd);
                let preview = ui.preview_rect;
                InvalidateRect(hwnd, &preview, 0);
            }
        }
        _ => {
            // Live-update the preview as fields change.
            if notification == EN_CHANGE || notification == CBN_SELCHANGE {
                let preview = ui.preview_rect;
                InvalidateRect(hwnd, &preview, 0);
            }
        }
    }
}

unsafe fn invalidate_all_children(hwnd: HWND) {
    let mut child = GetWindow(hwnd, GW_CHILD);
    while !child.is_null() {
        InvalidateRect(child, null_mut(), 0);
        child = GetWindow(child, GW_HWNDNEXT);
    }
}

pub unsafe extern "system" fn wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_CREATE {
        let live = {
            let app = state();
            if app.is_null() {
                Settings::default()
            } else {
                (*app).settings.clone()
            }
        };
        let ui = Box::into_raw(Box::new(Ui {
            pending: live.clone(),
            snapshot: live,
            tested: false,
            autostart: is_autostart_enabled(),
            channels: Vec::new(),
            dpi: GetDpiForWindow(hwnd).max(96) as i32,
            font: null_mut(),
            bg_brush: CreateSolidBrush(rgb(BG.0, BG.1, BG.2)),
            field_brush: CreateSolidBrush(rgb(FIELD.0, FIELD.1, FIELD.2)),
            preview_rect: RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            },
        }));
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, ui as isize);
        let height = build_controls(hwnd, &mut *ui);
        fit_window(hwnd, &*ui, height, None);
        return 0;
    }

    if msg == WM_NCDESTROY {
        let ui = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Ui;
        if !ui.is_null() {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            let ui = Box::from_raw(ui);
            if !ui.font.is_null() {
                DeleteObject(ui.font);
            }
            DeleteObject(ui.bg_brush as HGDIOBJ);
            DeleteObject(ui.field_brush as HGDIOBJ);
        }
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }

    let Some(ui) = ui(hwnd) else {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    };

    match msg {
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            paint(hwnd, ui);
            0
        }
        WM_DPICHANGED => {
            // Moved to a display with different scaling: rebuild the layout
            // at the new size, keeping whatever has been typed so far.
            let _ = read_controls(hwnd, ui, false);
            destroy_controls(hwnd, ui);
            ui.dpi = ((wparam >> 16) & 0xFFFF).max(96) as i32;
            let height = build_controls(hwnd, ui);
            let suggested = &*(lparam as *const RECT);
            fit_window(hwnd, ui, height, Some((suggested.left, suggested.top)));
            InvalidateRect(hwnd, null_mut(), 1);
            0
        }
        WM_CTLCOLORSTATIC | WM_CTLCOLORBTN => {
            let hdc = wparam as HDC;
            let id = GetDlgCtrlID(lparam as HWND);
            let color = if (ID_SECTION_FIRST..ID_SECTION_END).contains(&id) {
                TEXT_DIM
            } else {
                TEXT
            };
            SetTextColor(hdc, rgb(color.0, color.1, color.2));
            SetBkColor(hdc, rgb(BG.0, BG.1, BG.2));
            ui.bg_brush as LRESULT
        }
        WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX => {
            let hdc = wparam as HDC;
            SetTextColor(hdc, rgb(TEXT.0, TEXT.1, TEXT.2));
            SetBkColor(hdc, rgb(FIELD.0, FIELD.1, FIELD.2));
            ui.field_brush as LRESULT
        }
        WM_DRAWITEM => {
            let dis = lparam as *const DRAWITEMSTRUCT;
            if dis.is_null() || (*dis).CtlType != ODT_BUTTON {
                return 0;
            }
            let id = (*dis).CtlID as i32;
            if let Some((group, base, _)) = seg_group_of(id) {
                draw_segment(ui, dis, ui.segment(group) == (id - base) as usize);
            } else {
                draw_button(ui, dis);
            }
            1
        }
        WM_NOTIFY => {
            let hdr = lparam as *const NMHDR;
            if hdr.is_null() || (*hdr).code != NM_CUSTOMDRAW {
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            }
            let id = (*hdr).idFrom as i32;
            let Some(on) = ui.toggle(id).map(|flag| *flag) else {
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            };
            let cd = lparam as *const NMCUSTOMDRAW;
            if (*cd).dwDrawStage != CDDS_PREPAINT {
                return CDRF_DODEFAULT as LRESULT;
            }
            let item = (*cd).uItemState;
            let focus_visible = item & CDIS_FOCUS != 0 && item & CDIS_SHOWKEYBOARDCUES != 0;
            draw_toggle(ui, (*cd).hdc, (*cd).rc, on, focus_visible);
            // The checkbox's own box and caption would draw over the switch.
            CDRF_SKIPDEFAULT as LRESULT
        }
        WM_COMMAND => {
            let id = (wparam & 0xFFFF) as i32;
            let notification = ((wparam >> 16) & 0xFFFF) as u32;
            on_command(hwnd, ui, id, notification);
            0
        }
        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            // Closing without saving undoes anything Test pushed live.
            if ui.tested {
                // These were working when the window opened, so a conflict
                // here isn't worth interrupting a Cancel for.
                let _ = apply_live(ui.snapshot.clone());
            }
            let app = state();
            if !app.is_null() {
                (*app).hwnd_settings = null_mut();
            }
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
