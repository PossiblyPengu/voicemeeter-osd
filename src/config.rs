use std::path::PathBuf;
use windows_sys::Win32::Graphics::Dwm::DwmGetColorizationColor;
use windows_sys::Win32::System::Registry::*;

#[derive(Clone, Copy, PartialEq)]
pub enum ChannelKind {
    Strip,
    Bus,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Orientation {
    Vertical,
    Horizontal,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Position {
    AboveTray,
    Centre,
    /// Wherever the user dragged it to.
    Custom,
}

/// Voicemeeter's own fader limits; the display range must sit inside them.
pub const FADER_MIN_DB: f32 = -60.0;
pub const FADER_MAX_DB: f32 = 12.0;

// RegisterHotKey modifier flags, kept here so parsing stays Win32-free.
pub const MOD_ALT: u32 = 0x1;
pub const MOD_CONTROL: u32 = 0x2;
pub const MOD_SHIFT: u32 = 0x4;
pub const MOD_WIN: u32 = 0x8;

/// A global shortcut as RegisterHotKey takes it. `vk == 0` means unassigned.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Hotkey {
    pub mods: u32,
    pub vk: u32,
}

impl Hotkey {
    pub const NONE: Hotkey = Hotkey { mods: 0, vk: 0 };

    pub fn is_set(&self) -> bool {
        self.vk != 0
    }

    /// Stored as `mods:vk`, e.g. `3:38` for Ctrl+Alt+Up.
    fn parse(v: &str) -> Option<Hotkey> {
        let (mods, vk) = v.split_once(':')?;
        Some(Hotkey {
            mods: mods.trim().parse::<u32>().ok()? & (MOD_ALT | MOD_CONTROL | MOD_SHIFT | MOD_WIN),
            vk: vk.trim().parse::<u32>().ok()?.min(0xFF),
        })
    }

    fn serialize(&self) -> String {
        format!("{}:{}", self.mods, self.vk)
    }

    /// From a hotkey control's `HKM_GETHOTKEY` word: low byte is the key,
    /// high byte is HOTKEYF_SHIFT(1) / CONTROL(2) / ALT(4) / EXT(8).
    pub fn from_control(word: u32) -> Hotkey {
        let vk = word & 0xFF;
        let flags = (word >> 8) & 0xFF;
        let mut mods = 0;
        for (flag, m) in [(1, MOD_SHIFT), (2, MOD_CONTROL), (4, MOD_ALT)] {
            if flags & flag != 0 {
                mods |= m;
            }
        }
        Hotkey { mods, vk }
    }

    /// Word for `HKM_SETHOTKEY`. Navigation keys need the extended flag or
    /// the control names them after the numpad ("Num 8" instead of "Up").
    pub fn to_control(self) -> u32 {
        let mut flags = 0;
        for (m, flag) in [(MOD_SHIFT, 1), (MOD_CONTROL, 2), (MOD_ALT, 4)] {
            if self.mods & m != 0 {
                flags |= flag;
            }
        }
        if matches!(self.vk, 0x21..=0x28 | 0x2D | 0x2E) {
            flags |= 8;
        }
        (flags << 8) | (self.vk & 0xFF)
    }

    /// Human-readable form for logs and tooltips.
    pub fn describe(&self) -> String {
        if !self.is_set() {
            return "None".to_string();
        }
        let mut parts = Vec::new();
        for (flag, name) in [
            (MOD_CONTROL, "Ctrl"),
            (MOD_ALT, "Alt"),
            (MOD_SHIFT, "Shift"),
            (MOD_WIN, "Win"),
        ] {
            if self.mods & flag != 0 {
                parts.push(name.to_string());
            }
        }
        parts.push(match self.vk {
            0x26 => "Up".to_string(),
            0x28 => "Down".to_string(),
            0x25 => "Left".to_string(),
            0x27 => "Right".to_string(),
            0x21 => "PgUp".to_string(),
            0x22 => "PgDn".to_string(),
            0x70..=0x87 => format!("F{}", self.vk - 0x6F),
            v if (0x30..=0x39).contains(&v) || (0x41..=0x5A).contains(&v) => {
                char::from(v as u8).to_string()
            }
            v => format!("#{v}"),
        });
        parts.join("+")
    }
}

#[derive(Clone)]
pub struct Settings {
    pub kind: ChannelKind,
    pub index: i32,
    pub orientation: Orientation,
    pub position: Position,
    pub accent: (u8, u8, u8),
    pub accent_follow_system: bool,
    pub opacity_pct: u8,
    pub hide_ms: u32,
    pub min_db: f32,
    pub max_db: f32,
    pub hide_in_fullscreen: bool,
    pub debug_log: bool,
    /// Follow whichever strip or bus moves, rather than one fixed channel.
    pub watch_all: bool,
    pub hotkeys_enabled: bool,
    pub hotkey_up: Hotkey,
    pub hotkey_down: Hotkey,
    pub hotkey_mute: Hotkey,
    /// dB moved per wheel notch or hotkey press.
    pub step_db: f32,
    /// Scroll over the tray icon to change volume (needs a global mouse hook).
    pub tray_scroll: bool,
    pub custom_x: i32,
    pub custom_y: i32,
}

impl Settings {
    /// Accent actually used for drawing, honouring the follow-system option.
    pub fn effective_accent(&self) -> (u8, u8, u8) {
        if self.accent_follow_system {
            system_accent().unwrap_or(self.accent)
        } else {
            self.accent
        }
    }
}

/// Windows' current accent colour via DWM colorization.
pub fn system_accent() -> Option<(u8, u8, u8)> {
    unsafe {
        let mut color: u32 = 0;
        let mut opaque: i32 = 0;
        if DwmGetColorizationColor(&mut color, &mut opaque) != 0 {
            return None;
        }
        Some(((color >> 16) as u8, (color >> 8) as u8, color as u8))
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            // Confirmed setup: keyboard volume knob drives Bus[0]/Bus[1] (A1/A2).
            kind: ChannelKind::Bus,
            index: 0,
            orientation: Orientation::Vertical,
            position: Position::AboveTray,
            accent: (70, 160, 255),
            accent_follow_system: false,
            opacity_pct: 92,
            hide_ms: 1500,
            min_db: -60.0,
            max_db: 12.0,
            hide_in_fullscreen: true,
            debug_log: false,
            watch_all: false,
            hotkeys_enabled: false,
            hotkey_up: Hotkey {
                mods: MOD_CONTROL | MOD_ALT,
                vk: 0x26,
            },
            hotkey_down: Hotkey {
                mods: MOD_CONTROL | MOD_ALT,
                vk: 0x28,
            },
            hotkey_mute: Hotkey {
                mods: MOD_CONTROL | MOD_ALT,
                vk: 'M' as u32,
            },
            step_db: 1.0,
            tray_scroll: true,
            custom_x: 0,
            custom_y: 0,
        }
    }
}

/// Edition layout: 1 = Voicemeeter, 2 = Banana, 3 = Potato.
pub fn physical_strips(edition: i32) -> i32 {
    match edition {
        3 => 5,
        2 => 3,
        _ => 2,
    }
}

pub fn physical_buses(edition: i32) -> i32 {
    match edition {
        3 => 5,
        2 => 3,
        _ => 1,
    }
}

pub fn strip_count(edition: i32) -> i32 {
    match edition {
        3 => 8,
        2 => 5,
        _ => 3,
    }
}

pub fn bus_count(edition: i32) -> i32 {
    match edition {
        3 => 8,
        2 => 5,
        _ => 2,
    }
}

fn settings_path() -> PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
    let mut p = PathBuf::from(base);
    p.push("voicemeeter-osd");
    let _ = std::fs::create_dir_all(&p);
    p.push("settings.txt");
    p
}

impl Settings {
    pub fn load() -> Self {
        match std::fs::read_to_string(settings_path()) {
            Ok(text) => Settings::parse(&text),
            Err(_) => Settings::default(),
        }
    }

    /// Writes to a sibling temp file and renames it over the original, so a
    /// crash mid-write can't leave a truncated settings file behind.
    pub fn save(&self) {
        let path = settings_path();
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, self.serialize()).is_ok() && std::fs::rename(&tmp, &path).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }

    /// Pulls every numeric field back into its valid range. Shared by the
    /// file parser and the settings window.
    pub fn normalize(&mut self) {
        self.index = self.index.clamp(0, 63);
        self.opacity_pct = self.opacity_pct.clamp(20, 100);
        self.hide_ms = self.hide_ms.clamp(200, 10_000);
        if !self.step_db.is_finite() {
            self.step_db = 1.0;
        }
        self.step_db = self.step_db.clamp(0.1, 12.0);
        if !self.min_db.is_finite() {
            self.min_db = FADER_MIN_DB;
        }
        if !self.max_db.is_finite() {
            self.max_db = FADER_MAX_DB;
        }
        self.min_db = self.min_db.clamp(FADER_MIN_DB, FADER_MAX_DB - 1.0);
        self.max_db = self.max_db.clamp(FADER_MIN_DB + 1.0, FADER_MAX_DB);
        // An inverted range would panic downstream in f32::clamp.
        if self.max_db <= self.min_db {
            self.max_db = (self.min_db + 1.0).min(FADER_MAX_DB);
            self.min_db = self.min_db.min(self.max_db - 1.0);
        }
    }

    pub fn parse(text: &str) -> Self {
        let mut s = Settings::default();
        {
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some((k, v)) = line.split_once('=') {
                    let k = k.trim();
                    let v = v.trim();
                    match k {
                        "type" => {
                            s.kind = if v.eq_ignore_ascii_case("bus") {
                                ChannelKind::Bus
                            } else {
                                ChannelKind::Strip
                            }
                        }
                        "index" => {
                            if let Ok(n) = v.parse::<i32>() {
                                s.index = n;
                            }
                        }
                        "orientation" => {
                            s.orientation = if v.eq_ignore_ascii_case("horizontal") {
                                Orientation::Horizontal
                            } else {
                                Orientation::Vertical
                            }
                        }
                        "position" => {
                            // bottom_center/top_center are retired names from
                            // an earlier version; they fold into Centre.
                            s.position = match v {
                                "centre" | "center" | "bottom_center" | "top_center" => {
                                    Position::Centre
                                }
                                "custom" => Position::Custom,
                                _ => Position::AboveTray,
                            }
                        }
                        "accent" => {
                            let parts: Vec<u8> = v
                                .split(',')
                                .filter_map(|x| x.trim().parse::<u8>().ok())
                                .collect();
                            if parts.len() == 3 {
                                s.accent = (parts[0], parts[1], parts[2]);
                            }
                        }
                        "accent_follow_system" => s.accent_follow_system = v == "true",
                        "hide_in_fullscreen" => s.hide_in_fullscreen = v == "true",
                        "debug_log" => s.debug_log = v == "true",
                        "watch_all" => s.watch_all = v == "true",
                        "hotkeys_enabled" => s.hotkeys_enabled = v == "true",
                        "tray_scroll" => s.tray_scroll = v == "true",
                        "hotkey_up" => {
                            if let Some(h) = Hotkey::parse(v) {
                                s.hotkey_up = h;
                            }
                        }
                        "hotkey_down" => {
                            if let Some(h) = Hotkey::parse(v) {
                                s.hotkey_down = h;
                            }
                        }
                        "hotkey_mute" => {
                            if let Some(h) = Hotkey::parse(v) {
                                s.hotkey_mute = h;
                            }
                        }
                        "step_db" => {
                            if let Ok(n) = v.parse() {
                                s.step_db = n;
                            }
                        }
                        "custom_x" => {
                            if let Ok(n) = v.parse() {
                                s.custom_x = n;
                            }
                        }
                        "custom_y" => {
                            if let Ok(n) = v.parse() {
                                s.custom_y = n;
                            }
                        }
                        "opacity" => {
                            if let Ok(n) = v.parse::<u32>() {
                                s.opacity_pct = n.min(100) as u8;
                            }
                        }
                        "hide_ms" => {
                            if let Ok(n) = v.parse::<u64>() {
                                s.hide_ms = n.min(u32::MAX as u64) as u32;
                            }
                        }
                        "min_db" => {
                            if let Ok(n) = v.parse() {
                                s.min_db = n;
                            }
                        }
                        "max_db" => {
                            if let Ok(n) = v.parse() {
                                s.max_db = n;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        s.normalize();
        s
    }

    pub fn serialize(&self) -> String {
        let position = match self.position {
            Position::AboveTray => "above_tray",
            Position::Centre => "centre",
            Position::Custom => "custom",
        };
        let text = format!(
            "type={}\nindex={}\norientation={}\nposition={}\naccent={},{},{}\naccent_follow_system={}\nopacity={}\nhide_ms={}\nmin_db={}\nmax_db={}\nhide_in_fullscreen={}\ndebug_log={}\nwatch_all={}\nhotkeys_enabled={}\nhotkey_up={}\nhotkey_down={}\nhotkey_mute={}\nstep_db={}\ntray_scroll={}\ncustom_x={}\ncustom_y={}\n",
            match self.kind {
                ChannelKind::Strip => "strip",
                ChannelKind::Bus => "bus",
            },
            self.index,
            match self.orientation {
                Orientation::Vertical => "vertical",
                Orientation::Horizontal => "horizontal",
            },
            position,
            self.accent.0,
            self.accent.1,
            self.accent.2,
            self.accent_follow_system,
            self.opacity_pct,
            self.hide_ms,
            self.min_db,
            self.max_db,
            self.hide_in_fullscreen,
            self.debug_log,
            self.watch_all,
            self.hotkeys_enabled,
            self.hotkey_up.serialize(),
            self.hotkey_down.serialize(),
            self.hotkey_mute.serialize(),
            self.step_db,
            self.tray_scroll,
            self.custom_x,
            self.custom_y,
        );
        text
    }

    pub fn prefix_of(kind: ChannelKind) -> &'static str {
        match kind {
            ChannelKind::Strip => "Strip",
            ChannelKind::Bus => "Bus",
        }
    }

    pub fn gain_param(kind: ChannelKind, index: i32) -> String {
        format!("{}[{}].Gain", Self::prefix_of(kind), index)
    }

    pub fn mute_param(kind: ChannelKind, index: i32) -> String {
        format!("{}[{}].Mute", Self::prefix_of(kind), index)
    }

    pub fn label_param(kind: ChannelKind, index: i32) -> String {
        format!("{}[{}].Label", Self::prefix_of(kind), index)
    }

    /// Which level slot to meter: `(kind, first channel)` for the Remote
    /// API's GetLevel. Buses always occupy 8 channels each; input channels
    /// are packed, with physical strips taking 2 and virtual strips 8.
    pub fn level_slot(kind: ChannelKind, index: i32, edition: i32) -> (i32, i32) {
        match kind {
            ChannelKind::Bus => (3, index * 8),
            ChannelKind::Strip => {
                let physical = physical_strips(edition);
                let base = if index < physical {
                    index * 2
                } else {
                    physical * 2 + (index - physical) * 8
                };
                (1, base)
            }
        }
    }

    /// Voicemeeter's own naming: buses are A1..An then B1..Bn, where the
    /// count of physical buses depends on the edition.
    pub fn channel_name(kind: ChannelKind, index: i32, edition: i32) -> String {
        match kind {
            ChannelKind::Strip => format!("Strip {}", index + 1),
            ChannelKind::Bus => {
                let physical = physical_buses(edition);
                if index < physical {
                    format!("A{}", index + 1)
                } else {
                    format!("B{}", index - physical + 1)
                }
            }
        }
    }
}

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const RUN_VALUE: &str = "VoicemeeterOSD";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn is_autostart_enabled() -> bool {
    unsafe {
        let subkey = wide(RUN_KEY);
        let mut hkey: HKEY = std::ptr::null_mut();
        if RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, KEY_READ, &mut hkey) != 0 {
            return false;
        }
        let value_name = wide(RUN_VALUE);
        let mut value_type: REG_VALUE_TYPE = 0;
        let mut len: u32 = 0;
        let ok = RegQueryValueExW(
            hkey,
            value_name.as_ptr(),
            std::ptr::null_mut(),
            &mut value_type,
            std::ptr::null_mut(),
            &mut len,
        );
        RegCloseKey(hkey);
        ok == 0
    }
}

/// Where the app lives once installed, so autostart doesn't depend on a
/// build directory that `cargo clean` can delete.
pub fn install_dir() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(base).join("Programs").join("VoicemeeterOSD")
}

pub fn installed_exe() -> PathBuf {
    install_dir().join("voicemeeter-osd.exe")
}

/// Copies the running binary into the install directory, returning the path
/// autostart should point at. Already-installed copies are left alone.
pub fn ensure_installed() -> std::io::Result<PathBuf> {
    let current = std::env::current_exe()?;
    let target = installed_exe();
    if current == target {
        return Ok(target);
    }
    std::fs::create_dir_all(install_dir())?;
    // A running copy holds a lock; rename it aside so the copy can proceed.
    if target.exists() {
        let backup = target.with_extension("old");
        let _ = std::fs::remove_file(&backup);
        if std::fs::copy(&current, &target).is_err() {
            std::fs::rename(&target, &backup)?;
        } else {
            return Ok(target);
        }
    }
    std::fs::copy(&current, &target)?;
    Ok(target)
}

/// When autostart points at the installed copy but a newer build is the one
/// running, refresh the installed copy so the next boot runs the new version.
/// Only one instance runs at a time, so the installed exe isn't locked.
pub fn refresh_installed_copy() {
    if !is_autostart_enabled() {
        return;
    }
    let Ok(current) = std::env::current_exe() else {
        return;
    };
    let target = installed_exe();
    if current == target || !target.exists() {
        return;
    }
    let modified = |p: &std::path::Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let newer = matches!((modified(&current), modified(&target)), (Some(a), Some(b)) if a > b);
    let differs = match (std::fs::read(&current), std::fs::read(&target)) {
        (Ok(a), Ok(b)) => a != b,
        _ => false,
    };
    if newer && differs {
        match ensure_installed() {
            Ok(_) => crate::log::log_line!("refreshed installed copy at {}", target.display()),
            Err(e) => crate::log::log_line!("could not refresh installed copy: {e}"),
        }
    }
}

/// Every strip then every bus the edition exposes, in display order.
pub fn all_channels(edition: i32) -> Vec<(ChannelKind, i32)> {
    let strips = (0..strip_count(edition)).map(|i| (ChannelKind::Strip, i));
    let buses = (0..bus_count(edition)).map(|i| (ChannelKind::Bus, i));
    strips.chain(buses).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_survive_a_round_trip() {
        let mut original = Settings::default();
        original.kind = ChannelKind::Strip;
        original.index = 4;
        original.orientation = Orientation::Horizontal;
        original.accent = (12, 200, 90);
        original.accent_follow_system = true;
        original.opacity_pct = 55;
        original.hide_ms = 2200;
        original.min_db = -48.0;
        original.max_db = 6.0;
        original.hide_in_fullscreen = false;
        original.debug_log = true;
        original.watch_all = true;
        original.hotkeys_enabled = true;
        original.hotkey_up = Hotkey {
            mods: MOD_SHIFT | MOD_WIN,
            vk: 0x21,
        };
        original.hotkey_mute = Hotkey::NONE;
        original.step_db = 2.5;
        original.tray_scroll = false;
        original.position = Position::Custom;
        original.custom_x = -1400;
        original.custom_y = 640;

        let parsed = Settings::parse(&original.serialize());

        assert!(parsed.kind == ChannelKind::Strip);
        assert_eq!(parsed.index, 4);
        assert!(parsed.orientation == Orientation::Horizontal);
        assert!(parsed.position == Position::Custom);
        assert_eq!(parsed.accent, (12, 200, 90));
        assert!(parsed.accent_follow_system);
        assert_eq!(parsed.opacity_pct, 55);
        assert_eq!(parsed.hide_ms, 2200);
        assert_eq!(parsed.min_db, -48.0);
        assert_eq!(parsed.max_db, 6.0);
        assert!(!parsed.hide_in_fullscreen);
        assert!(parsed.debug_log);
        assert!(parsed.watch_all);
        assert!(parsed.hotkeys_enabled);
        assert_eq!(parsed.hotkey_up, original.hotkey_up);
        assert_eq!(parsed.hotkey_down, original.hotkey_down);
        assert_eq!(parsed.hotkey_mute, Hotkey::NONE);
        assert_eq!(parsed.step_db, 2.5);
        assert!(!parsed.tray_scroll);
        assert_eq!(parsed.custom_x, -1400);
        assert_eq!(parsed.custom_y, 640);
    }

    #[test]
    fn numeric_fields_are_clamped_to_valid_ranges() {
        let parsed =
            Settings::parse("opacity=300\nhide_ms=5\nstep_db=99\nmin_db=-200\nmax_db=80\n");
        assert_eq!(parsed.opacity_pct, 100);
        assert_eq!(parsed.hide_ms, 200);
        assert_eq!(parsed.step_db, 12.0);
        assert_eq!(parsed.min_db, FADER_MIN_DB);
        assert_eq!(parsed.max_db, FADER_MAX_DB);
    }

    #[test]
    fn non_finite_values_fall_back() {
        let parsed = Settings::parse("min_db=NaN\nstep_db=inf\n");
        assert_eq!(parsed.min_db, FADER_MIN_DB);
        assert_eq!(parsed.step_db, 1.0);
    }

    #[test]
    fn malformed_hotkeys_keep_the_default() {
        let parsed = Settings::parse("hotkey_up=banana\nhotkey_down=3\n");
        let defaults = Settings::default();
        assert_eq!(parsed.hotkey_up, defaults.hotkey_up);
        assert_eq!(parsed.hotkey_down, defaults.hotkey_down);
    }

    #[test]
    fn hotkeys_describe_themselves() {
        assert_eq!(Settings::default().hotkey_up.describe(), "Ctrl+Alt+Up");
        assert_eq!(Settings::default().hotkey_mute.describe(), "Ctrl+Alt+M");
        assert_eq!(Hotkey::NONE.describe(), "None");
        let f5 = Hotkey {
            mods: MOD_SHIFT,
            vk: 0x74,
        };
        assert_eq!(f5.describe(), "Shift+F5");
    }

    #[test]
    fn hotkey_control_words_round_trip() {
        let up = Settings::default().hotkey_up;
        // Ctrl(2) | Alt(4) | Ext(8) in the high byte, VK_UP low.
        assert_eq!(up.to_control(), 0x0E26);
        assert_eq!(Hotkey::from_control(up.to_control()), up);
        let mute = Settings::default().hotkey_mute;
        assert_eq!(mute.to_control(), 0x064D);
        assert_eq!(Hotkey::from_control(0), Hotkey::NONE);
    }

    #[test]
    fn channel_list_covers_the_edition() {
        let banana = all_channels(2);
        assert_eq!(banana.len(), 10);
        assert!(banana[0] == (ChannelKind::Strip, 0));
        assert!(banana[5] == (ChannelKind::Bus, 0));
    }

    #[test]
    fn inverted_db_range_is_normalized() {
        let parsed = Settings::parse("min_db=6\nmax_db=-12\n");
        assert!(parsed.max_db > parsed.min_db);
    }

    #[test]
    fn out_of_range_index_is_clamped() {
        assert_eq!(Settings::parse("index=-3\n").index, 0);
        assert_eq!(Settings::parse("index=99\n").index, 63);
    }

    #[test]
    fn unknown_and_malformed_lines_are_ignored() {
        let parsed = Settings::parse("# a comment\n\nnonsense\nfuture_key=1\nindex=3\n");
        assert_eq!(parsed.index, 3);
    }

    #[test]
    fn retired_position_names_still_load() {
        assert!(Settings::parse("position=bottom_center\n").position == Position::Centre);
        assert!(Settings::parse("position=top_center\n").position == Position::Centre);
        assert!(Settings::parse("position=above_tray\n").position == Position::AboveTray);
    }

    #[test]
    fn missing_file_contents_give_defaults() {
        let parsed = Settings::parse("");
        let defaults = Settings::default();
        assert!(parsed.kind == defaults.kind);
        assert_eq!(parsed.index, defaults.index);
        assert_eq!(parsed.hide_ms, defaults.hide_ms);
    }

    #[test]
    fn bus_names_follow_the_edition() {
        // Banana: three physical buses, then the B buses.
        assert_eq!(Settings::channel_name(ChannelKind::Bus, 0, 2), "A1");
        assert_eq!(Settings::channel_name(ChannelKind::Bus, 2, 2), "A3");
        assert_eq!(Settings::channel_name(ChannelKind::Bus, 3, 2), "B1");
        // Potato has five physical buses, so index 3 is still an A bus.
        assert_eq!(Settings::channel_name(ChannelKind::Bus, 3, 3), "A4");
        assert_eq!(Settings::channel_name(ChannelKind::Bus, 5, 3), "B1");
    }

    #[test]
    fn strip_names_are_one_based() {
        assert_eq!(Settings::channel_name(ChannelKind::Strip, 0, 2), "Strip 1");
    }

    #[test]
    fn bus_meters_sit_on_eight_channel_boundaries() {
        assert_eq!(Settings::level_slot(ChannelKind::Bus, 0, 2), (3, 0));
        assert_eq!(Settings::level_slot(ChannelKind::Bus, 2, 2), (3, 16));
    }

    #[test]
    fn strip_meters_account_for_virtual_strip_width() {
        // Banana: strips 0-2 are physical and 2 channels wide.
        assert_eq!(Settings::level_slot(ChannelKind::Strip, 0, 2), (1, 0));
        assert_eq!(Settings::level_slot(ChannelKind::Strip, 2, 2), (1, 4));
        // Strip 3 is the first virtual one, starting after 3 x 2 channels.
        assert_eq!(Settings::level_slot(ChannelKind::Strip, 3, 2), (1, 6));
        // And virtual strips are 8 wide, so the next starts 8 later.
        assert_eq!(Settings::level_slot(ChannelKind::Strip, 4, 2), (1, 14));
    }

    #[test]
    fn parameter_names_match_the_remote_api() {
        assert_eq!(Settings::gain_param(ChannelKind::Bus, 1), "Bus[1].Gain");
        assert_eq!(Settings::mute_param(ChannelKind::Bus, 1), "Bus[1].Mute");
        assert_eq!(Settings::label_param(ChannelKind::Bus, 1), "Bus[1].Label");
        assert_eq!(Settings::gain_param(ChannelKind::Strip, 2), "Strip[2].Gain");
    }
}

pub fn set_autostart(enabled: bool) {
    unsafe {
        let subkey = wide(RUN_KEY);
        let mut hkey: HKEY = std::ptr::null_mut();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            0,
            KEY_SET_VALUE,
            &mut hkey,
        ) != 0
        {
            return;
        }
        let value_name = wide(RUN_VALUE);
        if enabled {
            // Prefer the installed copy; fall back to wherever we're running.
            let exe = ensure_installed().or_else(|_| std::env::current_exe());
            if let Ok(exe) = exe {
                let data = wide(&format!("\"{}\"", exe.display()));
                RegSetValueExW(
                    hkey,
                    value_name.as_ptr(),
                    0,
                    REG_SZ,
                    data.as_ptr() as *const u8,
                    (data.len() * 2) as u32,
                );
            }
        } else {
            RegDeleteValueW(hkey, value_name.as_ptr());
        }
        RegCloseKey(hkey);
    }
}
