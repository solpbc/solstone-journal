// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The Windows shell calls the app makes: opening a page or a folder, the
//! folder picker, the admin terminal, the app's own sign-in entry, its icon
//! on the window and on the Start-menu entry, and one window per sign-in.

use std::ffi::{OsStr, c_void};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree, IPersistFile, STGM_READWRITE,
};
use windows::Win32::UI::Shell::{
    FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, FileOpenDialog, IFileOpenDialog, IShellItem, IShellLinkW,
    SHCNE_ASSOCCHANGED, SHCNE_UPDATEITEM, SHCNF_IDLIST, SHCNF_PATHW, SHChangeNotify,
    SHCreateItemFromParsingName, SIGDN_FILESYSPATH, ShellLink,
};
use windows::core::{Interface, PCWSTR};
use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteValueW,
    RegSetValueExW,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, CreateMutexW, EVENT_MODIFY_STATE, INFINITE, OpenEventW, SetEvent,
    WaitForSingleObject,
};
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, GetSystemMetrics, ICON_BIG, ICON_SMALL, IMAGE_ICON, LR_DEFAULTCOLOR,
    LR_LOADFROMFILE, LoadImageW, MB_ICONERROR, MB_OK, MessageBoxW, SM_CXICON, SM_CXSMICON,
    SW_SHOWNORMAL, SendMessageW, WM_SETICON,
};

fn wide(value: impl AsRef<OsStr>) -> Vec<u16> {
    value
        .as_ref()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// Open a web page in the owner's browser, or a folder in Explorer.
pub fn open(target: &str) -> Result<(), String> {
    let verb = wide("open");
    let file = wide(target);
    // SAFETY: both strings are NUL-terminated and outlive the call; the other
    // pointers are optional and null.
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecute reports success as a value above 32.
    if result as usize > 32 {
        Ok(())
    } else {
        Err(format!("windows couldn't open {target}"))
    }
}

/// A plain Windows message box, for a failure that leaves no window to say it in.
pub fn alert(title: &str, text: &str) {
    let title = wide(title);
    let text = wide(text);
    // SAFETY: both strings are NUL-terminated and outlive the modal call.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR,
        )
    };
}

/// The system folder picker, starting at `start` when it exists.
pub fn pick_folder(owner: isize, start: Option<&Path>) -> Result<Option<PathBuf>, String> {
    // SAFETY: COM is initialized on this (the window's) thread by the window
    // library; every interface is released when it drops, and the returned
    // path buffer is freed with CoTaskMemFree as the API requires.
    unsafe {
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)
            .map_err(|error| error.to_string())?;
        let options = dialog.GetOptions().map_err(|error| error.to_string())?;
        dialog
            .SetOptions(options | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM)
            .map_err(|error| error.to_string())?;
        if let Some(start) = start.filter(|path| path.is_dir()) {
            let start = wide(start);
            if let Ok(item) =
                SHCreateItemFromParsingName::<_, _, IShellItem>(PCWSTR(start.as_ptr()), None)
            {
                let _ = dialog.SetFolder(&item);
            }
        }
        if dialog.Show(Some(HWND(owner as *mut c_void))).is_err() {
            // Cancel comes back as an error; it is not one.
            return Ok(None);
        }
        let item = dialog.GetResult().map_err(|error| error.to_string())?;
        let name = item
            .GetDisplayName(SIGDN_FILESYSPATH)
            .map_err(|error| error.to_string())?;
        let path = PathBuf::from(std::ffi::OsString::from_wide(name.as_wide()));
        CoTaskMemFree(Some(name.0 as *const c_void));
        Ok(Some(path))
    }
}

/// A PowerShell window where `journal` and `solstone` resolve first to this
/// app's own command line, as the Mac app's admin terminal does. Nothing is
/// installed: the path is set for that window only.
pub fn open_admin_terminal(bin: &Path, home: &Path) -> Result<(), String> {
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
    let literal = |value: &Path| format!("'{}'", value.display().to_string().replace('\'', "''"));
    let script = format!(
        "$env:Path = {} + ';' + $env:Path; Set-Location -LiteralPath {}",
        literal(bin),
        literal(home)
    );
    Command::new("powershell.exe")
        .args(["-NoExit", "-NoLogo", "-Command", &script])
        .creation_flags(CREATE_NEW_CONSOLE)
        .spawn()
        .map(drop)
        .map_err(|error| format!("couldn't open a terminal: {error}"))
}

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "SolstoneJournalApp";

/// The app's own entry in the owner's sign-in list (the Windows counterpart
/// of the Mac app's login item). It opens the app in the taskbar when the
/// owner signs in; the journal itself starts from its scheduled task.
pub fn set_sign_in_launch(on: bool) -> Result<(), String> {
    let path = wide(RUN_KEY);
    let name = wide(RUN_VALUE);
    let mut key: HKEY = std::ptr::null_mut();
    // SAFETY: the key path is NUL-terminated and `key` is output storage.
    let opened = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            path.as_ptr(),
            0,
            std::ptr::null(),
            0,
            KEY_SET_VALUE,
            std::ptr::null(),
            &mut key,
            std::ptr::null_mut(),
        )
    };
    if opened != 0 {
        return Err(format!(
            "couldn't open the sign-in list (windows error {opened})"
        ));
    }
    let result = if on {
        let exe = std::env::current_exe().map_err(|error| error.to_string())?;
        let command = wide(format!("\"{}\" --sign-in", exe.display()));
        // SAFETY: `key` is open; the value bytes are the NUL-terminated UTF-16
        // command, and the length counts them in bytes.
        unsafe {
            RegSetValueExW(
                key,
                name.as_ptr(),
                0,
                REG_SZ,
                command.as_ptr().cast(),
                (command.len() * 2) as u32,
            )
        }
    } else {
        // SAFETY: `key` is open and the value name is NUL-terminated.
        match unsafe { RegDeleteValueW(key, name.as_ptr()) } {
            2 => 0, // already absent
            code => code,
        }
    };
    // SAFETY: `key` was opened above and is closed once.
    unsafe { RegCloseKey(key) };
    if result == 0 {
        Ok(())
    } else {
        Err(format!(
            "couldn't change the sign-in list (windows error {result})"
        ))
    }
}

/// Give the window `ico`, or this program's own icon when there is none yet.
pub fn set_window_icon(hwnd: isize, ico: Option<&Path>) {
    for (which, metric) in [(ICON_SMALL, SM_CXSMICON), (ICON_BIG, SM_CXICON)] {
        // SAFETY: the metric query takes no pointers.
        let side = unsafe { GetSystemMetrics(metric) };
        let icon = match ico {
            Some(path) => {
                let path = wide(path);
                // SAFETY: the path is NUL-terminated; a null module with
                // LR_LOADFROMFILE reads the file.
                unsafe {
                    LoadImageW(
                        std::ptr::null_mut(),
                        path.as_ptr(),
                        IMAGE_ICON,
                        side,
                        side,
                        LR_LOADFROMFILE,
                    )
                }
            }
            // SAFETY: resource id 1 is the icon group this program's build links
            // in; MAKEINTRESOURCE is the id carried in the pointer value.
            None => unsafe {
                LoadImageW(
                    GetModuleHandleW(std::ptr::null()),
                    std::ptr::without_provenance::<u16>(1),
                    IMAGE_ICON,
                    side,
                    side,
                    LR_DEFAULTCOLOR,
                )
            },
        };
        if !icon.is_null() {
            // SAFETY: `hwnd` is this app's live window; WM_SETICON takes the
            // icon handle as its lparam. The window keeps the icon for its life.
            unsafe { SendMessageW(hwnd as _, WM_SETICON, which as usize, icon as isize) };
        }
    }
}

/// Point the Start-menu entry for this program at `ico` (its own icon when
/// `None`). Returns whether an entry was found.
pub fn set_shortcut_icon(ico: Option<&Path>) -> Result<bool, String> {
    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    let Some(programs) = std::env::var_os("APPDATA")
        .map(|appdata| PathBuf::from(appdata).join(r"Microsoft\Windows\Start Menu\Programs"))
    else {
        return Ok(false);
    };
    let mut candidates = vec![programs.join("journal.lnk")];
    if let Ok(entries) = std::fs::read_dir(&programs) {
        candidates.extend(entries.flatten().map(|entry| entry.path()).filter(|path| {
            path.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("lnk"))
        }));
    }
    let icon_path = ico.map_or_else(|| exe.clone(), Path::to_path_buf);
    for candidate in candidates {
        if candidate.is_file() && update_link_icon(&candidate, &exe, &icon_path)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn update_link_icon(link: &Path, exe: &Path, icon: &Path) -> Result<bool, String> {
    // SAFETY: COM is initialized on this thread; the interfaces release on
    // drop, and every string handed over is NUL-terminated and outlives its
    // call. GetPath writes at most the buffer's length.
    unsafe {
        let shell_link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
            .map_err(|error| error.to_string())?;
        let file: IPersistFile = shell_link.cast().map_err(|error| error.to_string())?;
        let link_wide = wide(link);
        if file
            .Load(PCWSTR(link_wide.as_ptr()), STGM_READWRITE)
            .is_err()
        {
            return Ok(false);
        }
        let mut target = [0_u16; 1024];
        if shell_link
            .GetPath(&mut target, std::ptr::null_mut(), 0)
            .is_err()
        {
            return Ok(false);
        }
        let end = target
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(target.len());
        let target = PathBuf::from(std::ffi::OsString::from_wide(&target[..end]));
        if !opens_this_app(&target, exe) {
            return Ok(false);
        }
        let icon_wide = wide(icon);
        shell_link
            .SetIconLocation(PCWSTR(icon_wide.as_ptr()), 0)
            .map_err(|error| error.to_string())?;
        file.Save(PCWSTR(link_wide.as_ptr()), true)
            .map_err(|error| error.to_string())?;
        SHChangeNotify(
            SHCNE_UPDATEITEM,
            SHCNF_PATHW,
            Some(link_wide.as_ptr().cast()),
            None,
        );
        SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None);
        Ok(true)
    }
}

/// Whether a shortcut's target opens this install's app: the program itself
/// under `current\bin`, or the installer's stub of the same name at the
/// install root, which starts whichever version is current.
fn opens_this_app(target: &Path, exe: &Path) -> bool {
    let normal = |path: &Path| path.display().to_string().to_lowercase().replace('/', "\\");
    let same_name = target
        .file_name()
        .zip(exe.file_name())
        .is_some_and(|(left, right)| left.eq_ignore_ascii_case(right));
    let Some(root) = exe.ancestors().nth(3) else {
        return false;
    };
    same_name && normal(target).starts_with(&(normal(root) + "\\"))
}

/// One app window per signed-in owner. A second start brings the first
/// window forward instead of opening another.
pub struct SingleInstance {
    _mutex: usize,
    event: usize,
}

const INSTANCE_MUTEX: &str = r"Local\solstone-journal-app";
const SHOW_EVENT: &str = r"Local\solstone-journal-app-show";

impl SingleInstance {
    /// `None` when another instance holds the window; that instance has
    /// been asked to show itself.
    pub fn acquire() -> Option<Self> {
        let mutex_name = wide(INSTANCE_MUTEX);
        let event_name = wide(SHOW_EVENT);
        // SAFETY: names are NUL-terminated; default security, not inherited.
        let mutex = unsafe { CreateMutexW(std::ptr::null(), 0, mutex_name.as_ptr()) };
        // SAFETY: reads this thread's last error right after the call above.
        if mutex.is_null() || unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            // SAFETY: the event name is NUL-terminated; a missing event or a
            // failed signal leaves nothing to clean up but the handles.
            unsafe {
                // This start came from the owner, so it may hand the
                // foreground to the window it is asking to come forward.
                AllowSetForegroundWindow(u32::MAX);
                let event = OpenEventW(EVENT_MODIFY_STATE, 0, event_name.as_ptr());
                if !event.is_null() {
                    SetEvent(event);
                    CloseHandle(event);
                }
                if !mutex.is_null() {
                    CloseHandle(mutex);
                }
            }
            return None;
        }
        // SAFETY: an auto-reset, unsignalled, named event with default security.
        let event = unsafe { CreateEventW(std::ptr::null(), 0, 0, event_name.as_ptr()) };
        Some(Self {
            _mutex: mutex as usize,
            event: event as usize,
        })
    }

    /// Call `show` each time another start asks this window to come forward.
    pub fn on_show(&self, show: impl Fn() + Send + 'static) {
        let event = self.event;
        if event == 0 {
            return;
        }
        std::thread::spawn(move || {
            loop {
                // SAFETY: the event handle lives as long as the process: the
                // instance that owns it is held by `main` until exit.
                if unsafe { WaitForSingleObject(event as HANDLE, INFINITE) } != 0 {
                    return;
                }
                show();
            }
        });
    }
}
