// Minimal dynamic FFI bindings to VoicemeeterRemote64.dll (VB-Audio Voicemeeter Remote API).
// Loaded at runtime via LoadLibraryW/GetProcAddress so we don't need an import lib.

use std::ffi::CString;
use std::mem::transmute;
use std::ptr::null_mut;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{FreeLibrary, HMODULE, MAX_PATH};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ,
    KEY_WOW64_32KEY, REG_VALUE_TYPE,
};

type FnLogin = unsafe extern "system" fn() -> i32;
type FnLogout = unsafe extern "system" fn() -> i32;
type FnIsParametersDirty = unsafe extern "system" fn() -> i32;
type FnGetParameterFloat = unsafe extern "system" fn(*const i8, *mut f32) -> i32;
type FnSetParameterFloat = unsafe extern "system" fn(*const i8, f32) -> i32;
type FnGetParameterStringA = unsafe extern "system" fn(*const i8, *mut i8) -> i32;
type FnGetVoicemeeterType = unsafe extern "system" fn(*mut i32) -> i32;
type FnGetLevel = unsafe extern "system" fn(i32, i32, *mut f32) -> i32;

/// Exponential backoff for reconnect attempts. Pure, so it can be tested
/// without a Voicemeeter to talk to.
#[derive(Debug)]
pub struct Backoff {
    min: Duration,
    max: Duration,
    delay: Duration,
    next_at: Option<Instant>,
}

impl Backoff {
    pub fn new(min: Duration, max: Duration) -> Self {
        Backoff {
            min,
            max,
            delay: min,
            next_at: None,
        }
    }

    /// True when an attempt is due at `now`; records the attempt and pushes
    /// the next one out, doubling the wait up to the cap.
    pub fn attempt(&mut self, now: Instant) -> bool {
        if self.next_at.is_some_and(|at| now < at) {
            return false;
        }
        self.next_at = Some(now + self.delay);
        self.delay = (self.delay * 2).min(self.max);
        true
    }

    /// Wait before the attempt after the next one, for logging.
    pub fn delay(&self) -> Duration {
        self.delay
    }

    /// Back to eager retries once things are healthy again.
    pub fn reset(&mut self) {
        self.delay = self.min;
        self.next_at = None;
    }
}

/// Outcome of a cache pump.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Link {
    Up,
    NoServer,
    Stale,
}

pub struct VmrApi {
    module: HMODULE,
    login: FnLogin,
    logout: FnLogout,
    is_dirty: FnIsParametersDirty,
    get_float: FnGetParameterFloat,
    set_float: FnSetParameterFloat,
    get_string: FnGetParameterStringA,
    get_type: FnGetVoicemeeterType,
    get_level: FnGetLevel,
    logged_in: bool,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Read the Voicemeeter install directory from the registry, falling back to
/// the well-known default install path used by all Voicemeeter variants.
fn find_dll_path() -> String {
    unsafe {
        // The uninstaller entry is the only reliable pointer to the install
        // dir; Voicemeeter writes no path key of its own. One key covers all
        // editions, since only one can be installed at a time.
        let subkey = wide(
            "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\VB:Voicemeeter {17359A74-1236-5467}",
        );
        let mut hkey: HKEY = null_mut();
        let opened = RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            subkey.as_ptr(),
            0,
            KEY_READ | KEY_WOW64_32KEY,
            &mut hkey,
        );
        if opened == 0 {
            let value_name = wide("UninstallString");
            let mut buf = [0u16; MAX_PATH as usize];
            let mut buf_len: u32 = (buf.len() * 2) as u32;
            let mut value_type: REG_VALUE_TYPE = 0;
            let ok = RegQueryValueExW(
                hkey,
                value_name.as_ptr(),
                null_mut(),
                &mut value_type,
                buf.as_mut_ptr() as *mut u8,
                &mut buf_len,
            );
            RegCloseKey(hkey);
            if ok == 0 {
                let len = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
                let raw = String::from_utf16_lossy(&buf[..len]);
                // May be wrapped in quotes, optionally with trailing switches.
                let exe_path = raw
                    .trim()
                    .trim_start_matches('"')
                    .split('"')
                    .next()
                    .unwrap_or("")
                    .trim_end();
                if let Some(dir) = exe_path.rfind('\\').map(|i| &exe_path[..i]) {
                    let candidate = format!("{dir}\\VoicemeeterRemote64.dll");
                    if std::path::Path::new(&candidate).exists() {
                        return candidate;
                    }
                }
            }
        }
    }
    // Fallback: every Voicemeeter edition installs here by default.
    "C:\\Program Files (x86)\\VB\\Voicemeeter\\VoicemeeterRemote64.dll".to_string()
}

impl VmrApi {
    pub fn load() -> Result<Self, String> {
        let path = find_dll_path();
        let wpath = wide(&path);
        let module = unsafe { LoadLibraryW(wpath.as_ptr()) };
        if module.is_null() {
            return Err(format!("failed to load {path}"));
        }

        macro_rules! proc {
            ($name:literal, $ty:ty) => {{
                let cname = CString::new($name).unwrap();
                let addr = unsafe { GetProcAddress(module, cname.as_ptr() as *const u8) };
                match addr {
                    Some(f) => unsafe { transmute::<unsafe extern "system" fn() -> isize, $ty>(f) },
                    None => {
                        unsafe { FreeLibrary(module) };
                        return Err(format!("missing export {}", $name));
                    }
                }
            }};
        }

        let login = proc!("VBVMR_Login", FnLogin);
        let logout = proc!("VBVMR_Logout", FnLogout);
        let is_dirty = proc!("VBVMR_IsParametersDirty", FnIsParametersDirty);
        let get_float = proc!("VBVMR_GetParameterFloat", FnGetParameterFloat);
        let set_float = proc!("VBVMR_SetParameterFloat", FnSetParameterFloat);
        let get_string = proc!("VBVMR_GetParameterStringA", FnGetParameterStringA);
        let get_type = proc!("VBVMR_GetVoicemeeterType", FnGetVoicemeeterType);
        let get_level = proc!("VBVMR_GetLevel", FnGetLevel);

        let login_result = unsafe { login() };
        // 0 = ok, 1 = ok but Voicemeeter app not running yet (API still usable once it starts)
        let logged_in = login_result == 0 || login_result == 1;

        Ok(Self {
            module,
            login,
            logout,
            is_dirty,
            get_float,
            set_float,
            get_string,
            get_type,
            get_level,
            logged_in,
        })
    }

    pub fn is_logged_in(&self) -> bool {
        self.logged_in
    }

    /// Pumps the Remote API's parameter cache. `VBVMR_GetParameter*` serves
    /// cached values that only refresh when this is called, so it must run
    /// every poll tick.
    pub fn pump(&self) -> Link {
        match unsafe { (self.is_dirty)() } {
            rc if rc >= 0 => Link::Up,
            // -2: Voicemeeter itself isn't running. Logging in again won't
            // help until it starts, so callers should back off.
            -2 => Link::NoServer,
            // -1 and anything else: our session went stale (Voicemeeter
            // restarted, or a previous client never logged out).
            _ => Link::Stale,
        }
    }

    /// Re-establishes the session after the server went away.
    pub fn relogin(&mut self) {
        unsafe {
            (self.logout)();
            let rc = (self.login)();
            self.logged_in = rc == 0 || rc == 1;
        }
    }

    pub fn get_float(&self, param: &str) -> Option<f32> {
        let cparam = CString::new(param).ok()?;
        self.get_float_c(&cparam)
    }

    /// Same read with a pre-built `CString`, for the per-tick channel scan
    /// where allocating a string per parameter would add up.
    pub fn get_float_c(&self, param: &std::ffi::CStr) -> Option<f32> {
        let mut value: f32 = 0.0;
        let rc = unsafe { (self.get_float)(param.as_ptr(), &mut value) };
        if rc == 0 {
            Some(value)
        } else {
            None
        }
    }

    pub fn set_float(&self, param: &str, value: f32) -> bool {
        let Ok(cparam) = CString::new(param) else {
            return false;
        };
        self.set_float_c(&cparam, value)
    }

    pub fn set_float_c(&self, param: &std::ffi::CStr, value: f32) -> bool {
        unsafe { (self.set_float)(param.as_ptr(), value) == 0 }
    }

    /// Reads a string parameter such as `Bus[0].Label`.
    pub fn get_string(&self, param: &str) -> Option<String> {
        let cparam = CString::new(param).ok()?;
        // The API writes up to 512 bytes into the caller's buffer.
        let mut buf = [0i8; 512];
        let rc = unsafe { (self.get_string)(cparam.as_ptr(), buf.as_mut_ptr()) };
        if rc != 0 {
            return None;
        }
        let bytes: Vec<u8> = buf
            .iter()
            .take_while(|b| **b != 0)
            .map(|b| *b as u8)
            .collect();
        let text = String::from_utf8_lossy(&bytes).trim().to_string();
        (!text.is_empty()).then_some(text)
    }

    /// 1 = Voicemeeter, 2 = Banana, 3 = Potato; `None` while Voicemeeter
    /// isn't running, so callers can retry instead of trusting a guess.
    pub fn edition(&self) -> Option<i32> {
        let mut kind = 0i32;
        unsafe {
            if (self.get_type)(&mut kind) != 0 {
                return None;
            }
            match kind {
                1..=3 => Some(kind),
                // The header lists 6 for Potato running as a 64-bit process.
                6 => Some(3),
                _ => None,
            }
        }
    }

    /// Live audio level as linear amplitude. `kind` follows the Remote API:
    /// 0-2 are input stages, 3 is bus output.
    pub fn level(&self, kind: i32, channel: i32) -> Option<f32> {
        let mut value = 0.0f32;
        unsafe {
            if (self.get_level)(kind, channel, &mut value) == 0 {
                Some(value)
            } else {
                None
            }
        }
    }

    /// Loudest of a stereo pair, which is what a single meter should show.
    pub fn stereo_level(&self, kind: i32, base: i32) -> Option<f32> {
        let left = self.level(kind, base)?;
        let right = self.level(kind, base + 1).unwrap_or(left);
        Some(left.max(right))
    }

    /// Brings the Voicemeeter window to the front.
    pub fn show_voicemeeter(&self) {
        self.set_float("Command.Show", 1.0);
    }
}

impl Drop for VmrApi {
    fn drop(&mut self) {
        unsafe {
            (self.logout)();
            FreeLibrary(self.module);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backoff() -> Backoff {
        Backoff::new(Duration::from_secs(2), Duration::from_secs(10))
    }

    #[test]
    fn first_attempt_is_immediate() {
        assert!(backoff().attempt(Instant::now()));
    }

    #[test]
    fn attempts_wait_for_the_delay() {
        let mut b = backoff();
        let t0 = Instant::now();
        assert!(b.attempt(t0));
        assert!(!b.attempt(t0 + Duration::from_secs(1)));
        assert!(b.attempt(t0 + Duration::from_secs(2)));
    }

    #[test]
    fn delay_doubles_up_to_the_cap() {
        let mut b = backoff();
        let mut now = Instant::now();
        let mut waits = Vec::new();
        for _ in 0..5 {
            assert!(b.attempt(now));
            let next = b.next_at.unwrap();
            waits.push((next - now).as_secs());
            now = next;
        }
        assert_eq!(waits, [2, 4, 8, 10, 10]);
    }

    #[test]
    fn reset_makes_retries_eager_again() {
        let mut b = backoff();
        let t0 = Instant::now();
        b.attempt(t0);
        b.attempt(t0 + Duration::from_secs(2));
        b.reset();
        assert!(b.attempt(t0 + Duration::from_secs(3)));
        assert_eq!(b.delay(), Duration::from_secs(4));
    }
}
