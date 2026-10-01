// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The window's page: a WebView2 view of the pages built into this program.
//! Nothing it shows comes from the network. Every request under the app's own
//! origin is answered from memory, any other navigation is refused, and a
//! link to the journal opens in the owner's browser instead. Developer tools,
//! the browser's own shortcuts and its context menu are off, and so is the
//! address reputation check that would send addresses to an outside service.

use std::path::Path;
use std::sync::mpsc;

use webview2_com::Microsoft::Web::WebView2::Win32::{
    COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
    CreateCoreWebView2EnvironmentWithOptions, GetAvailableCoreWebView2BrowserVersionString,
    ICoreWebView2, ICoreWebView2Controller, ICoreWebView2Environment,
    ICoreWebView2EnvironmentOptions, ICoreWebView2Settings3, ICoreWebView2Settings8,
};
use webview2_com::{
    CoreWebView2EnvironmentOptions, CreateCoreWebView2ControllerCompletedHandler,
    CreateCoreWebView2EnvironmentCompletedHandler, ExecuteScriptCompletedHandler,
    NavigationStartingEventHandler, NewWindowRequestedEventHandler, WebMessageReceivedEventHandler,
    WebResourceRequestedEventHandler, take_pwstr, wait_with_pump,
};
use windows::Win32::Foundation::{E_POINTER, HWND, RECT};
use windows::Win32::UI::Shell::SHCreateMemStream;
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
use windows::core::{HSTRING, Interface, PWSTR};

/// A page or file the app serves: its bytes and content type.
pub type Asset = fn(&str) -> Option<(&'static [u8], &'static str)>;

/// The pages built into the app and where they are served from.
pub struct Pages {
    /// The origin they are served under, never a real network address.
    pub origin: &'static str,
    /// The page the window opens on.
    pub start: &'static str,
    pub asset: Asset,
    pub content_security_policy: &'static str,
}

pub struct Page {
    controller: ICoreWebView2Controller,
    webview: ICoreWebView2,
}

fn error(error: impl std::fmt::Debug) -> String {
    format!("{error:?}")
}

impl Page {
    /// Fill the window `hwnd` with the app's `pages`.
    /// `on_message` gets each message the page posts; `on_link` gets an
    /// address the page tried to open outside the app.
    pub fn new(
        hwnd: isize,
        data_dir: &Path,
        pages: Pages,
        mut on_message: impl FnMut(String) + 'static,
        on_link: impl Fn(String) + 'static,
    ) -> Result<Self, String> {
        let Pages {
            origin,
            start,
            asset,
            content_security_policy,
        } = pages;
        let hwnd = HWND(hwnd as *mut core::ffi::c_void);
        let environment = create_environment(data_dir)?;
        let controller = create_controller(hwnd, &environment)?;
        // SAFETY: every call below is a WebView2 COM method on interfaces this
        // function holds; strings are HSTRINGs that outlive their calls, and
        // each handler is reference-counted by WebView2 for as long as it is
        // registered.
        unsafe {
            let webview = controller.CoreWebView2().map_err(error)?;
            let settings = webview.Settings().map_err(error)?;
            settings.SetAreDevToolsEnabled(false).map_err(error)?;
            settings
                .SetAreDefaultContextMenusEnabled(false)
                .map_err(error)?;
            settings.SetIsStatusBarEnabled(false).map_err(error)?;
            if let Ok(settings) = settings.cast::<ICoreWebView2Settings3>() {
                settings
                    .SetAreBrowserAcceleratorKeysEnabled(false)
                    .map_err(error)?;
            }
            if let Ok(settings) = settings.cast::<ICoreWebView2Settings8>() {
                settings
                    .SetIsReputationCheckingRequired(false)
                    .map_err(error)?;
            }

            let mut token = Default::default();
            // The pages, from memory.
            let filter = HSTRING::from(format!("{origin}/*"));
            webview
                .AddWebResourceRequestedFilter(&filter, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL)
                .map_err(error)?;
            let responder = environment.clone();
            webview
                .add_WebResourceRequested(
                    &WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
                        let Some(args) = args else { return Ok(()) };
                        let mut uri = PWSTR::null();
                        args.Request()?.Uri(&mut uri)?;
                        let uri = take_pwstr(uri);
                        let path = uri
                            .strip_prefix(origin)
                            .unwrap_or("/")
                            .split(['?', '#'])
                            .next()
                            .unwrap_or("/");
                        let response = match asset(path) {
                            Some((body, mime)) => {
                                let stream = SHCreateMemStream(Some(body));
                                let headers = HSTRING::from(format!(
                                    "Content-Type: {mime}\r\nContent-Security-Policy: {content_security_policy}\r\nCache-Control: no-store"
                                ));
                                responder.CreateWebResourceResponse(
                                    stream.as_ref(),
                                    200,
                                    &HSTRING::from("OK"),
                                    &headers,
                                )?
                            }
                            None => responder.CreateWebResourceResponse(
                                None,
                                404,
                                &HSTRING::from("Not Found"),
                                &HSTRING::new(),
                            )?,
                        };
                        args.SetResponse(&response)
                    })),
                    &mut token,
                )
                .map_err(error)?;
            // Messages from the page.
            webview
                .add_WebMessageReceived(
                    &WebMessageReceivedEventHandler::create(Box::new(move |_, args| {
                        let Some(args) = args else { return Ok(()) };
                        let mut message = PWSTR::null();
                        if args.TryGetWebMessageAsString(&mut message).is_ok() {
                            on_message(take_pwstr(message));
                        }
                        Ok(())
                    })),
                    &mut token,
                )
                .map_err(error)?;
            // Only the app's own pages, in this window.
            webview
                .add_NavigationStarting(
                    &NavigationStartingEventHandler::create(Box::new(move |_, args| {
                        let Some(args) = args else { return Ok(()) };
                        let mut uri = PWSTR::null();
                        args.Uri(&mut uri)?;
                        let uri = take_pwstr(uri);
                        args.SetCancel(!uri.starts_with(origin))
                    })),
                    &mut token,
                )
                .map_err(error)?;
            webview
                .add_NewWindowRequested(
                    &NewWindowRequestedEventHandler::create(Box::new(move |_, args| {
                        let Some(args) = args else { return Ok(()) };
                        let mut uri = PWSTR::null();
                        args.Uri(&mut uri)?;
                        on_link(take_pwstr(uri));
                        args.SetHandled(true)
                    })),
                    &mut token,
                )
                .map_err(error)?;

            let page = Self {
                controller,
                webview,
            };
            page.fit(hwnd.0 as isize, false);
            page.controller.SetIsVisible(true).map_err(error)?;
            page.webview
                .Navigate(&HSTRING::from(format!("{origin}{start}")))
                .map_err(error)?;
            Ok(page)
        }
    }

    /// Follow the window: hidden while it sits in the taskbar, and filling its
    /// client area whenever it is shown or resized.
    pub fn fit(&self, hwnd: isize, minimized: bool) {
        let mut rect = RECT::default();
        // SAFETY: `hwnd` is the live window this page belongs to and `rect`
        // is output storage; the rest are COM calls on the live controller.
        unsafe {
            if minimized {
                let _ = self.controller.SetIsVisible(false);
                return;
            }
            if GetClientRect(HWND(hwnd as *mut core::ffi::c_void), &mut rect).is_ok() {
                let _ = self.controller.SetBounds(rect);
            }
            let _ = self.controller.SetIsVisible(true);
        }
    }

    /// The window moved: WebView2 places its popups from this.
    pub fn moved(&self) {
        // SAFETY: a COM call on the live controller.
        unsafe {
            let _ = self.controller.NotifyParentWindowPositionChanged();
        }
    }

    pub fn focus(&self) {
        // SAFETY: a COM call on the live controller.
        unsafe {
            let _ = self
                .controller
                .MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC);
        }
    }

    /// Run `script` in the page; its result is not wanted.
    pub fn run_script(&self, script: &str) {
        let script = HSTRING::from(script);
        // SAFETY: a COM call on the live view; the handler is reference-counted.
        unsafe {
            let _ = self.webview.ExecuteScript(
                &script,
                &ExecuteScriptCompletedHandler::create(Box::new(|_, _| Ok(()))),
            );
        }
    }

    pub fn close(&self) {
        // SAFETY: a COM call on the live controller, once, before it drops.
        unsafe {
            let _ = self.controller.Close();
        }
    }
}

fn create_environment(data_dir: &Path) -> Result<ICoreWebView2Environment, String> {
    let mut version = PWSTR::null();
    // SAFETY: asks the loader for the installed runtime's version; the string
    // it returns is freed by `take_pwstr`.
    let found = unsafe {
        GetAvailableCoreWebView2BrowserVersionString(windows::core::PCWSTR::null(), &mut version)
    };
    if found.is_err() || version.is_null() {
        return Err(webview_runtime_missing());
    }
    let _ = take_pwstr(version);
    let (sender, receiver) = mpsc::channel();
    let data_dir = HSTRING::from(data_dir);
    let options = CoreWebView2EnvironmentOptions::default();
    // SAFETY: the options object and strings outlive the call; the completion
    // handler runs on this thread while `wait_with_pump` pumps its messages.
    unsafe {
        // No SmartScreen address checks: the window only shows built-in pages.
        options.set_additional_browser_arguments(
            "--disable-features=msSmartScreenProtection".to_owned(),
        );
        CreateCoreWebView2EnvironmentWithOptions(
            windows::core::PCWSTR::null(),
            &data_dir,
            &ICoreWebView2EnvironmentOptions::from(options),
            &CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(
                move |status, environment| {
                    let result = status.and_then(|()| {
                        environment.ok_or_else(|| windows::core::Error::from(E_POINTER))
                    });
                    let _ = sender.send(result);
                    Ok(())
                },
            )),
        )
        .map_err(window_failed)?;
    }
    wait_with_pump(receiver)
        .map_err(error)?
        .map_err(window_failed)
}

fn create_controller(
    hwnd: HWND,
    environment: &ICoreWebView2Environment,
) -> Result<ICoreWebView2Controller, String> {
    let (sender, receiver) = mpsc::channel();
    // SAFETY: `hwnd` is the live parent window; the completion handler runs on
    // this thread while `wait_with_pump` pumps its messages.
    unsafe {
        environment
            .CreateCoreWebView2Controller(
                hwnd,
                &CreateCoreWebView2ControllerCompletedHandler::create(Box::new(
                    move |status, controller| {
                        let result = status.and_then(|()| {
                            controller.ok_or_else(|| windows::core::Error::from(E_POINTER))
                        });
                        let _ = sender.send(result);
                        Ok(())
                    },
                )),
            )
            .map_err(error)?;
    }
    wait_with_pump(receiver).map_err(error)?.map_err(error)
}

/// What the owner reads when Windows has no web view runtime to draw with.
fn webview_runtime_missing() -> String {
    "this PC is missing the Microsoft Edge WebView2 Runtime. install the WebView2 Runtime from Microsoft, then open journal again.".to_owned()
}

/// Any other reason the window could not be drawn, with Windows' own words.
fn window_failed(reason: impl std::fmt::Debug) -> String {
    format!("the journal app couldn't open its window.\n{reason:?}")
}
