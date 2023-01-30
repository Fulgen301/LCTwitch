use windows::Win32::{Foundation::*, UI::Shell::{SetWindowSubclass, RemoveWindowSubclass}};

type SubclassProc = unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM, usize, usize) -> LRESULT;

pub struct WindowSubclass {
    window: HWND,
    subclass_proc: SubclassProc,
    id: usize
}

impl WindowSubclass {
    pub fn new(window: HWND, subclass_proc: SubclassProc, id: usize, ref_data: usize) -> Result<WindowSubclass, windows::core::Error> {
        unsafe {
            if SetWindowSubclass(window, Some(subclass_proc), id, ref_data).as_bool() {
                Ok(WindowSubclass { window, subclass_proc, id})
            }
            else {
                Err(windows::core::Error::from_win32())
            }
        }
    }

    pub fn window(&self) -> HWND {
        self.window
    }

    pub fn id(&self) -> usize {
        self.id
    }
}

impl Drop for WindowSubclass {
    fn drop(&mut self) {
        unsafe {
            RemoveWindowSubclass(self.window, Some(self.subclass_proc), self.id);
        }
    }
}