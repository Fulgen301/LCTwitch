use crate::LCTwitchMainThread;

use std::ffi::c_void;

use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_OK};
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::System::LibraryLoader::{GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_PIN, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS};
use windows::Win32::System::SystemServices::{DLL_PROCESS_ATTACH, DLL_PROCESS_DETACH};
use windows::Win32::Foundation::HINSTANCE;

#[no_mangle]
#[allow(non_snake_case)]
unsafe extern "system" fn DllMain(_: HINSTANCE, reason: u32, _reserved: *const c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        let mut handle = HINSTANCE(0);
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_PIN | GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, 
            PCWSTR::from_raw(DllMain as *const () as *const u16),
            &mut handle as *mut HINSTANCE
        );

        let main_thread_struct = LCTwitchMainThread::new();
        if let Ok(main_thread_struct) = main_thread_struct {    
            // DllMain _must not_ call LoadLibrary since the loader lock is being held.
            // Since we cannot guarantee that no crate will ever call that function,
            // do the instance initialization in its own thread.
            std::thread::spawn(move || {
                match crate::start(main_thread_struct) {
                    Ok(_) => {},
                    Err(err) => {
                        let message = err.to_string();
                        unsafe {
                            MessageBoxW(None, &HSTRING::from(message), w!("Error"), MB_OK);
                        }
                        panic!("{}", err.to_string());
                    }
                }
            });

            return 1;
        }

        if cfg!(debug_assertions) {
            panic!();
        }
        else {
            return 0;
        }
    }
    else if reason == DLL_PROCESS_DETACH {
    }

    1
}