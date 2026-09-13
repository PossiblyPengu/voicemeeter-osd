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
        Some((
            (color >> 16) as u8,
            (color >> 8) as u8,
            color as u8,
        ))
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
        }
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
        let mut s = Settings::default();
        if let Ok(text) = std::fs::read_to_string(settings_path()) {
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
                            if let Ok(n) = v.parse() {
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
                        "opacity" => {
                            if let Ok(n) = v.parse() {
                                s.opacity_pct = n;
                            }
                        }
                        "hide_ms" => {
                            if let Ok(n) = v.parse() {
                                s.hide_ms = n;
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
        s
    }

    pub fn save(&self) {
        let position = match self.position {
            Position::AboveTray => "above_tray",
            Position::Centre => "centre",
        };
        let text = format!(
            "type={}\nindex={}\norientation={}\nposition={}\naccent={},{},{}\naccent_follow_system={}\nopacity={}\nhide_ms={}\nmin_db={}\nmax_db={}\nhide_in_fullscreen={}\n",
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
        );
        let _ = std::fs::write(settings_path(), text);
    }

    fn prefix(&self) -> &'static str {
        match self.kind {
            ChannelKind::Strip => "Strip",
            ChannelKind::Bus => "Bus",
        }
    }

    pub fn gain_param(&self) -> String {
        format!("{}[{}].Gain", self.prefix(), self.index)
    }

    pub fn mute_param(&self) -> String {
        format!("{}[{}].Mute", self.prefix(), self.index)
    }

    pub fn label_param(&self) -> String {
        format!("{}[{}].Label", self.prefix(), self.index)
    }

    /// Voicemeeter's own naming: buses are A1..An then B1..Bn, where the
    /// count of physical buses depends on the edition.
    pub fn channel_name(&self, edition: i32) -> String {
        match self.kind {
            ChannelKind::Strip => format!("Strip {}", self.index + 1),
            ChannelKind::Bus => {
                let physical = match edition {
                    3 => 5, // Potato
                    2 => 3, // Banana
                    _ => 1,
                };
                if self.index < physical {
                    format!("A{}", self.index + 1)
                } else {
                    format!("B{}", self.index - physical + 1)
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

pub fn set_autostart(enabled: bool) {
    unsafe {
        let subkey = wide(RUN_KEY);
        let mut hkey: HKEY = std::ptr::null_mut();
        if RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, KEY_SET_VALUE, &mut hkey) != 0 {
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
