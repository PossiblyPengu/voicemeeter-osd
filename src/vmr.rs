// Minimal dynamic FFI bindings to VoicemeeterRemote64.dll (VB-Audio Voicemeeter Remote API).
// Loaded at runtime via LoadLibraryW/GetProcAddress so we don't need an import lib.

use std::ffi::CString;
use std::mem::transmute;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{FreeLibrary, HMODULE, MAX_PATH};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY,
    REG_VALUE_TYPE,
};

type FnLogin = unsafe extern "system" fn() -> i32;
type FnLogout = unsafe extern "system" fn() -> i32;
type FnIsParametersDirty = unsafe extern "system" fn() -> i32;
type FnGetParameterFloat = unsafe extern "system" fn(*const i8, *mut f32) -> i32;
type FnSetParameterFloat = unsafe extern "system" fn(*const i8, f32) -> i32;
type FnGetParameterStringA = unsafe extern "system" fn(*const i8, *mut i8) -> i32;
type FnGetVoicemeeterType = unsafe extern "system" fn(*mut i32) -> i32;

pub struct VmrApi {
    module: HMODULE,
    login: FnLogin,
    logout: FnLogout,
    is_dirty: FnIsParametersDirty,
    get_float: FnGetParameterFloat,
    set_float: FnSetParameterFloat,
    get_string: FnGetParameterStringA,
    get_type: FnGetVoicemeeterType,
    logged_in: bool,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Read the Voicemeeter install directory from the registry, falling back to
/// the well-known default install path used by all Voicemeeter variants.
fn find_dll_path() -> String {
    unsafe {
        let subkey = wide("SOFTWARE\\VB:Audio\\Voicemeeter");
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
                let len = (buf_len as usize / 2).saturating_sub(1);
                let path = String::from_utf16_lossy(&buf[..len]);
                if let Some(dir) = path.rfind('\\').map(|i| &path[..i]) {
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
            ($name:literal) => {{
                let cname = CString::new($name).unwrap();
                let addr = unsafe { GetProcAddress(module, cname.as_ptr() as *const u8) };
                match addr {
                    Some(f) => unsafe { transmute(f) },
                    None => {
                        unsafe { FreeLibrary(module) };
                        return Err(format!("missing export {}", $name));
                    }
                }
            }};
        }

        let login: FnLogin = proc!("VBVMR_Login");
        let logout: FnLogout = proc!("VBVMR_Logout");
        let is_dirty: FnIsParametersDirty = proc!("VBVMR_IsParametersDirty");
        let get_float: FnGetParameterFloat = proc!("VBVMR_GetParameterFloat");
        let set_float: FnSetParameterFloat = proc!("VBVMR_SetParameterFloat");
        let get_string: FnGetParameterStringA = proc!("VBVMR_GetParameterStringA");
        let get_type: FnGetVoicemeeterType = proc!("VBVMR_GetVoicemeeterType");

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
            logged_in,
        })
    }

    pub fn is_logged_in(&self) -> bool {
        self.logged_in
    }

    /// Pumps the Remote API's parameter cache. `VBVMR_GetParameter*` serves
    /// cached values that only refresh when this is called, so it must run
    /// every poll tick. A negative result means the connection went stale
    /// (Voicemeeter restarted, or a previous client never logged out).
    pub fn pump(&self) -> bool {
        unsafe { (self.is_dirty)() >= 0 }
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
        let mut value: f32 = 0.0;
        let rc = unsafe { (self.get_float)(cparam.as_ptr(), &mut value) };
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
        unsafe { (self.set_float)(cparam.as_ptr(), value) == 0 }
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

    /// 1 = Voicemeeter, 2 = Banana, 3 = Potato.
    pub fn edition(&self) -> i32 {
        let mut kind = 0i32;
        unsafe {
            if (self.get_type)(&mut kind) == 0 {
                kind
            } else {
                1
            }
        }
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
