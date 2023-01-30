use std::error::Error;
use std::ffi::{c_char, c_void, CStr, CString};
use windows::Win32::System::LibraryLoader::GetModuleFileNameA;
use windows::Win32::System::Threading::GetCurrentThread;
use windows::Win32::Foundation::{HANDLE, MAX_PATH, HINSTANCE, NO_ERROR, WIN32_ERROR};

#[link(name = "detours", kind = "static")]
#[link(name = "syelog", kind = "static")]
extern "system" {
    fn DetourTransactionBegin() -> u32;
    fn DetourTransactionAbort() -> u32;
    fn DetourTransactionCommit() -> u32;
    fn DetourUpdateThread(thread: HANDLE) -> u32;
    fn DetourAttach(pointer: *mut *const c_void, detour: *mut *const c_void) -> u32;
    fn DetourDetach(pointer: *mut *const c_void, detour: *mut *const c_void) -> u32;
    fn DetourFindFunction(module: *const c_char, function: *const c_char) -> *const c_void;
}

pub struct Module {
    path: CString
}

impl Module {
    pub fn path(&self) -> &CStr {
        &self.path
    }
}

impl TryFrom<HINSTANCE> for Module {
    type Error = Box<dyn Error>;

    fn try_from(value: HINSTANCE) -> Result<Self, Self::Error> {
        let mut buffer = [0u8; MAX_PATH as usize];
        let result = unsafe { GetModuleFileNameA(value, &mut buffer) };
        if result == 0 || result == MAX_PATH {
            Err(windows::core::Error::from_win32().into())
        }
        else {
            Ok(Module {
                path: CStr::from_bytes_until_nul(buffer.as_slice())?.to_owned()
            })
        }
    }
}

fn check_result(result: u32) -> Result<(), windows::core::Error> {
    let result = WIN32_ERROR(result);
    match result {
        NO_ERROR => Ok(()),
        _ => Err(result.into())
    }
}

pub unsafe fn find_function_raw(module: &Module, function_name: &CStr) -> Option<*const c_void> {
    let result = DetourFindFunction(module.path().as_ptr(), function_name.as_ptr());
    if result.is_null() {
        None
    }
    else {
        Some(result)
    }
}

pub fn find_function<T>(module: &Module, function_name: &CStr) -> Option<T> where T: Sized {
    unsafe {
        let result = find_function_raw(module, function_name);
        result.map(|ptr| std::mem::transmute_copy(&ptr))
    }
}

fn with_transaction<F: FnOnce() -> Result<(), windows::core::Error>>(op: F) -> Result<(), windows::core::Error> {
    unsafe {
        check_result(DetourTransactionBegin())?;
        check_result(DetourUpdateThread(GetCurrentThread()))?;
        op()
            .and_then(|_| check_result(DetourTransactionCommit()))
            .map_err(|err| {
                DetourTransactionAbort();
                err
            })
    }
}

pub struct Detour<T, U> {
    source: *const T,
    target: *const U
}

impl<T, U> Detour<T, U> {
    pub fn new(source: T, target: U) -> Result<Detour<T, U>, windows::core::Error> where T: Sized, U: Sized {
        unsafe {
            let mut source = &source as *const T;
            let mut target = &target as *const U;

            with_transaction(|| {
                check_result(DetourAttach(&mut source as *mut *const T as *mut *const c_void, &mut target as *mut *const U as *mut *const c_void))
            })
            .map(|_| Detour { source, target })
        }
    }

    pub unsafe fn source(&self) -> *const T {
        self.source
    }

    pub unsafe fn target(&self) -> *const U {
        self.target
    }
}

impl<T, U> Drop for Detour<T, U> {
    fn drop(&mut self) {
        let _ = with_transaction(|| {
            unsafe {
                check_result(DetourDetach(&mut self.source as *mut *const T as *mut *const c_void, &mut self.target as *mut *const U as *mut *const c_void))
            }
        });
    }
}