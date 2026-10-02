//! Optional native text clipboard, using the public API 13 Pasteboard/UDMF ABI.
//! Loaded only for an explicit copy/paste. No service or clipboard permission probe
//! runs at startup. OS permission errors remain visible; no external helper is used.

use std::ffi::CStr;
use std::ffi::CString;
use std::ffi::c_char;
use std::ffi::c_int;
use std::ffi::c_void;
use std::ptr::NonNull;

#[repr(C)]
struct Pasteboard {
    _private: [u8; 0],
}
#[repr(C)]
struct Data {
    _private: [u8; 0],
}
#[repr(C)]
struct Record {
    _private: [u8; 0],
}
#[repr(C)]
struct PlainText {
    _private: [u8; 0],
}

struct Library(NonNull<c_void>);

impl Library {
    fn open(name: &CStr) -> Result<Self, String> {
        // SAFETY: Constant NUL-terminated sonames, loaded through the OS linker.
        NonNull::new(unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) })
            .map(Self)
            .ok_or_else(|| {
                format!(
                    "HarmonyOS clipboard library {} is unavailable",
                    name.to_string_lossy()
                )
            })
    }

    fn symbol(&self, name: &CStr) -> Result<*mut c_void, String> {
        // SAFETY: The library handle remains live throughout all calls.
        let pointer = unsafe { libc::dlsym(self.0.as_ptr(), name.as_ptr()) };
        if pointer.is_null() {
            Err(format!(
                "HarmonyOS clipboard API {} is unavailable (requires API 13 or later)",
                name.to_string_lossy()
            ))
        } else {
            Ok(pointer)
        }
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        // SAFETY: This owns one reference returned by dlopen.
        unsafe {
            libc::dlclose(self.0.as_ptr());
        }
    }
}

// Signatures are from database/pasteboard/oh_pasteboard.h and database/udmf/{udmf,uds}.h.
// Keeping the function signatures here also makes SDK ABI checking independent of the TUI.
macro_rules! api {
    ($pasteboard:ident, $udmf:ident; $($library:ident: $name:ident($($arg:ty),*) -> $result:ty;)*) => {
        #[allow(non_snake_case)]
        struct Api {
            $($name: unsafe extern "C" fn($($arg),*) -> $result,)*
            _pasteboard: Option<Library>,
            _udmf: Option<Library>,
        }
        impl Api {
            fn load() -> Result<Self, String> {
                let $pasteboard = Library::open(c"libpasteboard.so")?;
                let $udmf = Library::open(c"libudmf.so")?;
                Ok(Self {
                    // SAFETY: Each symbol and concrete C signature matches the public SDK.
                    $($name: unsafe {
                        std::mem::transmute::<*mut c_void, unsafe extern "C" fn($($arg),*) -> $result>(
                            $library.symbol(CStr::from_bytes_with_nul(concat!(stringify!($name), "\0").as_bytes()).expect("constant API name"))?
                        )
                    },)*
                    _pasteboard: Some($pasteboard),
                    _udmf: Some($udmf),
                })
            }
        }
    }
}

api! {
    pasteboard, udmf;
    pasteboard: OH_Pasteboard_Create() -> *mut Pasteboard;
    pasteboard: OH_Pasteboard_Destroy(*mut Pasteboard) -> ();
    pasteboard: OH_Pasteboard_SetData(*mut Pasteboard, *mut Data) -> c_int;
    pasteboard: OH_Pasteboard_GetData(*mut Pasteboard, *mut c_int) -> *mut Data;
    udmf: OH_UdmfData_Create() -> *mut Data;
    udmf: OH_UdmfData_Destroy(*mut Data) -> ();
    udmf: OH_UdmfData_AddRecord(*mut Data, *mut Record) -> c_int;
    udmf: OH_UdmfData_GetPrimaryPlainText(*mut Data, *mut PlainText) -> c_int;
    udmf: OH_UdmfRecord_Create() -> *mut Record;
    udmf: OH_UdmfRecord_Destroy(*mut Record) -> ();
    udmf: OH_UdmfRecord_AddPlainText(*mut Record, *mut PlainText) -> c_int;
    udmf: OH_UdsPlainText_Create() -> *mut PlainText;
    udmf: OH_UdsPlainText_Destroy(*mut PlainText) -> ();
    udmf: OH_UdsPlainText_SetContent(*mut PlainText, *const c_char) -> c_int;
    udmf: OH_UdsPlainText_GetContent(*mut PlainText) -> *const c_char;
}

struct Object<T> {
    pointer: NonNull<T>,
    destroy: unsafe extern "C" fn(*mut T),
}

impl<T> Object<T> {
    fn new(pointer: *mut T, destroy: unsafe extern "C" fn(*mut T)) -> Result<Self, String> {
        Ok(Self {
            pointer: NonNull::new(pointer).ok_or("HarmonyOS clipboard returned no object")?,
            destroy,
        })
    }
    fn raw(&self) -> *mut T {
        self.pointer.as_ptr()
    }
}

impl<T> Drop for Object<T> {
    fn drop(&mut self) {
        // SAFETY: Each owned API object is destroyed once, before its Api libraries.
        unsafe {
            (self.destroy)(self.raw());
        }
    }
}

fn status(operation: &str, code: c_int) -> Result<(), String> {
    if code == 0 {
        return Ok(());
    }
    let reason = match code {
        201 => {
            "permission denied; native reads require ohos.permission.READ_PASTEBOARD for this process"
        }
        801 => "the device does not support this clipboard capability",
        12900003 => "another clipboard write is in progress",
        _ => "native clipboard operation failed",
    };
    Err(format!(
        "HarmonyOS clipboard {operation}: {reason} (code {code}); use the terminal copy/paste shortcut if unavailable"
    ))
}

pub(crate) fn write_text(text: &str) -> Result<(), String> {
    let text = CString::new(text).map_err(|_| "clipboard text contains a NUL byte")?;
    let api = Api::load()?;
    write_with_api(&api, &text)
}

fn write_with_api(api: &Api, text: &CStr) -> Result<(), String> {
    // SAFETY: Objects are checked for null, kept alive across calls, and destroyed
    // with their matching API. SetContent/AddRecord copy or retain their inputs.
    unsafe {
        let pasteboard = Object::new((api.OH_Pasteboard_Create)(), api.OH_Pasteboard_Destroy)?;
        let data = Object::new((api.OH_UdmfData_Create)(), api.OH_UdmfData_Destroy)?;
        let record = Object::new((api.OH_UdmfRecord_Create)(), api.OH_UdmfRecord_Destroy)?;
        let plain = Object::new((api.OH_UdsPlainText_Create)(), api.OH_UdsPlainText_Destroy)?;
        status(
            "set text",
            (api.OH_UdsPlainText_SetContent)(plain.raw(), text.as_ptr()),
        )?;
        status(
            "add text",
            (api.OH_UdmfRecord_AddPlainText)(record.raw(), plain.raw()),
        )?;
        status(
            "add record",
            (api.OH_UdmfData_AddRecord)(data.raw(), record.raw()),
        )?;
        status(
            "write",
            (api.OH_Pasteboard_SetData)(pasteboard.raw(), data.raw()),
        )
    }
}

pub(crate) fn read_text(max_bytes: usize) -> Result<String, String> {
    let api = Api::load()?;
    read_with_api(&api, max_bytes)
}

fn read_with_api(api: &Api, max_bytes: usize) -> Result<String, String> {
    // SAFETY: Data is owned until after its string is copied; all SDK outputs are
    // checked. The string is NUL-terminated by UDMF, and its copy is size-bounded.
    unsafe {
        let pasteboard = Object::new((api.OH_Pasteboard_Create)(), api.OH_Pasteboard_Destroy)?;
        let mut code = 0;
        let data = (api.OH_Pasteboard_GetData)(pasteboard.raw(), &mut code);
        // Retain any returned object even on error so it is also released.
        let data = Object::new(data, api.OH_UdmfData_Destroy);
        status("read", code)?;
        let data = data?;
        let plain = Object::new((api.OH_UdsPlainText_Create)(), api.OH_UdsPlainText_Destroy)?;
        status(
            "read plain text",
            (api.OH_UdmfData_GetPrimaryPlainText)(data.raw(), plain.raw()),
        )?;
        let content = (api.OH_UdsPlainText_GetContent)(plain.raw());
        if content.is_null() {
            return Err("HarmonyOS clipboard contains no plain text".into());
        }
        let length = libc::strnlen(content, max_bytes.saturating_add(1));
        if length > max_bytes {
            return Err("clipboard text exceeds the message size limit".into());
        }
        std::str::from_utf8(std::slice::from_raw_parts(content.cast::<u8>(), length))
            .map(str::to_owned)
            .map_err(|_| "HarmonyOS clipboard text is not UTF-8".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    thread_local! {
        static LIVE: Cell<usize> = const { Cell::new(0) };
        static NATIVE_STATUS: Cell<c_int> = const { Cell::new(0) };
    }
    // These are API doubles only: no system clipboard, account or device is accessed.
    unsafe extern "C" fn create<T>() -> *mut T {
        LIVE.set(LIVE.get() + 1);
        Box::into_raw(Box::new(0_u8)).cast()
    }
    unsafe extern "C" fn destroy<T>(pointer: *mut T) {
        unsafe {
            drop(Box::from_raw(pointer.cast::<u8>()));
        }
        LIVE.set(LIVE.get() - 1);
    }
    unsafe extern "C" fn pair<T, U>(_: *mut T, _: *mut U) -> c_int {
        0
    }
    unsafe extern "C" fn set_text(_: *mut PlainText, content: *const c_char) -> c_int {
        assert_eq!(unsafe { CStr::from_ptr(content) }, c"text");
        0
    }
    unsafe extern "C" fn set_data(_: *mut Pasteboard, _: *mut Data) -> c_int {
        NATIVE_STATUS.get()
    }
    unsafe extern "C" fn get_data(_: *mut Pasteboard, code: *mut c_int) -> *mut Data {
        unsafe {
            *code = NATIVE_STATUS.get();
            create()
        }
    }
    unsafe extern "C" fn get_content(_: *mut PlainText) -> *const c_char {
        c"hello".as_ptr()
    }
    fn fake() -> Api {
        Api {
            OH_Pasteboard_Create: create,
            OH_Pasteboard_Destroy: destroy,
            OH_Pasteboard_SetData: set_data,
            OH_Pasteboard_GetData: get_data,
            OH_UdmfData_Create: create,
            OH_UdmfData_Destroy: destroy,
            OH_UdmfData_AddRecord: pair,
            OH_UdmfData_GetPrimaryPlainText: pair,
            OH_UdmfRecord_Create: create,
            OH_UdmfRecord_Destroy: destroy,
            OH_UdmfRecord_AddPlainText: pair,
            OH_UdsPlainText_Create: create,
            OH_UdsPlainText_Destroy: destroy,
            OH_UdsPlainText_SetContent: set_text,
            OH_UdsPlainText_GetContent: get_content,
            _pasteboard: None,
            _udmf: None,
        }
    }

    #[test]
    fn native_calls_release_objects_on_success_and_permission_failure() {
        let api = fake();
        NATIVE_STATUS.set(0);
        write_with_api(&api, c"text").unwrap();
        assert_eq!(LIVE.get(), 0);
        assert_eq!(read_with_api(&api, 5).unwrap(), "hello");
        assert_eq!(LIVE.get(), 0);
        assert!(read_with_api(&api, 4).unwrap_err().contains("size limit"));
        assert_eq!(LIVE.get(), 0);
        NATIVE_STATUS.set(201);
        assert!(write_with_api(&api, c"text").unwrap_err().contains("201"));
        assert_eq!(LIVE.get(), 0);
        assert!(read_with_api(&api, 5).unwrap_err().contains("201"));
        assert_eq!(LIVE.get(), 0);
        NATIVE_STATUS.set(0);
    }
    #[test]
    fn native_errors_keep_codes_and_permission_diagnostics() {
        assert!(status("write", 0).is_ok());
        for code in [201, 801, 12900003, 12900000] {
            let error = status("read", code).unwrap_err();
            assert!(error.contains(&code.to_string()));
            assert!(error.contains("HarmonyOS clipboard"));
        }
        assert!(status("read", 201).unwrap_err().contains("READ_PASTEBOARD"));
    }
    #[test]
    fn nul_text_is_rejected_before_loading_any_library() {
        assert!(write_text("a\0b").unwrap_err().contains("NUL"));
    }
}
