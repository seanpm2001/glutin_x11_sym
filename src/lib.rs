#![cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
))]

#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate winit_types;
#[macro_use]
extern crate log;

use parking_lot::Mutex;
use winit_types::error::Error;
use winit_types::platform::{OsError, XError, XNotSupported};
use x11_dl::error::OpenError;

use std::ffi::CStr;
use std::mem::MaybeUninit;
use std::ops::{Deref, DerefMut};
use std::os::raw;
use std::ptr;
use std::sync::Arc;

lazy_static! {
    pub static ref XEXT: Result<x11_dl::dpms::Xext, OpenError> = x11_dl::dpms::Xext::open();
    pub static ref XSS: Result<x11_dl::xss::Xss, OpenError> = x11_dl::xss::Xss::open();
    pub static ref XFT: Result<x11_dl::xft::Xft, OpenError> = x11_dl::xft::Xft::open();
    pub static ref XT: Result<x11_dl::xt::Xt, OpenError> = x11_dl::xt::Xt::open();
    pub static ref XMU: Result<x11_dl::xmu::Xmu, OpenError> = x11_dl::xmu::Xmu::open();
    pub static ref XRENDER: Result<x11_dl::xrender::Xrender, OpenError> =
        x11_dl::xrender::Xrender::open();
    pub static ref XCURSOR: Result<x11_dl::xcursor::Xcursor, OpenError> =
        x11_dl::xcursor::Xcursor::open();
    pub static ref GLX: Result<x11_dl::glx::Glx, OpenError> = x11_dl::glx::Glx::open();
    pub static ref XINPUT: Result<x11_dl::xinput::XInput, OpenError> =
        x11_dl::xinput::XInput::open();
    pub static ref XINPUT2: Result<x11_dl::xinput2::XInput2, OpenError> =
        x11_dl::xinput2::XInput2::open();
    pub static ref XRANDR_2_2_0: Result<x11_dl::xrandr::Xrandr_2_2_0, OpenError> =
        x11_dl::xrandr::Xrandr_2_2_0::open();
    pub static ref XRANDR: Result<x11_dl::xrandr::Xrandr, OpenError> =
        x11_dl::xrandr::Xrandr::open();
    pub static ref XF86VMODE: Result<x11_dl::xf86vmode::Xf86vmode, OpenError> =
        x11_dl::xf86vmode::Xf86vmode::open();
    pub static ref XTEST_XF86VMODE: Result<x11_dl::xtest::Xf86vmode, OpenError> =
        x11_dl::xtest::Xf86vmode::open();
    pub static ref XRECORD_XF86VMODE: Result<x11_dl::xrecord::Xf86vmode, OpenError> =
        x11_dl::xrecord::Xf86vmode::open();
    pub static ref XINERAMA: Result<x11_dl::xinerama::Xlib, OpenError> =
        x11_dl::xinerama::Xlib::open();
    pub static ref XLIB: Result<x11_dl::xlib::Xlib, OpenError> = x11_dl::xlib::Xlib::open();
    pub static ref XLIB_XCB: Result<x11_dl::xlib_xcb::Xlib_xcb, OpenError> =
        x11_dl::xlib_xcb::Xlib_xcb::open();
    pub static ref X11_DISPLAY: Mutex<Result<Arc<Display>, Error>> =
        { Mutex::new(Display::new().map(Arc::new)) };
}

#[macro_export]
macro_rules! syms {
    ($name:ident) => {{ glutin_x11_sym::$name.as_ref().unwrap() }};
    ($($name:ident),+) => {{( $(syms!($name)),+ )}};
}

macro_rules! lsyms {
    ($name:ident) => {{ crate::$name.as_ref().unwrap() }};
    ($($name:ident),+) => {{( $(lsyms!($name)),+ )}};
}

#[derive(Debug)]
pub struct Display {
    pub display: *mut x11_dl::xlib::Display,
    pub latest_error: Mutex<Option<Error>>,
    owned: bool,
}

unsafe impl Send for Display {}
unsafe impl Sync for Display {}

impl Display {
    #[inline]
    fn new() -> Result<Display, Error> {
        let xlib = lsyms!(XLIB);
        unsafe { (xlib.XInitThreads)() };
        unsafe { (xlib.XSetErrorHandler)(Some(x_error_callback)) };

        // calling XOpenDisplay
        let display = unsafe {
            let display = (xlib.XOpenDisplay)(ptr::null());
            if display.is_null() {
                return Err(make_oserror!(OsError::XNotSupported(
                    XNotSupported::XOpenDisplayFailed
                )));
            }
            display
        };

        Ok(Display {
            display,
            latest_error: Mutex::new(None),
            owned: true,
        })
    }

    #[inline]
    pub fn from_raw(display: *mut raw::c_void) -> Arc<Display> {
        if let Ok(ref x11_display) = *X11_DISPLAY.lock() {
            if x11_display.display == display as *mut _ {
                return Arc::clone(x11_display);
            }
        }

        warn!("X11 display not X11_DISPLAY's display, users of this display will not know errors.");
        Arc::new(Display {
            display: display as *mut _,
            latest_error: Mutex::new(None),
            owned: false,
        })
    }

    /// Checks whether an error has been triggered by the previous function calls.
    #[inline]
    pub fn check_errors(&self) -> Result<(), Error> {
        let error = self.latest_error.lock().take();
        if let Some(error) = error {
            Err(error)
        } else {
            Ok(())
        }
    }

    /// Ignores any previous error.
    #[inline]
    pub fn ignore_error(&self) {
        *self.latest_error.lock() = None;
    }
}

impl Drop for Display {
    #[inline]
    fn drop(&mut self) {
        if self.owned {
            let xlib = lsyms!(XLIB);
            unsafe { (xlib.XCloseDisplay)(self.display) };
        }
    }
}

unsafe extern "C" fn x_error_callback(
    display_ptr: *mut x11_dl::xlib::Display,
    event: *mut x11_dl::xlib::XErrorEvent,
) -> raw::c_int {
    let xlib = lsyms!(XLIB);
    let display = X11_DISPLAY.lock();
    if let Ok(ref display) = *display {
        // `assume_init` is safe here because the array consists of `MaybeUninit` values,
        // which do not require initialization.
        let mut buf: [MaybeUninit<raw::c_char>; 1024] = MaybeUninit::uninit().assume_init();
        (xlib.XGetErrorText)(
            display_ptr,
            (*event).error_code as raw::c_int,
            buf.as_mut_ptr() as *mut raw::c_char,
            buf.len() as raw::c_int,
        );
        let description = CStr::from_ptr(buf.as_ptr() as *const raw::c_char).to_string_lossy();

        let error = make_oserror!(OsError::XError(XError {
            description: description.into_owned(),
            error_code: (*event).error_code,
            request_code: (*event).request_code,
            minor_code: (*event).minor_code,
        }));

        error!("X11 error: {:#?}", error);

        *display.latest_error.lock() = Some(error);
    }
    // Fun fact: this return value is completely ignored.
    0
}

impl Deref for Display {
    type Target = *mut x11_dl::xlib::Display;

    fn deref(&self) -> &Self::Target {
        &self.display
    }
}

impl DerefMut for Display {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.display
    }
}
