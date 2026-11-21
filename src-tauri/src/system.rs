//! Windows odds and ends: L-Connect on/off, start with Windows, own memory use, single instance.

#[cfg(windows)]
use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};

/// True if any L-Connect process is running.
pub fn lconnect_running() -> bool {
    process_running(|n| n.starts_with("l-connect"))
}

/// Start MSI Afterburner (and with it RivaTuner) through the startup tasks they make for
/// "Start with Windows". Those tasks run them elevated without a UAC prompt, which a plain launch
/// can't do. Returns which tasks were started.
pub fn start_monitoring_tools() -> Vec<&'static str> {
    let mut out = Vec::new();
    #[cfg(windows)]
    for (task, exe) in [("MSIAfterburner", "msiafterburner.exe"), ("RTSS", "rtss.exe")] {
        if process_running(|n| n == exe) {
            continue;
        }
        use std::os::windows::process::CommandExt;
        let ok = std::process::Command::new("schtasks")
            .args(["/run", "/tn", task])
            .creation_flags(0x0800_0000)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if ok {
            out.push(task);
        }
    }
    out
}

/// True if a process whose lowercase exe name matches is running.
pub fn process_running(want: impl Fn(&str) -> bool) -> bool {
    #[cfg(windows)]
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap.is_null() || snap as isize == -1 {
            return false;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut found = false;
        if Process32FirstW(snap, &mut e) != 0 {
            loop {
                let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
                let name = String::from_utf16_lossy(&e.szExeFile[..len]).to_lowercase();
                if want(&name) {
                    found = true;
                    break;
                }
                if Process32NextW(snap, &mut e) == 0 {
                    break;
                }
            }
        }
        windows_sys::Win32::Foundation::CloseHandle(snap);
        found
    }
    #[cfg(not(windows))]
    false
}

/// Turn L-Connect 3 off (stop + disable its services and startup task) or back on.
/// Needs admin, so Windows shows one UAC prompt.
pub fn set_lconnect(on: bool) -> Result<(), String> {
    let script = if on {
        "Set-Service LConnectService -StartupType Automatic; Set-Service LConnectServiceWatcher -StartupType Automatic; \
         Start-Service LConnectService; Start-Service LConnectServiceWatcher; \
         Enable-ScheduledTask -TaskPath '\\LianLi\\' -TaskName 'L-Connect 3' | Out-Null; Start-ScheduledTask -TaskPath '\\LianLi\\' -TaskName 'L-Connect 3'"
    } else {
        "Stop-Service LConnectServiceWatcher -Force; Stop-Process -Name 'L-Connect 3' -Force -ErrorAction SilentlyContinue; Stop-Service LConnectService -Force; \
         Set-Service LConnectServiceWatcher -StartupType Disabled; Set-Service LConnectService -StartupType Disabled; \
         Disable-ScheduledTask -TaskPath '\\LianLi\\' -TaskName 'L-Connect 3' | Out-Null"
    };
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::Shell::ShellExecuteW;
        let w = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
        let args = format!("-NoProfile -WindowStyle Hidden -Command \"{script}\"");
        let r = unsafe { ShellExecuteW(std::ptr::null_mut(), w("runas").as_ptr(), w("powershell.exe").as_ptr(), w(&args).as_ptr(), std::ptr::null(), 0) };
        if (r as isize) <= 32 {
            return Err("Windows didn't run the change. The admin prompt may have been cancelled.".into());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = script;
        Err("only on Windows".into())
    }
}

pub fn set_autostart(on: bool) {
    #[cfg(windows)]
    {
        use winreg::enums::*;
        let Ok((k, _)) = winreg::RegKey::predef(HKEY_CURRENT_USER).create_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Run") else { return };
        if on {
            if let Ok(exe) = std::env::current_exe() {
                let _ = k.set_value("Vane", &format!("\"{}\" --tray", exe.display()));
            }
        } else {
            let _ = k.delete_value("Vane");
        }
    }
    #[cfg(not(windows))]
    let _ = on;
}

/// Working set of this process in MB.
pub fn memory_mb() -> f64 {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        let mut c: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        if GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) != 0 {
            return (c.WorkingSetSize as f64 / 1048576.0 * 10.0).round() / 10.0;
        }
        0.0
    }
    #[cfg(not(windows))]
    0.0
}

/// Single instance: returns false if Vane is already running (and asks that copy to show its window).
#[cfg(windows)]
pub fn claim_single_instance() -> bool {
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::{CreateEventW, CreateMutexW, OpenEventW, SetEvent, EVENT_MODIFY_STATE};
    let w = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    unsafe {
        let _m = CreateMutexW(std::ptr::null(), 0, w("Local\\VaneSingleInstance").as_ptr());
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let e = OpenEventW(EVENT_MODIFY_STATE, 0, w("Local\\VaneShowWindow").as_ptr());
            // this launch owns the foreground (the user just started it); let the running copy take it
            windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(windows_sys::Win32::UI::WindowsAndMessaging::ASFW_ANY);
            if !e.is_null() {
                SetEvent(e);
            }
            return false;
        }
        let _ = CreateEventW(std::ptr::null(), 0, 0, w("Local\\VaneShowWindow").as_ptr());
    }
    true
}

/// Blocks until another launch asks this copy to show its window.
#[cfg(windows)]
pub fn wait_show_request() {
    use windows_sys::Win32::System::Threading::{OpenEventW, WaitForSingleObject, INFINITE, SYNCHRONIZATION_SYNCHRONIZE};
    let w: Vec<u16> = "Local\\VaneShowWindow".encode_utf16().chain([0]).collect();
    unsafe {
        let e = OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, 0, w.as_ptr());
        if e.is_null() {
            std::thread::sleep(std::time::Duration::from_secs(3600));
            return;
        }
        WaitForSingleObject(e, INFINITE);
    }
}
