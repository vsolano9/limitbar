//! Small Win32 helpers: registry, startup entry, local time text, URLs.

use std::ffi::c_void;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{ERROR_SUCCESS, FILETIME, SYSTEMTIME};
use windows_sys::Win32::Globalization::{GetDateFormatEx, GetTimeFormatEx, TIME_NOSECONDS};
use windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use crate::wide;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_NAME: &str = "Limitbar";

fn reg_dword(subkey: &str, value: &str) -> Option<u32> {
    let mut data = 0u32;
    let mut size = 4u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            wide(subkey).as_ptr(),
            wide(value).as_ptr(),
            RRF_RT_REG_DWORD,
            null_mut(),
            &mut data as *mut u32 as *mut c_void,
            &mut size,
        )
    };
    (rc == ERROR_SUCCESS).then_some(data)
}

fn reg_string(subkey: &str, value: &str) -> Option<String> {
    let mut buf = vec![0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            wide(subkey).as_ptr(),
            wide(value).as_ptr(),
            RRF_RT_REG_SZ,
            null_mut(),
            buf.as_mut_ptr() as *mut c_void,
            &mut size,
        )
    };
    if rc != ERROR_SUCCESS {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]))
}

/// The taskbar follows the system (not app) colour mode.
pub fn light_taskbar() -> bool {
    reg_dword(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize", "SystemUsesLightTheme") == Some(1)
}

/// Windows 11 "Taskbar alignment: Left" puts Start where the widget would go.
pub fn taskbar_left_aligned() -> bool {
    reg_dword(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced", "TaskbarAl") == Some(0)
}

pub fn exe_path() -> String {
    let mut buf = vec![0u16; 1024];
    let n = unsafe { GetModuleFileNameW(null_mut(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
    String::from_utf16_lossy(&buf[..n])
}

pub fn startup_enabled() -> bool {
    let exe = exe_path().to_lowercase();
    reg_string(RUN_KEY, RUN_NAME).is_some_and(|v| v.to_lowercase().contains(&exe))
}

pub fn set_startup(on: bool) {
    unsafe {
        if on {
            let cmd = wide(&format!("\"{}\"", exe_path()));
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                wide(RUN_KEY).as_ptr(),
                wide(RUN_NAME).as_ptr(),
                REG_SZ,
                cmd.as_ptr() as *const c_void,
                (cmd.len() * 2) as u32,
            );
        } else {
            RegDeleteKeyValueW(HKEY_CURRENT_USER, wide(RUN_KEY).as_ptr(), wide(RUN_NAME).as_ptr());
        }
    }
}

fn local_systemtime(unix: i64) -> Option<SYSTEMTIME> {
    let ticks = (unix + 11_644_473_600).checked_mul(10_000_000)? as u64;
    let ft = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
    unsafe {
        let mut utc: SYSTEMTIME = std::mem::zeroed();
        let mut local: SYSTEMTIME = std::mem::zeroed();
        (FileTimeToSystemTime(&ft, &mut utc) != 0 && SystemTimeToTzSpecificLocalTime(null(), &utc, &mut local) != 0).then_some(local)
    }
}

/// Clock time in the user's own format, e.g. `11:59 PM` or `23:59`.
pub fn clock(unix: i64) -> String {
    let Some(st) = local_systemtime(unix) else { return String::new() };
    let mut buf = [0u16; 64];
    let n = unsafe { GetTimeFormatEx(null(), TIME_NOSECONDS, &st, null(), buf.as_mut_ptr(), 64) };
    String::from_utf16_lossy(&buf[..(n.max(1) - 1) as usize])
}

/// `Tue 9:00 PM`, just the clock time when it falls today, or `Tue Oct 27
/// 9:00 PM` when it is six or more days away (a weekday alone is ambiguous).
pub fn day_clock(unix: i64, now: i64) -> String {
    let (Some(st), Some(today)) = (local_systemtime(unix), local_systemtime(now)) else { return String::new() };
    if (st.wYear, st.wMonth, st.wDay) == (today.wYear, today.wMonth, today.wDay) {
        return clock(unix);
    }
    let mut buf = [0u16; 32];
    let fmt = wide(if (unix - now).abs() >= 6 * 86_400 { "ddd MMM d" } else { "ddd" });
    let n = unsafe { GetDateFormatEx(null(), 0, &st, fmt.as_ptr(), buf.as_mut_ptr(), 32, null()) };
    format!("{} {}", String::from_utf16_lossy(&buf[..(n.max(1) - 1) as usize]), clock(unix))
}

pub fn open_url(url: &str) {
    unsafe {
        ShellExecuteW(null_mut(), wide("open").as_ptr(), wide(url).as_ptr(), null(), null(), SW_SHOWNORMAL);
    }
}

/// Opens a text file in its associated editor, or Notepad when `.json` has
/// no association.
pub fn open_text_file(path: &str) {
    unsafe {
        let r = ShellExecuteW(null_mut(), wide("open").as_ptr(), wide(path).as_ptr(), null(), null(), SW_SHOWNORMAL);
        if (r as isize) <= 32 {
            let quoted = wide(&format!("\"{path}\""));
            ShellExecuteW(null_mut(), wide("open").as_ptr(), wide("notepad.exe").as_ptr(), quoted.as_ptr(), null(), SW_SHOWNORMAL);
        }
    }
}
