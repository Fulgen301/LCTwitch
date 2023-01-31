#![feature(abi_thiscall)]
#![feature(allocator_api)]
#![feature(async_closure)]
#![feature(cstr_from_bytes_until_nul)]
#![recursion_limit = "256"]

use std::{ffi::{CStr, c_char, CString, NulError}, error::Error};
use std::sync::Arc;

use byte_strings::c_str;
use detour::{find_function, Module};
use script::Script;
use window::WindowSubclass;
use windows::{Win32::{System::{LibraryLoader::GetModuleHandleW, Threading::{GetCurrentThread, GetCurrentProcess, WaitForSingleObject}, Diagnostics::Debug::*}, Foundation::{BOOL, HANDLE, HWND, WPARAM, LPARAM, LRESULT, HINSTANCE, DuplicateHandle, DUPLICATE_SAME_ACCESS}, UI::{WindowsAndMessaging::{EnumWindows, GetWindowLongPtrW, GWLP_HINSTANCE, GetClassNameW, WM_USER, PostMessageA}, Shell::DefSubclassProc}}, core::PWSTR};

pub mod dbghelp;
pub mod detour;
pub mod export;
pub mod http;
pub mod script;
pub mod window;

type FnLog = extern "C" fn(*const c_char) -> bool;

const WM_LCTWITCH_CALLBACK: u32 = WM_USER + 10;

extern "system" fn subclass_proc(window: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _subclass_id: usize, _ref_data: usize) -> LRESULT {
    if msg == WM_LCTWITCH_CALLBACK {
        unsafe {
            let ptr = Box::from_raw(std::mem::transmute::<_, *mut dyn FnOnce()>((wparam, lparam)));
            ptr();
        }

        return LRESULT(0);
    }

    unsafe { DefSubclassProc(window, msg, wparam, lparam) }
}

pub struct LCTwitchMainThread {
    handle: HANDLE,
    main_window_subclass: WindowSubclass
}

extern "system" fn is_main_window(window: HWND, param: LPARAM) -> BOOL {
    unsafe {
        let arguments = &mut *std::mem::transmute::<_, *mut (HINSTANCE, HWND)>(param);
        if HINSTANCE(GetWindowLongPtrW(window, GWLP_HINSTANCE)) != arguments.0 {
            return true.into();
        }

        let mut buffer = [0u16; 13];
        if GetClassNameW(window, &mut buffer[0..12]) == 0 {
            return true.into();
        }

        if !PWSTR::from_raw(buffer.as_mut_ptr()).to_string()
            .map_or_else(
                |_| false,
            |s| s == "C4Fullscreen") {
                arguments.1 = window;
                return false.into();
            }
        
        return true.into();
    }
}

impl LCTwitchMainThread {
    pub fn new() -> Result<LCTwitchMainThread, Box<dyn std::error::Error>> {
        let clonk_handle = unsafe { GetModuleHandleW(None)? };

        let mut arguments = (clonk_handle, Default::default());
        unsafe { EnumWindows(Some(is_main_window), LPARAM(&mut arguments as *mut (HINSTANCE, HWND) as isize)) };

        if arguments.1 == Default::default() {
            return Err("Could not find window handle".into());
        }

        let mut handle: HANDLE = Default::default();

        if !unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                GetCurrentThread(),
                GetCurrentProcess(),
                &mut handle,
                0,
                false,
                DUPLICATE_SAME_ACCESS
            )
        }.as_bool() {
            return Err("Could not duplicate thread handle".into());
        }

        Ok(LCTwitchMainThread{
            handle,
            main_window_subclass: WindowSubclass::new(arguments.1, subclass_proc, 1, 0)?
        })
    }
}

pub struct LCTwitch {
    main_thread_struct: LCTwitchMainThread,
    log: FnLog,
    script: Script
}

impl LCTwitch {
    pub fn new(main_thread_struct: LCTwitchMainThread) -> Result<LCTwitch, Box<dyn std::error::Error>> {
        unsafe {
            SymSetOptions(SYMOPT_UNDNAME | SYMOPT_DEFERRED_LOADS | SYMOPT_LOAD_ANYTHING);
            if !SymInitialize(GetCurrentProcess(), None, true).as_bool() {
                return Err(windows::core::Error::from_win32().into());
            }
        }

        let clonk_handle = unsafe { GetModuleHandleW(None)? };
        let clonk_module = Module::try_from(clonk_handle)?;

        let log = find_function::<FnLog>(&clonk_module, c_str!("Log")).ok_or("Failed to find Log")?;
        log(c_str!("Hello from Rust").as_ptr());

        let script = Script::new(&clonk_module)?;

        Ok(LCTwitch {
            main_thread_struct,
            log,
            script
        })
    }

    pub fn log(&self, message: &str) -> Result<(), NulError> {
        (self.log)(CString::new(message)?.as_ptr());
        Ok(())
    }

    pub fn log_cstr(&self, message: &CStr) {
        (self.log)(message.as_ptr());
    }

    pub fn run_in_main_thread<F: FnOnce() + Send>(&self, op: F) {
        let window = self.main_thread_struct.main_window_subclass.window();
        let ptr = Box::new(op) as Box<dyn FnOnce() -> ()>;

        unsafe {
            let raw = Box::into_raw(ptr);
            let fat_pointer = std::mem::transmute::<_, (WPARAM, LPARAM)>(raw);

            PostMessageA(window, WM_LCTWITCH_CALLBACK, fat_pointer.0, fat_pointer.1);
        }
    }
}

impl Drop for LCTwitch {
    fn drop(&mut self) {
    }
}

pub fn start(main_thread: LCTwitchMainThread) -> Result<(), Box<dyn Error>> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    runtime.block_on(main(main_thread))
}

pub async fn main(main_thread: LCTwitchMainThread) -> Result<(), Box<dyn Error>> {
    let main_thread_handle = main_thread.handle;
    let twitch = Arc::new(LCTwitch::new(main_thread)?);

    let (tx, rx) = tokio::sync::oneshot::channel::<()>();

    tokio::spawn(async move {
        unsafe {
            WaitForSingleObject(main_thread_handle, u32::MAX);
        }

        tx.send(()).unwrap();
    });

    crate::http::run_server(twitch.clone(), rx).await;
    Ok(())
}