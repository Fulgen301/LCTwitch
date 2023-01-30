use std::{ffi::c_void, alloc::Layout, ops::{DerefMut, Deref}, mem::MaybeUninit};

use windows::{core::PCWSTR, Win32::System::Memory::LocalFree};
use windows::Win32::{Foundation::{BOOL, ERROR_OUTOFMEMORY}, System::{Diagnostics::Debug::*, Threading::GetCurrentProcess}};

pub fn check_result(result: BOOL) -> Result<(), windows::core::Error> {
    if result.as_bool() {
        Ok(())
    }
    else {
        Err(windows::core::Error::from_win32())
    }
}

struct AlignedBox<T> {
    ptr: *mut T,
    layout: Layout
}

impl<T> AlignedBox<T> {
    pub fn new_with_extra_bytes(extra_bytes: usize) -> Result<Self, Box<dyn std::error::Error>> {
        unsafe {
            Self::new_with_layout(Layout::from_size_align(std::mem::size_of::<T>() + extra_bytes, std::mem::align_of::<T>())?).map_err(|e| e.into())
        }
    }

    unsafe fn new_with_layout(layout: Layout) -> Result<Self, windows::core::Error> {
        let ptr = std::alloc::alloc(layout) as *mut T;
        if ptr.is_null() {
            Err(windows::core::Error::from(ERROR_OUTOFMEMORY))
        }
        else {
            Ok(Self {
                ptr,
                layout
            })
        }
    }

    pub fn raw(&self) -> *mut T {
        self.ptr
    }
}

impl<T> Drop for AlignedBox<T> {
    fn drop(&mut self) {
        unsafe {
            std::alloc::dealloc(self.ptr as *mut u8, self.layout);
        }
    }
}

impl<T> Deref for AlignedBox<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe {
            &*self.ptr
        }
    }
}

impl<T> DerefMut for AlignedBox<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe {
            &mut *self.ptr
        }
    }
}

pub struct Members {
    mod_base: u64,
    ptr: AlignedBox<TI_FINDCHILDREN_PARAMS>
}

impl Members {
    pub fn new(symbol_info: &SYMBOL_INFO) -> Result<Members, windows::core::Error> {
        unsafe {
            let process = GetCurrentProcess();
            let mut count: u32 = 0;
            check_result(SymGetTypeInfo(process, symbol_info.ModBase, symbol_info.TypeIndex, TI_GET_CHILDRENCOUNT, &mut count as *mut u32 as *mut c_void))?;

            let extra_bytes = std::mem::size_of::<u32>() * (count as usize - 1);
            let mut ptr = AlignedBox::<TI_FINDCHILDREN_PARAMS>::new_with_extra_bytes(extra_bytes).map_err(|_| windows::core::Error::from(ERROR_OUTOFMEMORY))?;

            ptr.Count = count;
            ptr.Start = 0;

            check_result(SymGetTypeInfo(process, symbol_info.ModBase, symbol_info.TypeIndex, TI_FINDCHILDREN, ptr.raw() as *mut c_void))?;

            Ok(Members {
                mod_base: symbol_info.ModBase,
                ptr 
            })
        }
    }
}

impl Iterator for Members {
    type Item = MemberInfo;

    fn next(&mut self) -> Option<Self::Item> {
        let try_next = |child: u32| -> Result<Self::Item, Box<dyn std::error::Error>> {
            unsafe {
                let mut offset = MaybeUninit::<u32>::uninit();
                check_result(SymGetTypeInfo(GetCurrentProcess(), self.mod_base, child, TI_GET_OFFSET, offset.as_mut_ptr() as *mut c_void))?;
                let offset = offset.assume_init();

                let mut name_ptr = MaybeUninit::<*mut u16>::uninit();
                check_result(SymGetTypeInfo(GetCurrentProcess(), self.mod_base, child, TI_GET_SYMNAME, name_ptr.as_mut_ptr() as *mut c_void))?;
                let name_ptr = name_ptr.assume_init();

                let result = PCWSTR::from_raw(name_ptr).to_string();
                LocalFree(name_ptr as isize);
                let name = result?;

                let mut symbol_info = SYMBOL_INFO {
                    SizeOfStruct: std::mem::size_of::<SYMBOL_INFO>() as u32,
                    ..Default::default()
                };

                check_result(SymFromIndex(GetCurrentProcess(), self.mod_base, child, &mut symbol_info as *mut SYMBOL_INFO))?;

                Ok(MemberInfo {
                    name,
                    offset,
                    symbol_info
                })
            }
        };

        while self.ptr.Start < self.ptr.Count {
            let result = try_next(unsafe { *self.ptr.ChildId.get_unchecked(self.ptr.Start as usize) }).ok();

            self.ptr.Start += 1;

            if result.is_some() {
                return result;
            }
        }

        None
    }
}

pub struct MemberInfo {
    name: String,
    offset: u32,
    symbol_info: SYMBOL_INFO
}

impl MemberInfo {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn offset(&self) -> usize {
        self.offset as usize
    }

    pub fn symbol_info(&self) -> &SYMBOL_INFO {
        &self.symbol_info
    }
}