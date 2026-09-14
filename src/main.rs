#![windows_subsystem = "windows"]

mod config;
mod gfx;
mod log;
mod osd;
mod scale;
mod settings_ui;
mod tray;
mod vmr;

use std::ffi::CString;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicI32, AtomicPtr, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    GetLastError, COLORREF, ERROR_ALREADY_EXISTS, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT,
    WPARAM,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, ReleaseCapture, SetCapture, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_NOREPEAT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use config::{
    is_autostart_enabled, set_autostart, ChannelKind, Position, Settings, FADER_MAX_DB,
    FADER_MIN_DB,
};
use gfx::{Gdiplus, LayeredSurface};
use vmr::{Link, VmrApi};

pub const WM_TRAYICON: u32 = WM_APP + 1;
/// Posted by the mouse hook when the wheel turns over the tray icon.
const WM_TRAY_WHEEL: u32 = WM_APP + 2;
/// Posted by a second launch, asking this instance to open its settings.
const WM_OPEN_SETTINGS: u32 = WM_APP + 3;
/// Sent when the user changes their Windows accent colour.
const WM_DWMCOLORIZATIONCOLORCHANGED: u32 = 0x0320;
pub const TRAY_ID: u32 = 1;
pub const ID_MENU_SETTINGS: i32 = 2001;
pub const ID_MENU_AUTOSTART: i32 = 2002;
pub const ID_MENU_EXIT: i32 = 2003;
pub const ID_MENU_MUTE: i32 = 2004;
pub const ID_MENU_VOICEMEETER: i32 = 2005;
/// Channel submenu items, one id per entry in `config::all_channels` order.
pub const ID_MENU_CHANNEL_FIRST: i32 = 2100;
pub const ID_MENU_CHANNEL_COUNT: usize = 64;

const TIMER_POLL: usize = 1;
const TIMER_ANIM: usize = 2;
const POLL_MS: u32 = 40;
const FRAME_MS: u32 = 16;
const HOTKEY_UP: i32 = 1;
const HOTKEY_DOWN: i32 = 2;
const HOTKEY_MUTE: i32 = 3;
const FADE_IN_SECS: f32 = 0.11;
const FADE_OUT_SECS: f32 = 0.22;
const POP_IN_SECS: f32 = 0.26;
const POP_OUT_SECS: f32 = 0.20;
/// ~2s of failed polls before trying to log in again.
const STALE_POLLS: u32 = 50;
/// Reconnect attempts back off from here up to the cap, so a Voicemeeter
/// that isn't running (or a channel that doesn't exist) doesn't churn.
const RETRY_MIN: Duration = Duration::from_secs(2);
const RETRY_MAX: Duration = Duration::from_secs(60);

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
    /// Consecutive polls where the link or the channel read failed.
    failed_polls: u32,
    reconnect: vmr::Backoff,
    /// Carries partial wheel notches between events.
    wheel_steps: scale::WheelSteps,
    /// "A1", "Strip 2", or the user's own Voicemeeter label.
    pub channel_label: String,
    /// Layout of the running Voicemeeter. Falls back to 1 until it can be
    /// read, which is common when started at boot before Voicemeeter.
    pub edition: i32,
    edition_known: bool,
    /// Live signal level, 0-1, with a slowly falling peak marker.
    pub meter: f32,
    pub meter_peak: f32,
    /// Channel actually shown. Equals the configured one unless watch_all
    /// has followed a fader that moved.
    pub watch_kind: ChannelKind,
    pub watch_index: i32,
    /// Parameter names for the watched channel, built once per change of
    /// channel rather than on every poll.
    watch_gain: CString,
    watch_mute: CString,
    /// Every channel polled when watch_all is on, each with the values seen
    /// on the previous tick.
    pub watch_targets: Vec<WatchTarget>,
    /// Set while the user is dragging the bar somewhere new.
    pub drag: Option<DragAnchor>,
    last_frame: Instant,
    mouse_hook: HHOOK,
    tray_ticks: u32,
}

/// One strip or bus polled by the watch-all scan.
pub struct WatchTarget {
    pub kind: ChannelKind,
    pub index: i32,
    pub gain: CString,
    pub mute: CString,
    pub last_gain: Option<f32>,
    pub last_mute: Option<f32>,
}

/// All channels the running edition exposes, in a fixed scan order.
pub fn build_watch_targets(edition: i32) -> Vec<WatchTarget> {
    config::all_channels(edition)
        .into_iter()
        .map(|(kind, index)| WatchTarget {
            kind,
            index,
            gain: CString::new(Settings::gain_param(kind, index)).unwrap(),
            mute: CString::new(Settings::mute_param(kind, index)).unwrap(),
            last_gain: None,
            last_mute: None,
        })
        .collect()
}

/// Where a grab started, so a click can be told apart from a real drag.
pub struct DragAnchor {
    pub cursor: (i32, i32),
    pub window: (i32, i32),
    pub moved: bool,
}

impl AppState {
    /// Points the display at another channel and re-reads it without
    /// animating from the previous channel's level.
    pub fn set_watch(&mut self, kind: ChannelKind, index: i32) {
        self.watch_kind = kind;
        self.watch_index = index;
        self.watch_gain = CString::new(Settings::gain_param(kind, index)).unwrap();
        self.watch_mute = CString::new(Settings::mute_param(kind, index)).unwrap();
        self.last_gain = None;
        self.last_mute = None;
        self.refresh();
        self.display_pct = self.percent as f32;
        self.refresh_label();
    }

    /// Prefers the label the user typed into Voicemeeter over the generic name.
    pub fn refresh_label(&mut self) {
        self.channel_label = self
            .vmr
            .get_string(&Settings::label_param(self.watch_kind, self.watch_index))
            .unwrap_or_else(|| {
                Settings::channel_name(self.watch_kind, self.watch_index, self.edition)
            });
    }

    /// Rebuilds the watch-all scan list to match the current settings.
    pub fn sync_watch_targets(&mut self) {
        if self.settings.watch_all {
            self.watch_targets = build_watch_targets(self.edition);
        } else {
            self.watch_targets.clear();
        }
    }

    fn tooltip(&self) -> String {
        if self.muted {
            format!("{}  Muted", self.channel_label)
        } else {
            format!(
                "{}  {} ({}%)",
                self.channel_label,
                scale::format_db(self.last_gain),
                self.percent
            )
        }
    }

    /// Controls always act on the configured channel, not whichever one
    /// watch-all last followed, so a hotkey never changes an unexpected fader.
    fn focus_configured(&mut self) {
        let (kind, index) = (self.settings.kind, self.settings.index);
        if kind != self.watch_kind || index != self.watch_index {
            self.set_watch(kind, index);
        }
    }

    /// Nudges the configured channel by `steps` wheel notches or presses.
    fn nudge(&mut self, steps: i32) {
        self.focus_configured();
        let Some(current) = self.last_gain else {
            return;
        };
        // Clamp to the fader's real limits, not the display range, so a
        // narrowed range can't stop the wheel from reaching the ends.
        let target =
            (current + steps as f32 * self.settings.step_db).clamp(FADER_MIN_DB, FADER_MAX_DB);
        self.vmr.set_float_c(&self.watch_gain, target);
    }

    fn toggle_mute(&mut self) {
        self.focus_configured();
        let target = if self.muted { 0.0 } else { 1.0 };
        self.vmr.set_float_c(&self.watch_mute, target);
    }

    /// First channel whose gain or mute moved since the previous poll.
    /// Snapshots update for every target regardless, so a slow fader sweep
    /// is only reported once per actual change.
    fn scan_for_movement(&mut self) -> Option<(ChannelKind, i32)> {
        let mut moved = None;
        for target in &mut self.watch_targets {
            let gain = self.vmr.get_float_c(&target.gain);
            let mute = self.vmr.get_float_c(&target.mute);
            // A first successful read is the baseline, not movement.
            let gain_moved = matches!((target.last_gain, gain), (Some(a), Some(b)) if a != b);
            let mute_moved = matches!((target.last_mute, mute), (Some(a), Some(b)) if a != b);
            if moved.is_none() && (gain_moved || mute_moved) {
                moved = Some((target.kind, target.index));
            }
            if gain.is_some() {
                target.last_gain = gain;
            }
            if mute.is_some() {
                target.last_mute = mute;
            }
        }
        moved
    }

    fn refresh(&mut self) -> bool {
        let gain = self.vmr.get_float_c(&self.watch_gain);
        let mute = self.vmr.get_float_c(&self.watch_mute);
        self.read_failures = u32::from(gain.is_none());
        let mut changed = false;
        if let Some(g) = gain {
            if self.last_gain != Some(g) {
                changed = true;
            }
            self.last_gain = Some(g);
            self.percent = scale::gain_to_percent(g, self.settings.min_db, self.settings.max_db);
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

    /// Picks up the edition once Voicemeeter can report it, and again after
    /// a reconnect in case a different edition was started.
    fn check_edition(&mut self) {
        if self.edition_known {
            return;
        }
        let Some(edition) = self.vmr.edition() else {
            return;
        };
        self.edition_known = true;
        if edition != self.edition {
            log::log_line!("edition is now {edition} (was {})", self.edition);
            self.edition = edition;
            self.sync_watch_targets();
            self.refresh_label();
            tray::update_tip(self.hwnd_main, self.icon_small, &self.tooltip());
        }
    }

    /// Tracks connection health and logs back in, with backoff, when the
    /// session has been failing for a while.
    fn supervise_link(&mut self, link: Link) {
        if link == Link::Up && self.read_failures == 0 {
            self.failed_polls = 0;
            self.reconnect.reset();
            return;
        }
        if link == Link::NoServer {
            // Whatever starts next may be a different edition.
            self.edition_known = false;
        }
        self.failed_polls = self.failed_polls.saturating_add(1);
        if self.failed_polls < STALE_POLLS || !self.reconnect.attempt(Instant::now()) {
            return;
        }
        log::log_line!(
            "connection unhealthy ({link:?}, read_failures={}), logging back in; backoff now {:?}",
            self.read_failures,
            self.reconnect.delay()
        );
        self.vmr.relogin();
        self.edition_known = false;
    }

    /// Switches the configured channel from the tray menu, keeps the choice,
    /// and pops the bar so it's clear what's now being controlled.
    unsafe fn choose_channel(&mut self, kind: ChannelKind, index: i32) {
        self.settings.kind = kind;
        self.settings.index = index;
        self.settings.save();
        self.set_watch(kind, index);
        settings_ui::channel_chosen(self.hwnd_settings, kind, index);
        log::log_line!("tray: chose {}[{}]", Settings::prefix_of(kind), index);
        tray::update_tip(self.hwnd_main, self.icon_small, &self.tooltip());
        show_osd(self);
    }

    /// Applies a raw wheel delta from the tray icon or the bar.
    unsafe fn wheel(&mut self, delta: i32) {
        let steps = self.wheel_steps.feed(delta);
        if steps != 0 {
            self.nudge(steps);
            after_control(self.hwnd_main, self);
        }
    }
}

/// The one AppState, owned by `main` for the life of the message loop.
/// Window procedures and hooks can't carry Rust context, so they reach it
/// through here; everything runs on the UI thread, so the atomic is only
/// there to avoid `static mut`, not to share across threads.
static STATE: AtomicPtr<AppState> = AtomicPtr::new(null_mut());

pub fn state() -> *mut AppState {
    STATE.load(Ordering::Relaxed)
}

pub fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16)
}

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Cached tray icon bounds, so the mouse hook can test containment without
/// an RPC on every wheel event. Atomics because the hook reads them from
/// inside a system callback.
static TRAY_LEFT: AtomicI32 = AtomicI32::new(0);
static TRAY_TOP: AtomicI32 = AtomicI32::new(0);
static TRAY_RIGHT: AtomicI32 = AtomicI32::new(0);
static TRAY_BOTTOM: AtomicI32 = AtomicI32::new(0);
static HOOK_TARGET: AtomicPtr<core::ffi::c_void> = AtomicPtr::new(null_mut());
/// Explorer broadcasts this when the taskbar is (re)created.
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

fn store_tray_rect(rc: RECT) {
    TRAY_LEFT.store(rc.left, Ordering::Relaxed);
    TRAY_TOP.store(rc.top, Ordering::Relaxed);
    TRAY_RIGHT.store(rc.right, Ordering::Relaxed);
    TRAY_BOTTOM.store(rc.bottom, Ordering::Relaxed);
}

/// Re-reads where the tray icon sits. An unknown position clears the rect,
/// so the hook never swallows scrolls over a spot the icon has left.
fn refresh_tray_rect(hwnd: HWND) {
    let rc = tray::icon_rect(hwnd).unwrap_or(RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    });
    store_tray_rect(rc);
}

unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && wparam as u32 == WM_MOUSEWHEEL {
        let info = lparam as *const MSLLHOOKSTRUCT;
        if !info.is_null() {
            let pt = (*info).pt;
            let inside = pt.x >= TRAY_LEFT.load(Ordering::Relaxed)
                && pt.x < TRAY_RIGHT.load(Ordering::Relaxed)
                && pt.y >= TRAY_TOP.load(Ordering::Relaxed)
                && pt.y < TRAY_BOTTOM.load(Ordering::Relaxed);
            let target = HOOK_TARGET.load(Ordering::Relaxed);
            if inside && !target.is_null() {
                let delta = ((*info).mouseData >> 16) as i16 as i32;
                PostMessageW(target, WM_TRAY_WHEEL, delta as WPARAM, 0);
                return 1; // don't let the scroll fall through to the taskbar
            }
        }
    }
    CallNextHookEx(null_mut(), code, wparam, lparam)
}

unsafe fn show_osd(app: &mut AppState) {
    if app.settings.hide_in_fullscreen && osd::fullscreen_foreground() {
        log::log_line!("suppressed: fullscreen app has focus");
        return;
    }
    app.hide_at = Some(Instant::now() + Duration::from_millis(app.settings.hide_ms as u64));
    if !app.animating {
        // Fresh appearance: pick the display, then pop out of the tray.
        // Also re-read the label, which may have been renamed in Voicemeeter.
        app.refresh_label();
        app.animating = true;
        app.tray_point = tray::icon_center(app.hwnd_main);
        app.monitor = osd::resolve_monitor(&app.settings, app.tray_point);
        app.pop = 0.0;
        app.last_frame = Instant::now();
        SetTimer(app.hwnd_main, TIMER_ANIM, FRAME_MS, None);
    }
    log::log_line!(
        "show: {} {} ({}%) muted={}",
        app.channel_label,
        scale::format_db(app.last_gain),
        app.percent,
        app.muted
    );
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

/// Registers the global hotkeys and installs the tray-scroll mouse hook to
/// match settings. Safe to call whenever: everything is torn down first,
/// then set up again if the feature is on. Returns the shortcuts that
/// couldn't be registered, usually because another app already owns them.
pub fn sync_input(app: &mut AppState) -> Vec<String> {
    let mut taken = Vec::new();
    unsafe {
        for id in [HOTKEY_UP, HOTKEY_DOWN, HOTKEY_MUTE] {
            UnregisterHotKey(app.hwnd_main, id);
        }
        if app.settings.hotkeys_enabled {
            for (id, key) in [
                (HOTKEY_UP, app.settings.hotkey_up),
                (HOTKEY_DOWN, app.settings.hotkey_down),
                (HOTKEY_MUTE, app.settings.hotkey_mute),
            ] {
                if !key.is_set() {
                    continue;
                }
                // Volume should ramp while held; mute shouldn't flicker.
                let mut mods = key.mods as HOT_KEY_MODIFIERS;
                if id == HOTKEY_MUTE {
                    mods |= MOD_NOREPEAT;
                }
                if RegisterHotKey(app.hwnd_main, id, mods, key.vk) == 0 {
                    log::log_line!(
                        "hotkey {} failed to register (already taken?)",
                        key.describe()
                    );
                    taken.push(key.describe());
                }
            }
        }

        let want_hook = app.settings.tray_scroll;
        if want_hook && app.mouse_hook.is_null() {
            HOOK_TARGET.store(app.hwnd_main, Ordering::Relaxed);
            refresh_tray_rect(app.hwnd_main);
            app.mouse_hook = SetWindowsHookExW(
                WH_MOUSE_LL,
                Some(mouse_hook),
                GetModuleHandleW(null_mut()),
                0,
            );
        } else if !want_hook && !app.mouse_hook.is_null() {
            UnhookWindowsHookEx(app.mouse_hook);
            app.mouse_hook = null_mut();
        }
    }
    taken
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

unsafe fn cursor_over(hwnd: HWND) -> bool {
    let mut pt = POINT { x: 0, y: 0 };
    if GetCursorPos(&mut pt) == 0 {
        return false;
    }
    let mut rc: RECT = std::mem::zeroed();
    if GetWindowRect(hwnd, &mut rc) == 0 {
        return false;
    }
    pt.x >= rc.left && pt.x < rc.right && pt.y >= rc.top && pt.y < rc.bottom
}

unsafe fn animate(hwnd: HWND, app: &mut AppState) {
    // Timers are coarse and get coalesced under load, so step by the time
    // that really passed rather than assuming a perfect 16 ms.
    let now = Instant::now();
    let dt = now
        .duration_since(app.last_frame)
        .as_secs_f32()
        .clamp(0.001, 0.1);
    app.last_frame = now;
    // Frame-rate independent form of "move 30% / 25% of the way per 16 ms".
    let frames = dt / (FRAME_MS as f32 / 1000.0);
    let approach = |per_frame: f32| 1.0 - (1.0 - per_frame).powf(frames);

    // Meters only matter while the bar is on screen, so sample them here
    // rather than in the slower poll loop.
    let (kind, channel) = Settings::level_slot(app.watch_kind, app.watch_index, app.edition);
    let level = app
        .vmr
        .stereo_level(kind, channel)
        .map(|v| scale::amplitude_to_meter(v, app.settings.min_db))
        .unwrap_or(0.0);
    // Snap up, fall back slowly: how a VU meter is expected to behave.
    app.meter = if level > app.meter {
        level
    } else {
        app.meter + (level - app.meter) * approach(0.25)
    };
    app.meter_peak = if app.meter >= app.meter_peak {
        app.meter
    } else {
        (app.meter_peak - dt * 0.4).max(app.meter)
    };

    let target = app.percent as f32;
    let diff = target - app.display_pct;
    if diff.abs() < 0.4 {
        app.display_pct = target;
    } else {
        app.display_pct += diff * approach(0.3);
    }

    // Keep it up while the pointer is on it, since it's clickable and
    // scrollable now.
    if app.anim_alpha > 0.5 && cursor_over(app.hwnd_osd) {
        app.hide_at = Some(Instant::now() + Duration::from_millis(400));
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

/// Shared tail of every user-driven volume change.
unsafe fn after_control(hwnd: HWND, app: &mut AppState) {
    app.vmr.pump();
    app.refresh();
    tray::update_tip(hwnd, app.icon_small, &app.tooltip());
    show_osd(app);
}

unsafe fn poll(hwnd: HWND, app: &mut AppState) {
    // Pump the cache, but decide off real value changes: the dirty flag can
    // stop reporting if a previous client died without logging out.
    let link = app.vmr.pump();
    if link == Link::Up {
        app.check_edition();
    }
    if app.settings.watch_all && !app.watch_targets.is_empty() {
        if let Some((kind, index)) = app.scan_for_movement() {
            if kind != app.watch_kind || index != app.watch_index {
                app.set_watch(kind, index);
                tray::update_tip(hwnd, app.icon_small, &app.tooltip());
                show_osd(app);
                log::log_line!("watch: followed {}[{}]", Settings::prefix_of(kind), index);
            }
        }
    }
    if app.refresh() {
        tray::update_tip(hwnd, app.icon_small, &app.tooltip());
        show_osd(app);
    }
    app.tray_ticks += 1;
    if app.tray_ticks >= 50 {
        app.tray_ticks = 0;
        if !app.mouse_hook.is_null() {
            refresh_tray_rect(hwnd);
        }
    }
    app.supervise_link(link);
}

unsafe extern "system" fn main_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let taskbar_created = TASKBAR_CREATED.load(Ordering::Relaxed);
    if taskbar_created != 0 && msg == taskbar_created {
        // Explorer restarted: every tray icon is gone until re-added.
        let app = state();
        if !app.is_null() {
            let app = &*app;
            log::log_line!("taskbar recreated, restoring tray icon");
            tray::add(hwnd, app.icon_small);
            tray::update_tip(hwnd, app.icon_small, &app.tooltip());
            refresh_tray_rect(hwnd);
        }
        return 0;
    }

    match msg {
        WM_TRAYICON => {
            match (lparam & 0xFFFF) as u32 {
                WM_RBUTTONUP => {
                    let app = state();
                    if !app.is_null() {
                        tray::show_menu(hwnd, &*app);
                    }
                }
                WM_LBUTTONUP | WM_LBUTTONDBLCLK => settings_ui::open(GetModuleHandleW(null_mut())),
                _ => {}
            }
            0
        }
        WM_OPEN_SETTINGS => {
            settings_ui::open(GetModuleHandleW(null_mut()));
            0
        }
        WM_COMMAND => {
            match (wparam & 0xFFFF) as i32 {
                ID_MENU_SETTINGS => settings_ui::open(GetModuleHandleW(null_mut())),
                ID_MENU_AUTOSTART => set_autostart(!is_autostart_enabled()),
                ID_MENU_EXIT => {
                    DestroyWindow(hwnd);
                }
                id => {
                    let app = state();
                    if app.is_null() {
                        return 0;
                    }
                    let app = &mut *app;
                    match id {
                        ID_MENU_MUTE => {
                            app.toggle_mute();
                            after_control(hwnd, app);
                        }
                        ID_MENU_VOICEMEETER => app.vmr.show_voicemeeter(),
                        _ if id >= ID_MENU_CHANNEL_FIRST => {
                            let item = (id - ID_MENU_CHANNEL_FIRST) as usize;
                            if let Some(&(kind, index)) =
                                config::all_channels(app.edition).get(item)
                            {
                                app.choose_channel(kind, index);
                            }
                        }
                        _ => {}
                    }
                }
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
                TIMER_POLL => poll(hwnd, app),
                TIMER_ANIM => animate(hwnd, app),
                _ => {}
            }
            0
        }
        WM_TRAY_WHEEL => {
            let app = state();
            if !app.is_null() {
                (*app).wheel(wparam as i32);
            }
            0
        }
        WM_HOTKEY => {
            let app = state();
            if !app.is_null() {
                let app = &mut *app;
                match wparam as i32 {
                    HOTKEY_UP => app.nudge(1),
                    HOTKEY_DOWN => app.nudge(-1),
                    HOTKEY_MUTE => app.toggle_mute(),
                    _ => {}
                }
                after_control(hwnd, app);
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
        // Monitors, scaling or the taskbar moved: the tray icon and the
        // bar's resting place may both have changed.
        WM_DISPLAYCHANGE | WM_SETTINGCHANGE | WM_DPICHANGED => {
            refresh_tray_rect(hwnd);
            relayout_osd();
            DefWindowProcW(hwnd, msg, wparam, lparam)
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
        WM_LBUTTONDOWN => {
            let app = state();
            if !app.is_null() {
                let mut pt = POINT { x: 0, y: 0 };
                GetCursorPos(&mut pt);
                let mut rc: RECT = std::mem::zeroed();
                GetWindowRect(hwnd, &mut rc);
                (*app).drag = Some(DragAnchor {
                    cursor: (pt.x, pt.y),
                    window: (rc.left, rc.top),
                    moved: false,
                });
                SetCapture(hwnd);
            }
            0
        }
        WM_MOUSEMOVE => {
            let app = state();
            if !app.is_null() {
                let app = &mut *app;
                if let Some(drag) = &mut app.drag {
                    let mut pt = POINT { x: 0, y: 0 };
                    GetCursorPos(&mut pt);
                    let dx = pt.x - drag.cursor.0;
                    let dy = pt.y - drag.cursor.1;
                    if drag.moved || dx.abs() + dy.abs() > 6 {
                        if !drag.moved {
                            drag.moved = true;
                            // A real drag repositions the bar for good; kill
                            // the pop animation so it tracks the cursor 1:1.
                            app.settings.position = Position::Custom;
                            app.pop = 1.0;
                            app.anim_alpha = 1.0;
                        }
                        let x = drag.window.0 + dx;
                        let y = drag.window.1 + dy;
                        app.settings.custom_x = x;
                        app.settings.custom_y = y;
                        app.hide_at = Some(Instant::now() + Duration::from_millis(600));
                        SetWindowPos(
                            hwnd,
                            null_mut(),
                            x,
                            y,
                            0,
                            0,
                            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                        );
                    }
                }
            }
            0
        }
        WM_LBUTTONUP => {
            let app = state();
            if !app.is_null() {
                let app = &mut *app;
                match app.drag.take() {
                    Some(drag) if drag.moved => {
                        ReleaseCapture();
                        app.settings.save();
                        app.hide_at = Some(
                            Instant::now() + Duration::from_millis(app.settings.hide_ms as u64),
                        );
                        log::log_line!(
                            "dragged to {},{}",
                            app.settings.custom_x,
                            app.settings.custom_y
                        );
                        // An open settings window must not undo the drag
                        // when it's saved later.
                        settings_ui::bar_dragged(
                            app.hwnd_settings,
                            app.settings.custom_x,
                            app.settings.custom_y,
                        );
                        // Re-resolve the host monitor and rebuild the surface
                        // (the tray tail is gone now that we're floating).
                        app.monitor = osd::resolve_monitor(&app.settings, app.tray_point);
                        osd::rebuild_surface(app);
                        osd::render(app);
                    }
                    _ => {
                        ReleaseCapture();
                        app.vmr.show_voicemeeter();
                    }
                }
            }
            0
        }
        // Scroll over the bar to adjust, middle-click to mute. Windows routes
        // the wheel to the window under the pointer even though the bar
        // never takes focus.
        WM_MOUSEWHEEL => {
            let app = state();
            if !app.is_null() {
                (*app).wheel(((wparam >> 16) & 0xFFFF) as u16 as i16 as i32);
            }
            0
        }
        WM_MBUTTONUP => {
            let app = state();
            if !app.is_null() {
                let app = &mut *app;
                app.toggle_mute();
                after_control(app.hwnd_main, app);
            }
            0
        }
        WM_CAPTURECHANGED => {
            // Capture lost without a button-up (alt-tab, etc.): drop the drag.
            let app = state();
            if !app.is_null() {
                (*app).drag = None;
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

/// Launching again is how most people look for a "lost" tray app, so hand
/// the request to the running copy instead of doing nothing.
fn signal_running_instance() {
    unsafe {
        let existing = FindWindowW(wide(MAIN_CLASS).as_ptr(), null_mut());
        if !existing.is_null() {
            PostMessageW(existing, WM_OPEN_SETTINGS, 0, 0);
        }
    }
}

fn main() {
    if already_running() {
        signal_running_instance();
        return;
    }

    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let _gdiplus = Gdiplus::start();

    let vmr = match VmrApi::load() {
        Ok(v) => v,
        Err(e) => {
            fatal(&format!(
                "Could not load the Voicemeeter Remote API.\n\n{e}"
            ));
            return;
        }
    };
    if !vmr.is_logged_in() {
        fatal("Could not connect to Voicemeeter. Make sure Voicemeeter is installed and running.");
        return;
    }

    let settings = Settings::load();
    log::set_enabled(settings.debug_log);
    config::refresh_installed_copy();

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

        TASKBAR_CREATED.store(
            RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()),
            Ordering::Relaxed,
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

        let (watch_kind, watch_index) = (settings.kind, settings.index);
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
            failed_polls: 0,
            reconnect: vmr::Backoff::new(RETRY_MIN, RETRY_MAX),
            wheel_steps: scale::WheelSteps::default(),
            channel_label: String::new(),
            edition: 1,
            edition_known: false,
            meter: 0.0,
            meter_peak: 0.0,
            watch_kind,
            watch_index,
            watch_gain: CString::default(),
            watch_mute: CString::default(),
            watch_targets: Vec::new(),
            drag: None,
            last_frame: Instant::now(),
            mouse_hook: null_mut(),
            tray_ticks: 0,
        });
        let raw = Box::into_raw(boxed);
        STATE.store(raw, Ordering::Relaxed);

        let app = &mut *raw;
        app.check_edition();
        app.sync_watch_targets();
        app.set_watch(watch_kind, watch_index);
        log::log_line!(
            "started: edition={} (known={}) channel={} logged_in={}",
            app.edition,
            app.edition_known,
            Settings::gain_param(app.watch_kind, app.watch_index),
            app.vmr.is_logged_in()
        );
        osd::rebuild_surface(app);
        tray::add(hwnd_main, icon_small);
        tray::update_tip(hwnd_main, icon_small, &app.tooltip());
        // Wheel-over-tray needs a low-level hook: the shell's icon callback
        // never carries the wheel delta. Installed only if enabled.
        sync_input(app);

        SetTimer(hwnd_main, TIMER_POLL, POLL_MS, None);

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            // Gives the settings window tab navigation plus Enter/Esc, except
            // where a shortcut picker needs the key itself.
            let settings = (*raw).hwnd_settings;
            if !settings.is_null() {
                match settings_ui::route_key(settings, &msg) {
                    settings_ui::KeyRoute::Handled => continue,
                    settings_ui::KeyRoute::Raw => {}
                    settings_ui::KeyRoute::Dialog => {
                        if IsDialogMessageW(settings, &msg) != 0 {
                            continue;
                        }
                    }
                }
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        let app = &mut *raw;
        if !app.mouse_hook.is_null() {
            UnhookWindowsHookEx(app.mouse_hook);
        }
        for id in [HOTKEY_UP, HOTKEY_DOWN, HOTKEY_MUTE] {
            UnregisterHotKey(hwnd_main, id);
        }
        tray::remove(hwnd_main);
        STATE.store(null_mut(), Ordering::Relaxed);
        drop(Box::from_raw(raw));
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
