#![windows_subsystem = "windows"]

mod config;
mod gfx;
mod osd;
mod settings_ui;
mod tray;
mod vmr;

use std::ptr::null_mut;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    GetLastError, COLORREF, ERROR_ALREADY_EXISTS, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use config::{is_autostart_enabled, set_autostart, Settings};
use gfx::{Gdiplus, LayeredSurface};
use vmr::VmrApi;

pub const WM_TRAYICON: u32 = WM_APP + 1;
/// Posted by the mouse hook when the wheel turns over the tray icon.
const WM_TRAY_WHEEL: u32 = WM_APP + 2;
/// Sent when the user changes their Windows accent colour.
const WM_DWMCOLORIZATIONCOLORCHANGED: u32 = 0x0320;
pub const TRAY_ID: u32 = 1;
pub const ID_MENU_SETTINGS: i32 = 2001;
pub const ID_MENU_AUTOSTART: i32 = 2002;
pub const ID_MENU_EXIT: i32 = 2003;

const TIMER_POLL: usize = 1;
const TIMER_ANIM: usize = 2;
const POLL_MS: u32 = 40;
const FRAME_MS: u32 = 16;
const FADE_IN_SECS: f32 = 0.11;
const FADE_OUT_SECS: f32 = 0.22;
const POP_IN_SECS: f32 = 0.26;
const POP_OUT_SECS: f32 = 0.20;

const MAIN_CLASS: &str = "VoicemeeterOsdHostClass";

pub struct AppState {
    pub vmr: VmrApi,
    pub settings: Settings,
    pub hwnd_main: HWND,
    pub hwnd_osd: HWND,
    pub hwnd_settings: HWND,
    pub icon_small: HICON,
    pub icon_big: HICON,
    pub last_gain: Option<f32>,
    pub last_mute: Option<f32>,
    pub percent: i32,
    pub muted: bool,
    pub hide_at: Option<Instant>,
    pub surface: Option<LayeredSurface>,
    pub surface_dpi: i32,
    /// Eased bar level, so the fill glides instead of snapping.
    pub display_pct: f32,
    pub anim_alpha: f32,
    pub animating: bool,
    /// 0 = tucked into the tray icon, 1 = fully popped out.
    pub pop: f32,
    pub tray_point: Option<(i32, i32)>,
    /// Display hosting this showing, fixed for its duration.
    pub monitor: osd::MonitorInfo,
    pub read_failures: u32,
    pub stale_ticks: u32,
    /// "A1", "Strip 2", or the user's own Voicemeeter label.
    pub channel_label: String,
    pub edition: i32,
    tray_ticks: u32,
}

impl AppState {
    /// Re-reads a newly selected channel without animating from a stale level.
    pub fn refresh_channel(&mut self) {
        self.refresh();
        self.display_pct = self.percent as f32;
        self.refresh_label();
    }

    /// Prefers the label the user typed into Voicemeeter over the generic name.
    pub fn refresh_label(&mut self) {
        self.channel_label = self
            .vmr
            .get_string(&self.settings.label_param())
            .unwrap_or_else(|| self.settings.channel_name(self.edition));
    }

    fn tooltip(&self) -> String {
        if self.muted {
            format!("{}  Muted", self.channel_label)
        } else {
            format!(
                "{}  {} ({}%)",
                self.channel_label,
                osd::format_db(self.last_gain),
                self.percent
            )
        }
    }

    /// Nudges the watched channel by `steps` wheel notches.
    fn nudge(&mut self, steps: i32) {
        let Some(current) = self.last_gain else { return };
        let target = (current + steps as f32).clamp(self.settings.min_db, self.settings.max_db);
        self.vmr.set_float(&self.settings.gain_param(), target);
    }

    fn refresh(&mut self) -> bool {
        let gain = self.vmr.get_float(&self.settings.gain_param());
        let mute = self.vmr.get_float(&self.settings.mute_param());
        self.read_failures = u32::from(gain.is_none());
        let mut changed = false;
        if let Some(g) = gain {
            if self.last_gain != Some(g) {
                changed = true;
            }
            self.last_gain = Some(g);
            let range = (self.settings.max_db - self.settings.min_db).max(1.0);
            let pct = ((g - self.settings.min_db) / range * 100.0).round();
            self.percent = pct.clamp(0.0, 100.0) as i32;
        }
        if let Some(m) = mute {
            if self.last_mute != Some(m) {
                changed = true;
            }
            self.last_mute = Some(m);
            self.muted = m >= 0.5;
        }
        changed
    }
}

static mut STATE: *mut AppState = null_mut();

pub fn state() -> *mut AppState {
    unsafe { STATE }
}

pub fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16)
}

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Cached tray icon bounds, refreshed on a slow cadence so the mouse hook can
/// test containment without an RPC on every wheel event.
static mut TRAY_RECT: RECT = RECT {
    left: 0,
    top: 0,
    right: 0,
    bottom: 0,
};
static mut HOOK_TARGET: HWND = null_mut();

unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && wparam as u32 == WM_MOUSEWHEEL {
        let info = lparam as *const MSLLHOOKSTRUCT;
        if !info.is_null() {
            let pt = (*info).pt;
            let rc = TRAY_RECT;
            let inside =
                pt.x >= rc.left && pt.x < rc.right && pt.y >= rc.top && pt.y < rc.bottom;
            if inside && !HOOK_TARGET.is_null() {
                let delta = ((*info).mouseData >> 16) as i16 as i32;
                PostMessageW(HOOK_TARGET, WM_TRAY_WHEEL, delta as WPARAM, 0);
                return 1; // don't let the scroll fall through to the taskbar
            }
        }
    }
    CallNextHookEx(null_mut(), code, wparam, lparam)
}

unsafe fn show_osd(app: &mut AppState) {
    if app.settings.hide_in_fullscreen && osd::fullscreen_foreground() {
        return;
    }
    app.hide_at = Some(Instant::now() + Duration::from_millis(app.settings.hide_ms as u64));
    if !app.animating {
        // Fresh appearance: pick the display, then pop out of the tray.
        app.animating = true;
        app.tray_point = tray::icon_center(app.hwnd_main);
        app.monitor = osd::resolve_monitor(&app.settings, app.tray_point);
        app.pop = 0.0;
        SetTimer(app.hwnd_main, TIMER_ANIM, FRAME_MS, None);
    }
    osd::render(app);
    ShowWindow(app.hwnd_osd, SW_SHOWNOACTIVATE);
    SetWindowPos(
        app.hwnd_osd,
        HWND_TOPMOST,
        0,
        0,
        0,
        0,
        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
    );
}

pub fn show_preview() {
    unsafe {
        let app = state();
        if app.is_null() {
            return;
        }
        let app = &mut *app;
        app.refresh();
        show_osd(app);
    }
}

/// Re-renders the OSD after a settings change (size, colors, position).
pub fn relayout_osd() {
    unsafe {
        let app = state();
        if app.is_null() {
            return;
        }
        let app = &mut *app;
        app.tray_point = tray::icon_center(app.hwnd_main);
        app.monitor = osd::resolve_monitor(&app.settings, app.tray_point);
        osd::rebuild_surface(app);
        if app.animating {
            osd::render(app);
        }
    }
}

unsafe fn animate(hwnd: HWND, app: &mut AppState) {
    let dt = FRAME_MS as f32 / 1000.0;

    let target = app.percent as f32;
    let diff = target - app.display_pct;
    if diff.abs() < 0.4 {
        app.display_pct = target;
    } else {
        app.display_pct += diff * 0.3;
    }

    let expired = app.hide_at.map(|at| Instant::now() >= at).unwrap_or(true);
    if expired {
        app.anim_alpha -= dt / FADE_OUT_SECS;
        app.pop = (app.pop - dt / POP_OUT_SECS).max(0.0);
        if app.anim_alpha <= 0.0 {
            app.anim_alpha = 0.0;
            app.pop = 0.0;
            ShowWindow(app.hwnd_osd, SW_HIDE);
            KillTimer(hwnd, TIMER_ANIM);
            app.animating = false;
            app.hide_at = None;
            return;
        }
    } else {
        app.anim_alpha = (app.anim_alpha + dt / FADE_IN_SECS).min(1.0);
        app.pop = (app.pop + dt / POP_IN_SECS).min(1.0);
    }

    osd::render(app);
}

unsafe extern "system" fn main_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_TRAYICON => {
            match (lparam & 0xFFFF) as u32 {
                WM_RBUTTONUP => tray::show_menu(hwnd),
                WM_LBUTTONUP | WM_LBUTTONDBLCLK => {
                    settings_ui::open(GetModuleHandleW(null_mut()))
                }
                _ => {}
            }
            0
        }
        WM_COMMAND => {
            match (wparam & 0xFFFF) as i32 {
                ID_MENU_SETTINGS => settings_ui::open(GetModuleHandleW(null_mut())),
                ID_MENU_AUTOSTART => set_autostart(!is_autostart_enabled()),
                ID_MENU_EXIT => {
                    DestroyWindow(hwnd);
                }
                _ => {}
            }
            0
        }
        WM_TIMER => {
            let app = state();
            if app.is_null() {
                return 0;
            }
            let app = &mut *app;
            match wparam {
                TIMER_POLL => {
                    // Pump the cache, but decide off real value changes: the
                    // dirty flag can stop reporting if a previous client died
                    // without logging out.
                    let healthy = app.vmr.pump();
                    if app.refresh() {
                        tray::update_tip(hwnd, app.icon_small, &app.tooltip());
                        show_osd(app);
                    }
                    app.tray_ticks += 1;
                    if app.tray_ticks >= 50 {
                        app.tray_ticks = 0;
                        if let Some(rc) = tray::icon_rect(hwnd) {
                            TRAY_RECT = rc;
                        }
                    }
                    if healthy && app.read_failures == 0 {
                        app.stale_ticks = 0;
                    } else {
                        app.stale_ticks += 1;
                        // ~2s of failed reads means the session is gone.
                        if app.stale_ticks > 50 {
                            app.stale_ticks = 0;
                            app.vmr.relogin();
                        }
                    }
                }
                TIMER_ANIM => animate(hwnd, app),
                _ => {}
            }
            0
        }
        WM_TRAY_WHEEL => {
            let app = state();
            if !app.is_null() {
                let app = &mut *app;
                let notches = (wparam as i32) / 120;
                if notches != 0 {
                    app.nudge(notches);
                    app.vmr.pump();
                    app.refresh();
                    tray::update_tip(hwnd, app.icon_small, &app.tooltip());
                    show_osd(app);
                }
            }
            0
        }
        WM_DWMCOLORIZATIONCOLORCHANGED => {
            let app = state();
            if !app.is_null() && (*app).settings.accent_follow_system {
                relayout_osd();
            }
            0
        }
        WM_DPICHANGED => {
            relayout_osd();
            0
        }
        WM_DESTROY => {
            tray::remove(hwnd);
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe extern "system" fn osd_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_LBUTTONUP => {
            let app = state();
            if !app.is_null() {
                (*app).vmr.show_voicemeeter();
            }
            0
        }
        // Content comes from UpdateLayeredWindow; nothing to paint here.
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn register_class(hinstance: HINSTANCE, name: &str, proc: WNDPROC, icon: HICON) {
    let class_name = wide(name);
    let wc = WNDCLASSW {
        style: 0,
        lpfnWndProc: proc,
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinstance,
        hIcon: icon,
        hCursor: LoadCursorW(null_mut(), IDC_ARROW),
        hbrBackground: null_mut(),
        lpszMenuName: null_mut(),
        lpszClassName: class_name.as_ptr(),
    };
    RegisterClassW(&wc);
}

fn already_running() -> bool {
    unsafe {
        let name = wide("Local\\VoicemeeterOsdSingleInstance");
        let handle = CreateMutexW(null_mut(), 1, name.as_ptr());
        handle.is_null() || GetLastError() == ERROR_ALREADY_EXISTS
    }
}

fn main() {
    if already_running() {
        return;
    }

    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let _gdiplus = Gdiplus::start();

    let vmr = match VmrApi::load() {
        Ok(v) => v,
        Err(e) => {
            fatal(&format!("Could not load the Voicemeeter Remote API.\n\n{e}"));
            return;
        }
    };
    if !vmr.is_logged_in() {
        fatal("Could not connect to Voicemeeter. Make sure Voicemeeter is installed and running.");
        return;
    }

    let settings = Settings::load();

    unsafe {
        let hinstance = GetModuleHandleW(null_mut());
        let icon_small = tray::load_app_icon(hinstance, GetSystemMetrics(SM_CXSMICON));
        let icon_big = tray::load_app_icon(hinstance, GetSystemMetrics(SM_CXICON));

        register_class(hinstance, MAIN_CLASS, Some(main_wndproc), icon_big);
        register_class(hinstance, osd::CLASS_NAME, Some(osd_wndproc), null_mut());
        register_class(
            hinstance,
            settings_ui::CLASS_NAME,
            Some(settings_ui::wndproc),
            icon_big,
        );

        let hwnd_main = CreateWindowExW(
            0,
            wide(MAIN_CLASS).as_ptr(),
            wide("Voicemeeter OSD").as_ptr(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            hinstance,
            null_mut(),
        );

        // Not WS_EX_TRANSPARENT: the bar accepts clicks to raise Voicemeeter.
        let hwnd_osd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            wide(osd::CLASS_NAME).as_ptr(),
            wide("Voicemeeter OSD Bar").as_ptr(),
            WS_POPUP,
            0,
            0,
            10,
            10,
            null_mut(),
            null_mut(),
            hinstance,
            null_mut(),
        );

        if hwnd_main.is_null() || hwnd_osd.is_null() {
            fatal("Failed to create application windows.");
            return;
        }

        let boxed = Box::new(AppState {
            vmr,
            settings,
            hwnd_main,
            hwnd_osd,
            hwnd_settings: null_mut(),
            icon_small,
            icon_big,
            last_gain: None,
            last_mute: None,
            percent: 0,
            muted: false,
            hide_at: None,
            surface: None,
            surface_dpi: 96,
            display_pct: 0.0,
            anim_alpha: 0.0,
            animating: false,
            pop: 0.0,
            tray_point: None,
            monitor: osd::MonitorInfo::default(),
            read_failures: 0,
            stale_ticks: 0,
            channel_label: String::new(),
            edition: 1,
            tray_ticks: 0,
        });
        STATE = Box::into_raw(boxed);

        let app = &mut *STATE;
        app.edition = app.vmr.edition();
        app.refresh();
        app.display_pct = app.percent as f32;
        app.refresh_label();
        osd::rebuild_surface(app);
        tray::add(hwnd_main, icon_small);
        tray::update_tip(hwnd_main, icon_small, &app.tooltip());

        // Wheel-over-tray needs a low-level hook: the shell's icon callback
        // never carries the wheel delta.
        HOOK_TARGET = hwnd_main;
        if let Some(rc) = tray::icon_rect(hwnd_main) {
            TRAY_RECT = rc;
        }
        let hook = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), hinstance, 0);

        SetTimer(hwnd_main, TIMER_POLL, POLL_MS, None);

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            // Gives the settings window tab navigation plus Enter/Esc.
            let settings = (*STATE).hwnd_settings;
            if !settings.is_null() && IsDialogMessageW(settings, &msg) != 0 {
                continue;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        if !hook.is_null() {
            UnhookWindowsHookEx(hook);
        }
        tray::remove(hwnd_main);
        drop(Box::from_raw(STATE));
        STATE = null_mut();
    }
}

fn fatal(message: &str) {
    unsafe {
        MessageBoxW(
            null_mut(),
            wide(message).as_ptr(),
            wide("Voicemeeter OSD").as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}
