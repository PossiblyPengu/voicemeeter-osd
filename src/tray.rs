use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{HINSTANCE, HWND, POINT, RECT};
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconGetRect, Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD,
    NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW, NOTIFYICONIDENTIFIER,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::config::{self, is_autostart_enabled, Settings};
use crate::{
    settings_ui, wide, AppState, ID_MENU_AUTOSTART, ID_MENU_CHANNEL_COUNT, ID_MENU_CHANNEL_FIRST,
    ID_MENU_EXIT, ID_MENU_MUTE, ID_MENU_SETTINGS, ID_MENU_VOICEMEETER, TRAY_ID, WM_TRAYICON,
};

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
        if Shell_NotifyIconGetRect(&id, &mut rc) != 0 || rc.right <= rc.left || rc.bottom <= rc.top
        {
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
    icon_rect(hwnd).map(|rc| ((rc.left + rc.right) / 2, (rc.top + rc.bottom) / 2))
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

/// Menu text with `&` escaped, so a Voicemeeter label like "Mic & Line"
/// doesn't turn into a keyboard mnemonic.
fn menu_text(text: &str) -> Vec<u16> {
    wide(&text.replace('&', "&&"))
}

pub fn show_menu(hwnd: HWND, app: &AppState) {
    unsafe {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return;
        }
        let (kind, index) = (app.settings.kind, app.settings.index);

        // Mute state of the configured channel, which is what the item acts
        // on, even if the bar is currently following another one.
        let muted = app
            .vmr
            .get_float(&Settings::mute_param(kind, index))
            .is_some_and(|m| m >= 0.5);
        let caption = settings_ui::channel_caption(kind, index, app.edition);
        AppendMenuW(
            menu,
            MF_STRING | if muted { MF_CHECKED } else { MF_UNCHECKED },
            ID_MENU_MUTE as usize,
            menu_text(&format!("Mute {caption}")).as_ptr(),
        );

        let channels = CreatePopupMenu();
        if !channels.is_null() {
            for (item, (k, i)) in config::all_channels(app.edition)
                .into_iter()
                .take(ID_MENU_CHANNEL_COUNT)
                .enumerate()
            {
                let checked = if (k, i) == (kind, index) {
                    MF_CHECKED
                } else {
                    MF_UNCHECKED
                };
                AppendMenuW(
                    channels,
                    MF_STRING | checked,
                    ID_MENU_CHANNEL_FIRST as usize + item,
                    menu_text(&settings_ui::channel_caption(k, i, app.edition)).as_ptr(),
                );
            }
            // The menu owns the submenu once appended, and destroys it too.
            AppendMenuW(menu, MF_POPUP, channels as usize, wide("Channel").as_ptr());
        }
        AppendMenuW(
            menu,
            MF_STRING,
            ID_MENU_VOICEMEETER as usize,
            wide("Open Voicemeeter").as_ptr(),
        );
        AppendMenuW(menu, MF_SEPARATOR, 0, null_mut());

        AppendMenuW(
            menu,
            MF_STRING,
            ID_MENU_SETTINGS as usize,
            wide("Settings").as_ptr(),
        );
        SetMenuDefaultItem(menu, ID_MENU_SETTINGS as u32, 0);
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
        AppendMenuW(
            menu,
            MF_STRING,
            ID_MENU_EXIT as usize,
            wide("Exit").as_ptr(),
        );

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
