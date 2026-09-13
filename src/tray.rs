use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{HINSTANCE, HWND, POINT, RECT};
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconGetRect, Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD,
    NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW, NOTIFYICONIDENTIFIER,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::config::is_autostart_enabled;
use crate::{wide, ID_MENU_AUTOSTART, ID_MENU_EXIT, ID_MENU_SETTINGS, TRAY_ID, WM_TRAYICON};

/// Icon resource id emitted by build.rs (`1 ICON "assets/icon.ico"`).
const ICON_RESOURCE_ID: u16 = 1;

pub fn load_app_icon(hinstance: HINSTANCE, size: i32) -> HICON {
    unsafe {
        LoadImageW(
            hinstance,
            ICON_RESOURCE_ID as *const u16,
            IMAGE_ICON,
            size,
            size,
            LR_DEFAULTCOLOR,
        ) as HICON
    }
}

fn notify_data(hwnd: HWND, icon: HICON, tip_text: &str) -> NOTIFYICONDATAW {
    unsafe {
        let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd;
        nid.uID = TRAY_ID;
        nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        nid.uCallbackMessage = WM_TRAYICON;
        nid.hIcon = icon;
        let mut tip = wide(tip_text);
        tip.truncate(nid.szTip.len() - 1);
        tip.push(0);
        nid.szTip[..tip.len()].copy_from_slice(&tip);
        nid
    }
}

/// Live tooltip, e.g. "A1  -26.0 dB (47%)".
pub fn update_tip(hwnd: HWND, icon: HICON, text: &str) {
    let nid = notify_data(hwnd, icon, text);
    unsafe {
        Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

pub fn icon_rect(hwnd: HWND) -> Option<RECT> {
    unsafe {
        let id = NOTIFYICONIDENTIFIER {
            cbSize: std::mem::size_of::<NOTIFYICONIDENTIFIER>() as u32,
            hWnd: hwnd,
            uID: TRAY_ID,
            guidItem: std::mem::zeroed(),
        };
        let mut rc: RECT = std::mem::zeroed();
        if Shell_NotifyIconGetRect(&id, &mut rc) != 0 || rc.right <= rc.left {
            return None;
        }
        Some(rc)
    }
}

pub fn add(hwnd: HWND, icon: HICON) {
    let nid = notify_data(hwnd, icon, "Voicemeeter OSD");
    unsafe {
        Shell_NotifyIconW(NIM_ADD, &nid);
    }
}

/// Screen-space center of our tray icon. When the icon sits in the Windows 11
/// overflow flyout this resolves to the chevron button, which is still where
/// the user sees it come from.
pub fn icon_center(hwnd: HWND) -> Option<(i32, i32)> {
    unsafe {
        let id = NOTIFYICONIDENTIFIER {
            cbSize: std::mem::size_of::<NOTIFYICONIDENTIFIER>() as u32,
            hWnd: hwnd,
            uID: TRAY_ID,
            guidItem: std::mem::zeroed(),
        };
        let mut rc: RECT = std::mem::zeroed();
        if Shell_NotifyIconGetRect(&id, &mut rc) != 0 {
            return None;
        }
        if rc.right <= rc.left || rc.bottom <= rc.top {
            return None;
        }
        Some(((rc.left + rc.right) / 2, (rc.top + rc.bottom) / 2))
    }
}

pub fn remove(hwnd: HWND) {
    unsafe {
        let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd;
        nid.uID = TRAY_ID;
        Shell_NotifyIconW(NIM_DELETE, &nid);
    }
}

pub fn show_menu(hwnd: HWND) {
    unsafe {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return;
        }
        AppendMenuW(
            menu,
            MF_STRING,
            ID_MENU_SETTINGS as usize,
            wide("Settings").as_ptr(),
        );
        AppendMenuW(menu, MF_SEPARATOR, 0, null_mut());
        let autostart = if is_autostart_enabled() {
            MF_STRING | MF_CHECKED
        } else {
            MF_STRING
        };
        AppendMenuW(
            menu,
            autostart,
            ID_MENU_AUTOSTART as usize,
            wide("Start with Windows").as_ptr(),
        );
        AppendMenuW(menu, MF_SEPARATOR, 0, null_mut());
        AppendMenuW(menu, MF_STRING, ID_MENU_EXIT as usize, wide("Exit").as_ptr());

        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        // Without this the menu will not dismiss when clicking away.
        SetForegroundWindow(hwnd);
        TrackPopupMenu(
            menu,
            TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
            pt.x,
            pt.y,
            0,
            hwnd,
            null_mut(),
        );
        PostMessageW(hwnd, WM_NULL, 0, 0);
        DestroyMenu(menu);
    }
}
