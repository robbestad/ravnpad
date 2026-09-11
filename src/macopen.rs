// macOS file-open events (double-click in Finder, "Open With").
//
// winit's application delegate does not implement application:openFile:,
// so AppKit shows "cannot open files in the '…' format" when Finder asks
// the app to open a document. We add the handlers to the delegate class
// at runtime and queue the paths for the UI to pick up.
//
// The open-documents AppleEvent can be dispatched before the app creator
// runs, so install() spawns a thread that patches the delegate class the
// moment winit registers it — ahead of the first event-loop dispatch.

use std::ffi::CStr;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use eframe::egui;
use objc2::msg_send;
use objc2::runtime::{AnyClass, AnyObject, Bool, Sel};
use objc2::sel;

static PENDING: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
static CONTEXT: Mutex<Option<egui::Context>> = Mutex::new(None);

unsafe extern "C-unwind" fn open_file(
    _this: *mut AnyObject,
    _cmd: Sel,
    _app: *mut AnyObject,
    filename: *mut AnyObject,
) -> Bool {
    unsafe {
        push(filename);
    }
    Bool::YES
}

unsafe extern "C-unwind" fn open_files(
    _this: *mut AnyObject,
    _cmd: Sel,
    _app: *mut AnyObject,
    filenames: *mut AnyObject,
) {
    unsafe {
        let count: usize = msg_send![filenames, count];
        for i in 0..count {
            let name: *mut AnyObject = msg_send![filenames, objectAtIndex: i];
            if !name.is_null() {
                push(name);
            }
        }
    }
}

unsafe fn push(filename: *mut AnyObject) {
    let path: *const std::ffi::c_char = msg_send![filename, fileSystemRepresentation];
    if path.is_null() {
        return;
    }
    let text = unsafe { CStr::from_ptr(path) }
        .to_string_lossy()
        .into_owned();
    if let Ok(mut pending) = PENDING.lock() {
        pending.push(PathBuf::from(text));
    }
    if let Ok(guard) = CONTEXT.lock()
        && let Some(ctx) = &*guard
    {
        ctx.request_repaint();
    }
}

pub fn set_context(ctx: &egui::Context) {
    if let Ok(mut guard) = CONTEXT.lock() {
        *guard = Some(ctx.clone());
    }
}

// Adds application:openFile:/application:openFiles: to winit's app
// delegate. The class is only registered once the event loop creates
// the delegate inside run_native, so poll for it on a background thread
// to patch it before the launch-time AppleEvent is dispatched.
pub fn install() {
    std::thread::spawn(|| {
        for _ in 0..5000 {
            if let Some(class) = AnyClass::get(c"WinitApplicationDelegate") {
                unsafe {
                    add_methods(class);
                }
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
}

unsafe fn add_methods(class: &AnyClass) {
    let class = class as *const AnyClass as *mut AnyClass;
    let open_file = open_file
        as unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject, *mut AnyObject) -> Bool;
    let open_files = open_files
        as unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject, *mut AnyObject);
    unsafe {
        objc2::ffi::class_addMethod(
            class,
            sel!(application:openFile:),
            std::mem::transmute::<_, objc2::runtime::Imp>(open_file),
            c"B@:@@".as_ptr(),
        );
        objc2::ffi::class_addMethod(
            class,
            sel!(application:openFiles:),
            std::mem::transmute::<_, objc2::runtime::Imp>(open_files),
            c"v@:@@".as_ptr(),
        );
    }
}

pub fn drain() -> Vec<PathBuf> {
    PENDING
        .lock()
        .map(|mut pending| std::mem::take(&mut *pending))
        .unwrap_or_default()
}
