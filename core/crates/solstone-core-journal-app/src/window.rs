// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The app's one top-level window and its message loop, in plain Win32. The
//! web view fills it and follows its size. Other threads hand the window work
//! through [`Proxy`], which queues it and wakes the loop.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{
    DWMWA_CAPTION_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetProcessDpiAwarenessContext,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow,
    DispatchMessageW, GetMessageW, IDC_ARROW, IMAGE_ICON, LR_DEFAULTSIZE, LoadCursorW, LoadImageW,
    MINMAXINFO, MSG, PostMessageW, PostQuitMessage, RegisterClassExW, SIZE_MINIMIZED,
    SPI_GETWORKAREA, SW_MINIMIZE, SW_RESTORE, SW_SHOWMINNOACTIVE, SW_SHOWNORMAL, SWP_NOZORDER,
    SetForegroundWindow, SetWindowPos, ShowWindow, SystemParametersInfoW, TranslateMessage, WM_APP,
    WM_CLOSE, WM_DESTROY, WM_GETMINMAXINFO, WM_MOVE, WM_SETTINGCHANGE, WM_SIZE, WM_THEMECHANGED,
    WNDCLASSEXW, WS_OVERLAPPEDWINDOW,
};

/// What other threads can ask of the window.
pub enum UserEvent {
    /// A message from the page.
    Ipc(String),
    /// Script to run in the page.
    Script(String),
    /// Another start asked this window to come forward.
    Show,
    /// Quit the app.
    Exit,
}

/// What the loop hands the app.
pub enum WindowEvent {
    User(UserEvent),
    /// The window was activated; the page may want a fresh reading.
    Activated,
    /// The window changed size; `minimized` when it went to the taskbar.
    Resized {
        minimized: bool,
    },
    /// The window moved.
    Moved,
}

const WM_APP_EVENT: u32 = WM_APP + 1;
const CLASS_NAME: &str = "SolstoneJournalAppWindow";
/// The smallest the window goes, in 96-dpi pixels: the Mac app's minimum.
const MIN_SIZE: (i32, i32) = (720, 500);
const START_SIZE: (i32, i32) = (900, 640);

type Queue = Arc<Mutex<VecDeque<UserEvent>>>;
type Handler = Box<dyn FnMut(WindowEvent)>;

thread_local! {
    static HANDLER: RefCell<Option<Handler>> = const { RefCell::new(None) };
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Keep the native caption on the page's cream ground and system appearance.
/// The frame remains Windows' own, including its buttons and resize behavior.
fn apply_caption_appearance(hwnd: HWND) {
    let key = wide(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
    let name = wide("AppsUseLightTheme");
    let mut light = 1_u32;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: the strings are NUL-terminated, and the DWORD output has the
    // exact size admitted by RRF_RT_REG_DWORD. A missing preference is light.
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&raw mut light).cast(),
            &raw mut size,
        )
    };
    let dark = i32::from(result == 0 && light == 0);
    // Derive the native COLORREF from the same compiled tokens as the page;
    // a token revision must change both without a second palette to maintain.
    let tokens = if dark != 0 {
        include_str!("../../solstone-core-convey-shell/assets/static/tokens-dark.css")
    } else {
        include_str!("../../solstone-core-convey-shell/assets/static/tokens.css")
    };
    let hex = tokens
        .split("--cream:")
        .nth(1)
        .expect("the page defines its cream ground")
        .trim_start()
        .strip_prefix('#')
        .expect("the cream ground is an RGB token");
    let rgb = u32::from_str_radix(&hex[..6], 16).expect("the cream ground is an RGB token");
    let caption = ((rgb & 0xff) << 16) | (rgb & 0xff00) | ((rgb >> 16) & 0xff);
    // SAFETY: hwnd is the live top-level window, both attributes read one
    // four-byte value for this call, and neither retains the pointer. DWM
    // attributes unsupported on Win10 leave the native default intact.
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            (&raw const dark).cast(),
            size_of::<i32>() as u32,
        );
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_CAPTION_COLOR as u32,
            (&raw const caption).cast(),
            size_of::<u32>() as u32,
        );
    }
}

/// Hands work to the window from any thread.
#[derive(Clone)]
pub struct Proxy {
    hwnd: usize,
    queue: Queue,
}

impl Proxy {
    pub fn send_event(&self, event: UserEvent) {
        self.queue.lock().expect("window queue").push_back(event);
        // SAFETY: posting to a window handle that no longer exists fails
        // harmlessly; the queue is drained only on the window's own thread.
        unsafe { PostMessageW(self.hwnd as HWND, WM_APP_EVENT, 0, 0) };
    }
}

pub struct Window {
    hwnd: HWND,
    queue: Queue,
}

impl Window {
    /// Create the window, minimized to the taskbar when `minimized`.
    pub fn new(title: &str, minimized: bool) -> Self {
        // SAFETY: called once, before any window exists; failure leaves the
        // process at its default awareness, which only blurs the window.
        unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        let class = wide(CLASS_NAME);
        let title = wide(title);
        // SAFETY: the class and title strings outlive the calls that read
        // them; resource id 1 is the icon group this program links in.
        let hwnd = unsafe {
            let instance = GetModuleHandleW(std::ptr::null());
            let class_info = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(window_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: instance,
                hIcon: LoadImageW(
                    instance,
                    std::ptr::without_provenance::<u16>(1),
                    IMAGE_ICON,
                    0,
                    0,
                    LR_DEFAULTSIZE,
                ),
                hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
                hbrBackground: std::ptr::null_mut(),
                lpszMenuName: std::ptr::null(),
                lpszClassName: class.as_ptr(),
                hIconSm: std::ptr::null_mut(),
            };
            RegisterClassExW(&class_info);
            CreateWindowExW(
                0,
                class.as_ptr(),
                title.as_ptr(),
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                START_SIZE.0,
                START_SIZE.1,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance,
                std::ptr::null(),
            )
        };
        assert!(!hwnd.is_null(), "create the journal window");
        apply_caption_appearance(hwnd);
        let window = Self {
            hwnd,
            queue: Arc::default(),
        };
        let scale = window.scale();
        // Start at the Mac window's size, but never larger than the screen
        // the taskbar leaves, and centred in it.
        let mut work = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        // SAFETY: SPI_GETWORKAREA writes one RECT to the pointer given.
        unsafe { SystemParametersInfoW(SPI_GETWORKAREA, 0, (&raw mut work).cast(), 0) };
        let (work_width, work_height) = (work.right - work.left, work.bottom - work.top);
        let fit = |wanted: i32, room: i32| {
            let wanted = (f64::from(wanted) * scale) as i32;
            if room > 0 {
                wanted.min(room * 95 / 100)
            } else {
                wanted
            }
        };
        let (width, height) = (
            fit(START_SIZE.0, work_width),
            fit(START_SIZE.1, work_height),
        );
        // SAFETY: `hwnd` is the live window created above.
        unsafe {
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                work.left + (work_width - width).max(0) / 2,
                work.top + (work_height - height).max(0) / 2,
                width,
                height,
                SWP_NOZORDER,
            );
            ShowWindow(
                hwnd,
                if minimized {
                    SW_SHOWMINNOACTIVE
                } else {
                    SW_SHOWNORMAL
                },
            );
        }
        window
    }

    pub fn hwnd(&self) -> isize {
        self.hwnd as isize
    }

    fn scale(&self) -> f64 {
        // SAFETY: `hwnd` is this live window.
        f64::from(unsafe { GetDpiForWindow(self.hwnd) }.max(96)) / 96.0
    }

    pub fn proxy(&self) -> Proxy {
        Proxy {
            hwnd: self.hwnd as usize,
            queue: self.queue.clone(),
        }
    }

    pub fn minimize(&self) {
        // SAFETY: `hwnd` is this live window.
        unsafe { ShowWindow(self.hwnd, SW_MINIMIZE) };
    }

    pub fn bring_forward(&self) {
        // SAFETY: `hwnd` is this live window.
        unsafe {
            ShowWindow(self.hwnd, SW_RESTORE);
            SetForegroundWindow(self.hwnd);
        }
    }

    pub fn close(&self) {
        // SAFETY: `hwnd` is this live window; destroying it ends the loop.
        unsafe { DestroyWindow(self.hwnd) };
    }

    /// Run the message loop until the window is destroyed, handing each event
    /// to `handler` on this thread.
    pub fn run(&self, handler: impl FnMut(WindowEvent) + 'static) {
        HANDLER.with(|slot| *slot.borrow_mut() = Some(Box::new(handler)));
        QUEUE.with(|slot| *slot.borrow_mut() = Some(self.queue.clone()));
        // SAFETY: the standard message loop on the thread that owns the window.
        unsafe {
            let mut message: MSG = std::mem::zeroed();
            while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        HANDLER.with(|slot| slot.borrow_mut().take());
    }
}

thread_local! {
    static QUEUE: RefCell<Option<Queue>> = const { RefCell::new(None) };
    static DEFERRED: RefCell<VecDeque<WindowEvent>> = const { RefCell::new(VecDeque::new()) };
}

fn dispatch(event: WindowEvent) {
    // The handler is taken out while it runs, so a call it makes that pumps
    // messages (a modal dialog, or bringing the window back, which resizes it
    // there and then) cannot re-enter it. What arrives meanwhile waits and
    // runs right after, rather than being lost: a lost resize left the page
    // blank once the window came back from the taskbar.
    let Some(mut handler) = HANDLER.with(|slot| slot.borrow_mut().take()) else {
        DEFERRED.with(|deferred| deferred.borrow_mut().push_back(event));
        return;
    };
    handler(event);
    while let Some(next) = DEFERRED.with(|deferred| deferred.borrow_mut().pop_front()) {
        handler(next);
    }
    HANDLER.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            *slot = Some(handler);
        }
    });
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_SETTINGCHANGE | WM_THEMECHANGED => {
            apply_caption_appearance(hwnd);
            // SAFETY: forwarding the unchanged system appearance notification.
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
        WM_APP_EVENT => {
            loop {
                let next = QUEUE.with(|slot| {
                    slot.borrow()
                        .as_ref()
                        .and_then(|queue| queue.lock().expect("window queue").pop_front())
                });
                match next {
                    Some(event) => dispatch(WindowEvent::User(event)),
                    None => break,
                }
            }
            0
        }
        // Closing the window keeps the app in the taskbar and the journal
        // running, as closing the Mac app's window keeps it in the dock.
        WM_CLOSE => {
            // SAFETY: `hwnd` is the window this procedure serves.
            unsafe { ShowWindow(hwnd, SW_MINIMIZE) };
            0
        }
        WM_GETMINMAXINFO => {
            // SAFETY: for this message `lparam` points at the system's
            // MINMAXINFO for the window, valid for the call.
            unsafe {
                let scale = f64::from(GetDpiForWindow(hwnd).max(96)) / 96.0;
                let info = lparam as *mut MINMAXINFO;
                (*info).ptMinTrackSize = POINT {
                    x: (f64::from(MIN_SIZE.0) * scale) as i32,
                    y: (f64::from(MIN_SIZE.1) * scale) as i32,
                };
            }
            0
        }
        // WM_ACTIVATE with a non-zero low word: the window became active.
        0x0006 if wparam & 0xFFFF != 0 => {
            dispatch(WindowEvent::Activated);
            // SAFETY: forwarding the message unchanged to the default procedure.
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
        WM_SIZE => {
            dispatch(WindowEvent::Resized {
                minimized: wparam == SIZE_MINIMIZED as usize,
            });
            0
        }
        WM_MOVE => {
            dispatch(WindowEvent::Moved);
            0
        }
        WM_DESTROY => {
            // SAFETY: ends this thread's message loop.
            unsafe { PostQuitMessage(0) };
            0
        }
        // SAFETY: forwarding the message unchanged to the default procedure.
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}
