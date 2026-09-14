use windows_sys::Win32::Foundation::{POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, HMONITOR, MONITORINFO,
    MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY,
};
use windows_sys::Win32::Graphics::GdiPlus::{
    GdipGraphicsClear, GdipResetWorldTransform, GdipScaleWorldTransform, MatrixOrderPrepend,
};
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::Shell::{
    SHQueryUserNotificationState, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::config::{Orientation, Position, Settings};
use crate::gfx::{argb, shade, tint, Canvas, LayeredSurface, ALIGN_CENTER, ALIGN_NEAR};
use crate::scale::{ease_out_cubic, format_db, gain_fraction};
use crate::{state, AppState};

pub const CLASS_NAME: &str = "VoicemeeterOsdBarClass";

/// Space reserved around the card for the drop shadow.
const SHADOW: i32 = 10;
/// Height of the callout tail that points down at the tray icon.
const TAIL_H: i32 = 9;
const TAIL_W: i32 = 18;

/// The tail only makes sense when docked by the tray, where there is
/// something for it to point at.
fn has_tail(settings: &Settings) -> bool {
    settings.position == Position::AboveTray
}

/// Full window size including shadow padding and tail, in physical pixels.
pub fn size_for(settings: &Settings, dpi: i32) -> (i32, i32) {
    let (w, h) = match settings.orientation {
        Orientation::Vertical => (66, 226),
        Orientation::Horizontal => (300, 86),
    };
    let pad = SHADOW * 2;
    let tail = if has_tail(settings) { TAIL_H } else { 0 };
    ((w + pad) * dpi / 96, (h + pad + tail) * dpi / 96)
}

/// Geometry of the display currently hosting the OSD. Captured when the bar
/// appears so it stays put for that showing, even if focus moves.
#[derive(Clone, Copy)]
pub struct MonitorInfo {
    pub work: RECT,
    pub full: RECT,
    pub dpi: i32,
}

impl Default for MonitorInfo {
    fn default() -> Self {
        unsafe {
            let primary = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
            metrics_of(primary)
        }
    }
}

unsafe fn metrics_of(monitor: HMONITOR) -> MonitorInfo {
    let mut mi: MONITORINFO = std::mem::zeroed();
    mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    if GetMonitorInfoW(monitor, &mut mi) == 0 {
        let fallback = RECT {
            left: 0,
            top: 0,
            right: GetSystemMetrics(SM_CXSCREEN),
            bottom: GetSystemMetrics(SM_CYSCREEN),
        };
        return MonitorInfo {
            work: fallback,
            full: fallback,
            dpi: 96,
        };
    }
    let mut dpi_x: u32 = 96;
    let mut dpi_y: u32 = 96;
    GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
    MonitorInfo {
        work: mi.rcWork,
        full: mi.rcMonitor,
        dpi: (dpi_x as i32).max(96),
    }
}

/// True when the user has actually dragged the bar somewhere; `custom_(0,0)`
/// is the never-dragged sentinel, which behaves like Centre.
fn custom_is_set(settings: &Settings) -> bool {
    settings.custom_x != 0 || settings.custom_y != 0
}

/// Picks the display to show on: the one owning the tray icon when docked,
/// otherwise the one the user is actually working on.
pub fn resolve_monitor(settings: &Settings, tray_point: Option<(i32, i32)>) -> MonitorInfo {
    unsafe {
        let monitor = match settings.position {
            Position::AboveTray => match tray_point {
                Some((x, y)) => MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST),
                None => MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY),
            },
            Position::Custom if custom_is_set(settings) => MonitorFromPoint(
                POINT {
                    x: settings.custom_x,
                    y: settings.custom_y,
                },
                MONITOR_DEFAULTTONEAREST,
            ),
            Position::Centre | Position::Custom => {
                let foreground = GetForegroundWindow();
                if !foreground.is_null() {
                    MonitorFromWindow(foreground, MONITOR_DEFAULTTONEAREST)
                } else {
                    let mut pt = POINT { x: 0, y: 0 };
                    if GetCursorPos(&mut pt) != 0 {
                        MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST)
                    } else {
                        MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY)
                    }
                }
            }
        };
        metrics_of(monitor)
    }
}

/// Top-left position for the window, shadow padding accounted for.
pub fn origin_for(
    settings: &Settings,
    dpi: i32,
    monitor: &MonitorInfo,
    tray_point: Option<(i32, i32)>,
) -> (i32, i32) {
    let (w, h) = size_for(settings, dpi);
    let gap = 10 * dpi / 96;
    let pad = SHADOW * dpi / 96;
    match settings.position {
        Position::AboveTray => {
            let wa = monitor.work;
            let y = wa.bottom - h + pad - gap;
            // Centre over the icon itself; fall back to the corner if the
            // shell won't tell us where it is.
            let x = match tray_point {
                Some((tx, _)) => {
                    let ideal = tx - w / 2;
                    let min_x = wa.left - pad + gap;
                    let max_x = wa.right - w + pad - gap;
                    ideal.clamp(min_x.min(max_x), max_x)
                }
                None => wa.right - w + pad - gap,
            };
            (x, y)
        }
        Position::Custom if custom_is_set(settings) => {
            // Saved coordinates can outlive the display they were set on;
            // keep enough of the card on screen to grab it back.
            let wa = monitor.work;
            let visible = 48 * dpi / 96;
            let min_x = wa.left - w + visible;
            let max_x = wa.right - visible;
            let min_y = wa.top - h + visible;
            let max_y = wa.bottom - visible;
            (
                settings.custom_x.clamp(min_x.min(max_x), max_x),
                settings.custom_y.clamp(min_y.min(max_y), max_y),
            )
        }
        // Horizontally centred but sitting low, so it floats clear of
        // whatever you're actually looking at instead of covering it.
        Position::Centre | Position::Custom => {
            let wa = monitor.work;
            let x = wa.left + (wa.right - wa.left - w) / 2;
            let y = wa.bottom - h - (wa.bottom - wa.top) / 8;
            (x, y)
        }
    }
}

pub unsafe fn rebuild_surface(app: &mut AppState) {
    let dpi = app.monitor.dpi;
    let (w, h) = size_for(&app.settings, dpi);
    app.surface = LayeredSurface::new(w, h);
    app.surface_dpi = dpi;
}

/// Repaints the card and pushes it to the layered window.
pub unsafe fn render(app: &mut AppState) {
    // The target display may scale differently from the last one we drew for.
    if app.surface.is_none() || app.surface_dpi != app.monitor.dpi {
        rebuild_surface(app);
    }
    let Some(surface) = &app.surface else { return };

    let dpi = app.surface_dpi;
    let s = |v: f32| v * dpi as f32 / 96.0;
    let canvas = surface.canvas();

    // Grow out of (and shrink back into) the tray icon.
    let pop = ease_out_cubic(app.pop.clamp(0.0, 1.0));
    let scale = MIN_SCALE + (1.0 - MIN_SCALE) * pop;

    GdipResetWorldTransform(canvas.graphics);
    GdipGraphicsClear(canvas.graphics, 0);
    GdipScaleWorldTransform(canvas.graphics, scale, scale, MatrixOrderPrepend);

    let vertical = app.settings.orientation == Orientation::Vertical;
    let pad = s(SHADOW as f32);
    let tail_h = if has_tail(&app.settings) {
        s(TAIL_H as f32)
    } else {
        0.0
    };
    let card_w = surface.width as f32 - pad * 2.0;
    let card_h = surface.height as f32 - pad * 2.0 - tail_h;
    let radius = s(if vertical { 22.0 } else { 16.0 });

    draw_shadow(&canvas, pad, card_w, card_h, radius, s(1.0));

    // Card body: subtle vertical gradient with a hairline border.
    canvas.fill_round_rect_gradient(
        pad,
        pad,
        card_w,
        card_h,
        radius,
        argb(247, 38, 38, 42),
        argb(247, 24, 24, 27),
    );
    canvas.stroke_round_rect(
        pad + 0.5,
        pad + 0.5,
        card_w - 1.0,
        card_h - 1.0,
        radius,
        argb(26, 255, 255, 255),
        s(1.0),
    );

    if tail_h > 0.0 {
        // Point at the icon's real x, not just the card's middle, so the tail
        // stays accurate when the card is clamped against a screen edge.
        let (end_x, _) = origin_for(&app.settings, dpi, &app.monitor, app.tray_point);
        let tail_cx = match app.tray_point {
            Some((tx, _)) => {
                let local = (tx - end_x) as f32;
                local.clamp(pad + radius + s(6.0), pad + card_w - radius - s(6.0))
            }
            None => pad + card_w / 2.0,
        };
        draw_tail(&canvas, tail_cx, pad + card_h, s(TAIL_W as f32), tail_h);
    }

    let accent = app.settings.effective_accent();
    let pct = (app.display_pct / 100.0).clamp(0.0, 1.0);

    if vertical {
        draw_vertical(&canvas, app, pad, card_w, card_h, &s, accent, pct);
    } else {
        draw_horizontal(&canvas, app, pad, card_w, card_h, &s, accent, pct);
    }

    drop(canvas);

    let draw_w = (surface.width as f32 * scale).round() as i32;
    let draw_h = (surface.height as f32 * scale).round() as i32;

    let (end_x, end_y) = origin_for(&app.settings, dpi, &app.monitor, app.tray_point);
    let docked = app.settings.position == Position::AboveTray;
    let (x, y) = match (docked, app.tray_point) {
        // Travel out of the tray icon toward the resting place.
        (true, Some((tx, ty))) => {
            let start_x = (tx - draw_w / 2) as f32;
            let start_y = (ty - draw_h / 2) as f32;
            (
                (start_x + (end_x as f32 - start_x) * pop).round() as i32,
                (start_y + (end_y as f32 - start_y) * pop).round() as i32,
            )
        }
        // Floating: grow about its own centre instead of flying across the screen.
        _ => (
            end_x + (surface.width - draw_w) / 2,
            end_y + (surface.height - draw_h) / 2,
        ),
    };

    let opacity = (app.settings.opacity_pct as f32 / 100.0) * app.anim_alpha;
    surface.commit(
        app.hwnd_osd,
        x,
        y,
        draw_w,
        draw_h,
        (opacity.clamp(0.0, 1.0) * 255.0) as u8,
    );
}

/// Smallest scale at the start of the pop, so it emerges from the icon
/// rather than from nothing.
const MIN_SCALE: f32 = 0.28;

/// Green through amber to red as the signal approaches clipping.
fn meter_color(level: f32) -> u32 {
    if level > 0.96 {
        argb(255, 240, 80, 70)
    } else if level > 0.88 {
        argb(255, 235, 175, 70)
    } else {
        argb(255, 110, 205, 130)
    }
}

/// True when a game or presentation is filling a screen, so a topmost
/// overlay would intrude.
pub fn fullscreen_foreground() -> bool {
    unsafe {
        let mut state = 0;
        if SHQueryUserNotificationState(&mut state) == 0
            && (state == QUNS_RUNNING_D3D_FULL_SCREEN || state == QUNS_PRESENTATION_MODE)
        {
            return true;
        }
        // Borderless-fullscreen windows don't set that state, so also check
        // whether the active window covers its entire monitor.
        let foreground = GetForegroundWindow();
        if foreground.is_null() || foreground == GetShellWindow() || is_desktop(foreground) {
            return false;
        }
        let mut rc: RECT = std::mem::zeroed();
        if GetWindowRect(foreground, &mut rc) == 0 {
            return false;
        }
        let monitor = metrics_of(MonitorFromWindow(foreground, MONITOR_DEFAULTTONEAREST));
        rc.left <= monitor.full.left
            && rc.top <= monitor.full.top
            && rc.right >= monitor.full.right
            && rc.bottom >= monitor.full.bottom
    }
}

/// The desktop is a monitor-sized window too: clicking the wallpaper focuses
/// `WorkerW` (or `Progman`), which must not count as a fullscreen app.
unsafe fn is_desktop(hwnd: windows_sys::Win32::Foundation::HWND) -> bool {
    let mut buf = [0u16; 32];
    let len = GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
    let class = String::from_utf16_lossy(&buf[..len.max(0) as usize]);
    class == "WorkerW" || class == "Progman"
}

/// Callout tail below the card. Drawn a touch above the card's edge so it
/// merges with the body instead of showing a seam.
unsafe fn draw_tail(canvas: &Canvas, cx: f32, card_bottom: f32, width: f32, height: f32) {
    let top = card_bottom - 1.0;
    let half = width / 2.0;
    let tip = card_bottom + height;
    canvas.fill_polygon(
        &[(cx - half, top), (cx + half, top), (cx, tip)],
        argb(247, 24, 24, 27),
    );
    let edge = argb(26, 255, 255, 255);
    canvas.stroke_line(cx - half, top, cx, tip, edge, 1.0);
    canvas.stroke_line(cx + half, top, cx, tip, edge, 1.0);
}

/// Cheap fake blur: concentric rounded rects with falling alpha.
unsafe fn draw_shadow(canvas: &Canvas, pad: f32, w: f32, h: f32, radius: f32, unit: f32) {
    let layers = 6;
    for i in (1..=layers).rev() {
        let spread = unit * i as f32 * 1.5;
        let alpha = (16.0 / i as f32) as u8;
        canvas.fill_round_rect(
            pad - spread,
            pad - spread + unit * 1.5,
            w + spread * 2.0,
            h + spread * 2.0,
            radius + spread,
            argb(alpha, 0, 0, 0),
        );
    }
}

unsafe fn draw_speaker(canvas: &Canvas, cx: f32, cy: f32, size: f32, muted: bool, color: u32) {
    let s = size;
    let body = [
        (cx - s * 0.42, cy - s * 0.16),
        (cx - s * 0.18, cy - s * 0.16),
        (cx + s * 0.10, cy - s * 0.44),
        (cx + s * 0.10, cy + s * 0.44),
        (cx - s * 0.18, cy + s * 0.16),
        (cx - s * 0.42, cy + s * 0.16),
    ];
    canvas.fill_polygon(&body, color);

    if muted {
        let x = cx + s * 0.24;
        let r = s * 0.26;
        canvas.stroke_line(x, cy - r, x + r * 1.5, cy + r, color, s * 0.14);
        canvas.stroke_line(x, cy + r, x + r * 1.5, cy - r, color, s * 0.14);
    } else {
        canvas.stroke_arc(cx + s * 0.06, cy, s * 0.30, -52.0, 104.0, color, s * 0.13);
        canvas.stroke_arc(
            cx + s * 0.06,
            cy,
            s * 0.50,
            -52.0,
            104.0,
            argb(((color >> 24) as u8).saturating_sub(70), 255, 255, 255),
            s * 0.13,
        );
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn draw_vertical(
    canvas: &Canvas,
    app: &AppState,
    pad: f32,
    card_w: f32,
    card_h: f32,
    s: &dyn Fn(f32) -> f32,
    accent: (u8, u8, u8),
    pct: f32,
) {
    let cx = pad + card_w / 2.0;
    let glyph_cy = pad + s(24.0);
    draw_speaker(
        canvas,
        cx,
        glyph_cy,
        s(20.0),
        app.muted,
        argb(235, 255, 255, 255),
    );

    let track_w = s(10.0);
    // Centre the track+meter pair as a group, not the track alone.
    let track_x = cx - s(10.5);
    let track_top = pad + s(58.0);
    let track_bottom = pad + card_h - s(42.0);
    let track_h = track_bottom - track_top;
    canvas.fill_round_rect(
        track_x,
        track_top,
        track_w,
        track_h,
        track_w / 2.0,
        argb(38, 255, 255, 255),
    );

    // Live signal meter alongside the fader, so you can see level as well as
    // the setting.
    let meter_w = s(4.0);
    let meter_x = track_x + track_w + s(7.0);
    canvas.fill_round_rect(
        meter_x,
        track_top,
        meter_w,
        track_h,
        meter_w / 2.0,
        argb(30, 255, 255, 255),
    );
    let meter_h = track_h * app.meter.clamp(0.0, 1.0);
    if meter_h > 0.5 {
        canvas.fill_round_rect(
            meter_x,
            track_bottom - meter_h,
            meter_w,
            meter_h,
            meter_w / 2.0,
            meter_color(app.meter),
        );
    }
    if app.meter_peak > 0.02 {
        let y = track_bottom - track_h * app.meter_peak.clamp(0.0, 1.0);
        canvas.fill_round_rect(
            meter_x,
            y - s(1.0),
            meter_w,
            s(2.0),
            s(1.0),
            meter_color(app.meter_peak),
        );
    }

    // Unity tick: shows at a glance whether you're above or below 0 dB.
    if app.settings.min_db < 0.0 && app.settings.max_db > 0.0 {
        let unity = gain_fraction(0.0, app.settings.min_db, app.settings.max_db);
        let y = track_bottom - track_h * unity;
        canvas.fill_round_rect(
            track_x - s(5.0),
            y - s(0.5),
            track_w + s(10.0),
            s(1.0),
            0.0,
            argb(70, 255, 255, 255),
        );
    }

    let fill_h = track_h * pct;
    if fill_h > 0.5 {
        let (top_color, bottom_color) = if app.muted {
            (
                tint(shade((150, 62, 62), 0.25), 255),
                tint((150, 62, 62), 255),
            )
        } else {
            (tint(shade(accent, 0.28), 255), tint(accent, 255))
        };
        canvas.fill_round_rect_gradient(
            track_x,
            track_bottom - fill_h,
            track_w,
            fill_h,
            track_w / 2.0,
            top_color,
            bottom_color,
        );
    }

    // Channel caption above the track.
    canvas.draw_text(
        &app.channel_label,
        pad,
        pad + s(38.0),
        card_w,
        s(14.0),
        s(10.5),
        argb(150, 255, 255, 255),
        ALIGN_CENTER,
        false,
    );

    let label = if app.muted {
        "Muted".to_string()
    } else {
        format!("{}%", app.percent)
    };
    canvas.draw_text(
        &label,
        pad,
        pad + card_h - s(36.0),
        card_w,
        s(20.0),
        s(14.0),
        argb(240, 255, 255, 255),
        ALIGN_CENTER,
        true,
    );
    if !app.muted {
        canvas.draw_text(
            &format_db(app.last_gain),
            pad,
            pad + card_h - s(20.0),
            card_w,
            s(14.0),
            s(10.5),
            argb(140, 255, 255, 255),
            ALIGN_CENTER,
            false,
        );
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn draw_horizontal(
    canvas: &Canvas,
    app: &AppState,
    pad: f32,
    card_w: f32,
    card_h: f32,
    s: &dyn Fn(f32) -> f32,
    accent: (u8, u8, u8),
    pct: f32,
) {
    let glyph_cx = pad + s(34.0);
    let glyph_cy = pad + card_h / 2.0;
    draw_speaker(
        canvas,
        glyph_cx,
        glyph_cy,
        s(26.0),
        app.muted,
        argb(235, 255, 255, 255),
    );

    let track_left = pad + s(62.0);
    let track_right = pad + card_w - s(74.0);
    let track_h = s(10.0);
    let track_y = pad + card_h / 2.0 - track_h / 2.0 + s(6.0);
    let track_w = track_right - track_left;

    canvas.draw_text(
        &app.channel_label,
        track_left,
        pad + s(14.0),
        track_w,
        s(14.0),
        s(10.5),
        argb(150, 255, 255, 255),
        ALIGN_NEAR,
        false,
    );
    canvas.fill_round_rect(
        track_left,
        track_y,
        track_w,
        track_h,
        track_h / 2.0,
        argb(38, 255, 255, 255),
    );

    if app.settings.min_db < 0.0 && app.settings.max_db > 0.0 {
        let unity = gain_fraction(0.0, app.settings.min_db, app.settings.max_db);
        let x = track_left + track_w * unity;
        canvas.fill_round_rect(
            x - s(0.5),
            track_y - s(5.0),
            s(1.0),
            track_h + s(10.0),
            0.0,
            argb(70, 255, 255, 255),
        );
    }

    // Live signal meter under the fader, with the same peak-hold behaviour
    // as the vertical layout.
    let meter_h = s(4.0);
    let meter_y = track_y + track_h + s(8.0);
    canvas.fill_round_rect(
        track_left,
        meter_y,
        track_w,
        meter_h,
        meter_h / 2.0,
        argb(30, 255, 255, 255),
    );
    let meter_w = track_w * app.meter.clamp(0.0, 1.0);
    if meter_w > 0.5 {
        canvas.fill_round_rect(
            track_left,
            meter_y,
            meter_w,
            meter_h,
            meter_h / 2.0,
            meter_color(app.meter),
        );
    }
    if app.meter_peak > 0.02 {
        let x = track_left + track_w * app.meter_peak.clamp(0.0, 1.0);
        canvas.fill_round_rect(
            x - s(1.0),
            meter_y - s(1.0),
            s(2.0),
            meter_h + s(2.0),
            s(1.0),
            meter_color(app.meter_peak),
        );
    }

    let fill_w = track_w * pct;
    if fill_w > 0.5 {
        let (left_color, right_color) = if app.muted {
            (
                tint((150, 62, 62), 255),
                tint(shade((150, 62, 62), 0.25), 255),
            )
        } else {
            (tint(accent, 255), tint(shade(accent, 0.28), 255))
        };
        canvas.fill_round_rect_gradient(
            track_left,
            track_y,
            fill_w,
            track_h,
            track_h / 2.0,
            left_color,
            right_color,
        );
    }

    let label = if app.muted {
        "Muted".to_string()
    } else {
        format!("{}%", app.percent)
    };
    canvas.draw_text(
        &label,
        pad + card_w - s(70.0),
        pad + card_h / 2.0 - s(14.0),
        s(58.0),
        s(20.0),
        s(15.0),
        argb(240, 255, 255, 255),
        ALIGN_CENTER,
        true,
    );
    if !app.muted {
        canvas.draw_text(
            &format_db(app.last_gain),
            pad + card_w - s(70.0),
            pad + card_h / 2.0 + s(6.0),
            s(58.0),
            s(14.0),
            s(10.5),
            argb(140, 255, 255, 255),
            ALIGN_CENTER,
            false,
        );
    }
}

/// Draws a miniature of the bar for the settings preview pane.
pub unsafe fn draw_preview(canvas: &Canvas, settings: &Settings, x: f32, y: f32, w: f32, h: f32) {
    let app = state();
    let (percent, muted) = if app.is_null() {
        (65, false)
    } else {
        ((*app).percent, (*app).muted)
    };
    let radius = 12.0;
    canvas.fill_round_rect_gradient(
        x,
        y,
        w,
        h,
        radius,
        argb(255, 38, 38, 42),
        argb(255, 24, 24, 27),
    );
    canvas.stroke_round_rect(
        x + 0.5,
        y + 0.5,
        w - 1.0,
        h - 1.0,
        radius,
        argb(30, 255, 255, 255),
        1.0,
    );

    let pct = (percent as f32 / 100.0).clamp(0.0, 1.0);
    let accent = settings.effective_accent();
    if settings.orientation == Orientation::Vertical {
        let track_w = 8.0;
        let track_x = x + w / 2.0 - track_w / 2.0;
        let track_top = y + 14.0;
        let track_h = h - 40.0;
        canvas.fill_round_rect(
            track_x,
            track_top,
            track_w,
            track_h,
            track_w / 2.0,
            argb(38, 255, 255, 255),
        );
        let fill_h = track_h * pct;
        canvas.fill_round_rect_gradient(
            track_x,
            track_top + track_h - fill_h,
            track_w,
            fill_h,
            track_w / 2.0,
            tint(shade(accent, 0.28), 255),
            tint(accent, 255),
        );
        canvas.draw_text(
            &format!("{percent}%"),
            x,
            y + h - 22.0,
            w,
            16.0,
            11.0,
            argb(230, 255, 255, 255),
            ALIGN_CENTER,
            true,
        );
    } else {
        let track_h = 8.0;
        let track_left = x + 34.0;
        let track_w = w - 76.0;
        let track_y = y + h / 2.0 - track_h / 2.0;
        draw_speaker(
            canvas,
            x + 20.0,
            y + h / 2.0,
            16.0,
            muted,
            argb(230, 255, 255, 255),
        );
        canvas.fill_round_rect(
            track_left,
            track_y,
            track_w,
            track_h,
            track_h / 2.0,
            argb(38, 255, 255, 255),
        );
        canvas.fill_round_rect_gradient(
            track_left,
            track_y,
            track_w * pct,
            track_h,
            track_h / 2.0,
            tint(accent, 255),
            tint(shade(accent, 0.28), 255),
        );
        canvas.draw_text(
            &format!("{percent}%"),
            x + w - 40.0,
            y + h / 2.0 - 8.0,
            34.0,
            16.0,
            11.0,
            argb(230, 255, 255, 255),
            ALIGN_CENTER,
            true,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor() -> MonitorInfo {
        let area = RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1040,
        };
        MonitorInfo {
            work: area,
            full: area,
            dpi: 96,
        }
    }

    #[test]
    fn offscreen_custom_position_is_pulled_back() {
        let mut settings = Settings::default();
        settings.position = Position::Custom;
        settings.custom_x = -3000;
        settings.custom_y = 4000;
        let (x, y) = origin_for(&settings, 96, &monitor(), None);
        let (w, h) = size_for(&settings, 96);
        let wa = monitor().work;
        assert!(x + w >= wa.left + 48);
        assert!(x <= wa.right - 48);
        assert!(y + h >= wa.top + 48);
        assert!(y <= wa.bottom - 48);
    }

    #[test]
    fn onscreen_custom_position_is_untouched() {
        let mut settings = Settings::default();
        settings.position = Position::Custom;
        settings.custom_x = 300;
        settings.custom_y = 200;
        assert_eq!(origin_for(&settings, 96, &monitor(), None), (300, 200));
    }
}
