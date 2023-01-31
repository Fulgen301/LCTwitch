use std::{ffi::{CString, c_char, c_void, CStr}, error::Error, mem::MaybeUninit, cell::RefCell, ops::{Deref, DerefMut}};

use byte_strings::c_str;
use cpp::*;

use crate::{LCTwitch, detour::{self, Module}, dbghelp::{self, Members}, http::{ErrorCode}};
use windows::{core::PCSTR, Win32::System::{LibraryLoader::GetModuleHandleA}};
use windows::Win32::System::{Diagnostics::Debug::*, Threading::GetCurrentProcess};

type C4AulScriptEngine = c_void;
type C4Config = c_void;
type C4Game = c_void;
type C4GameControl = c_void;
type C4ControlScript = c_void;

#[repr(C)]
#[derive(Clone, Copy, PartialEq)]
pub enum C4AulScriptStrict {
    Strict3 = 3
}

pub enum ScriptError {
    Code(ErrorCode), 
    Box(Box<dyn Error + Send + Sync>)
}

impl From<ErrorCode> for ScriptError {
    fn from(value: ErrorCode) -> Self {
        Self::Code(value)
    }
}

impl<T> From<T> for ScriptError where T: Into<Box<dyn Error + Send + Sync>> {
    fn from(value: T) -> Self {
        Self::Box(value.into())
    }
}

impl std::fmt::Display for ScriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Code(code) => code.fmt(f),
            Self::Box(boxed) => boxed.fmt(f)
        }
    }
}

#[repr(C)]
struct C4Value {
    data: usize,
    next: *mut C4Value,
    first_ref: *mut C4Value,
    owning_map: *mut c_void,
    type_: u8,
    __padding: [u8; 3],
    has_base_container: bool
}

unsafe impl Send for C4Value {}

#[repr(C)]
struct StdStrBuf {
    is_ref: bool,
    data: *mut c_char,
    size: usize
}

struct ExecuteInfo {
    control_script_size: usize,
    script_offset: usize,
    script_engine: *mut C4AulScriptEngine,
    direct_exec: extern "win64" fn(*mut C4AulScriptEngine, *const c_void, *const c_char, *const c_char, bool, C4AulScriptStrict) -> C4Value,
    get_data_string: extern "win64" fn(*const C4Value) -> StdStrBuf,
    c4value_destructor: extern "win64" fn(*mut C4Value),
    stdstrbuf_destructor: extern "win64" fn(*mut StdStrBuf),
    grab_pointer: extern "win64" fn(*mut StdStrBuf),
    value_reply: Option<tokio::sync::oneshot::Sender<Result<AutoFree<c_char>, ScriptError>>>
}

const VTABLE_ENTRIES: usize = 7;
const VTABLE_EXECUTE: usize = 3;


cpp!{{
    #pragma pointers_to_members(full_generality, single_inheritance)
    
    struct C4Value
    {
        char data[40];
    };

    struct StdStrBuf
    {
        bool fRef;
        const char *pData;
        size_t iSize;
    };

    class C4AulScriptEngine;
    using DirectExecFunc = C4Value(C4AulScriptEngine::*)(const void *, const char *, const char *, bool, std::int32_t);
    using GetDataStringFunc = StdStrBuf(C4Value::*)();
    using C4ValueDestructorFunc = void(C4Value::*)();
    using StdStrBufDestructorFunc = void(StdStrBuf::*)();
    using GrabPointerFunc = void *(StdStrBuf::*)();
    
    }}

#[repr(transparent)]
struct AutoFree<T>(*mut T);

impl<T> Drop for AutoFree<T> {
    fn drop(&mut self) {
        let ptr = self.0;
        cpp!(unsafe [ptr as "void *"] {
            free(ptr);
        });
    }
}

impl<T> Deref for AutoFree<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.0 }
    }
}

impl<T> DerefMut for AutoFree<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.0 }
    }
}

unsafe impl<T> Send for AutoFree<T> {}

pub struct Script {
    is_running: *const bool,
    game_control: *mut C4GameControl,
    constructor: extern "win64" fn(*mut C4ControlScript),
    buf_copy: extern "win64" fn(*mut c_void),
    do_input: extern "win64" fn(*mut C4GameControl, i32, *mut C4ControlScript, i32),
    vtable: *const *const c_void,
    modified_vtable: [*const c_void; VTABLE_ENTRIES + 2],
    target_obj_offset: usize,
    
    control_mode: *const i32,
    allow_scripting_in_replays: *const bool,

    league_address: *const StdStrBuf,

    is_host: *const bool,
    network_enabled: *const bool,
    
    execute_info: ExecuteInfo
}

impl Script {
    pub fn new(clonk_module: &Module) -> Result<Script, Box<dyn Error>> {
        let clonk_base_address = unsafe { GetModuleHandleA(PCSTR::null())? }.0 as u64;

        let find_member_offset = |members: &mut Members, name: &'static str| members.find(|info| info.name() == name).ok_or(name).map(|info| info.offset());

        let symbol_info = RefCell::new(SYMBOL_INFO {
            SizeOfStruct: std::mem::size_of::<SYMBOL_INFO>() as u32,
            ..Default::default()
        });

        let member = |base: *const c_void, name: &'static str| -> Result<*const c_void, Box<dyn Error>> {
            unsafe {
                Ok(base.add(find_member_offset(&mut Members::new(&*symbol_info.borrow_mut())?, name)?))
            }
        };

        let control_script_size = MaybeUninit::<u64>::uninit();

        unsafe {
            dbghelp::check_result(SymGetTypeFromName(GetCurrentProcess(), clonk_base_address, PCSTR::from_raw(c_str!("C4ControlScript").as_ptr() as *mut u8), symbol_info.as_ptr()))?;
            dbghelp::check_result(SymGetTypeInfo(GetCurrentProcess(), clonk_base_address, symbol_info.borrow().TypeIndex, TI_GET_LENGTH, control_script_size.as_ptr() as *mut c_void))?;
        }

        let control_script_size = unsafe { control_script_size.assume_init() } as usize;

        let target_obj_offset = find_member_offset(&mut Members::new(&*symbol_info.borrow_mut())?, "iTargetObj")?;

        let script_offset = find_member_offset(&mut Members::new(&*symbol_info.borrow_mut())?, "Script")?;

        unsafe {
            dbghelp::check_result(SymGetTypeFromName(GetCurrentProcess(), clonk_base_address, PCSTR::from_raw(c_str!("C4Game").as_ptr() as *mut u8), symbol_info.as_ptr()))?;
        }

        let game = detour::find_function::<*mut C4Game>(clonk_module, c_str!("Game")).ok_or("Game")?;
        let is_running = member(game, "IsRunning")? as *const bool;
        let game_control = member(game, "Control")? as *mut _;
        let network = member(game, "Network")?;
        let game_parameters = member(game, "Parameters")? as *mut _;
        let script_engine = member(game, "ScriptEngine")? as *mut _;

        unsafe {
            dbghelp::check_result(SymGetTypeFromName(GetCurrentProcess(), clonk_base_address, PCSTR::from_raw(c_str!("C4GameControl").as_ptr() as *mut u8), symbol_info.as_ptr()))?;
        }

        let control_mode = member(game_control, "eMode")? as *const i32;

        unsafe {
            dbghelp::check_result(SymGetTypeFromName(GetCurrentProcess(), clonk_base_address, PCSTR::from_raw(c_str!("C4GameParameters").as_ptr() as *mut u8), symbol_info.as_ptr()))?;
        }

        let league_address = member(game_parameters, "LeagueAddress")? as *const StdStrBuf;

        let config = detour::find_function::<*mut C4Config>(clonk_module, c_str!("Config")).ok_or("Config")?;

        unsafe {
            dbghelp::check_result(SymGetTypeFromName(GetCurrentProcess(), clonk_base_address, PCSTR::from_raw(c_str!("C4Config").as_ptr() as *mut u8), symbol_info.as_ptr()))?;
        }

        let general = member(config, "General")?;

        unsafe {
            dbghelp::check_result(SymGetTypeFromName(GetCurrentProcess(), clonk_base_address, PCSTR::from_raw(c_str!("C4ConfigGeneral").as_ptr() as *mut u8), symbol_info.as_ptr()))?;
        }

        let allow_scripting_in_replays = member(general, "AllowScriptingInReplays")? as *const bool;

        unsafe {
            dbghelp::check_result(SymGetTypeFromName(GetCurrentProcess(), clonk_base_address, PCSTR::from_raw(c_str!("C4Network2").as_ptr() as *mut u8), symbol_info.as_ptr()))?;
        }

        let is_host = member(network, "fHost")? as *const bool;
        let status = member(network, "Status")?;

        unsafe {
            dbghelp::check_result(SymGetTypeFromName(GetCurrentProcess(), clonk_base_address, PCSTR::from_raw(c_str!("C4Network2Status").as_ptr() as *mut u8), symbol_info.as_ptr()))?;
        }

        let network_enabled = member(status, "eState")? as *const bool;


        let mut obj = Self {
            game_control,
            is_running,
            constructor: detour::find_function(clonk_module, c_str!("C4ControlPacket::C4ControlPacket")).ok_or("C4ControlPacket::C4ControlPacket")?,
            buf_copy: detour::find_function(clonk_module, c_str!("StdStrBuf::Copy")).ok_or("StdStrBuf::Copy")?,
            do_input: detour::find_function(clonk_module, c_str!("C4GameControl::DoInput")).ok_or("C4GameControl::DoInput")?,
            vtable: detour::find_function(clonk_module, c_str!("C4ControlScript::`vftable'")).ok_or("vftable")?,
            modified_vtable: [std::ptr::null(); VTABLE_ENTRIES + 2],
            target_obj_offset,
            control_mode,
            allow_scripting_in_replays,
            league_address,
            is_host,
            network_enabled,

            execute_info: ExecuteInfo {
                control_script_size,
                script_offset,
                script_engine,
                direct_exec: detour::find_function(clonk_module, c_str!("C4AulScript::DirectExec")).ok_or("C4AulScript::DirectExec")?,
                get_data_string: detour::find_function(clonk_module, c_str!("C4Value::GetDataString")).ok_or("C4Value::GetDataString")?,
                c4value_destructor: detour::find_function(clonk_module, c_str!("C4Value::~C4Value")).ok_or("C4Value::~C4Value")?,
                stdstrbuf_destructor: detour::find_function(clonk_module, c_str!("StdStrBuf::~StdStrBuf")).ok_or("StdStrBuf::~StdStrBuf")?,
                grab_pointer: detour::find_function(clonk_module, c_str!("StdStrBuf::GrabPointer")).ok_or("StdStrBuf::GrabPointer")?,
                value_reply: None
            },
        };

        obj.prepare_vtable();
        Ok(obj)
    }

    fn prepare_vtable(&mut self) {
        unsafe {
            std::ptr::copy_nonoverlapping(self.vtable.offset(-1), self.modified_vtable.as_mut_ptr(), VTABLE_ENTRIES + 1);
        }

        self.modified_vtable[1 + VTABLE_EXECUTE] = control_script_execute as *const c_void;

        self.modified_vtable[1 + VTABLE_ENTRIES] = std::ptr::null();
    }

    fn set_execute_info(vtable: &mut [*const c_void], execute_info: Box<ExecuteInfo>) {
        let execute_info = Box::into_raw(execute_info);
        vtable[1 + VTABLE_ENTRIES] = execute_info as *const c_void;
    }

    pub async fn run_script(&self, instance: &LCTwitch, script: &str) -> Result<String, ScriptError> {
        unsafe {
            if !*self.is_running {
                return Err(ErrorCode::NoScenario.into());
            }

            if *self.network_enabled && !*self.is_host {
                return Err(ErrorCode::NotHost.into());
            }

            if *self.control_mode == 3 && !*self.allow_scripting_in_replays {
                return Err(ErrorCode::NoScriptingInReplays.into());
            }

            if (*self.league_address).size > 0 {
                return Err(ErrorCode::LeagueActive.into());
            }
        }

        let script = CString::new(script)?;

        let (tx, rx) = tokio::sync::oneshot::channel::<Result<AutoFree<c_char>, ScriptError>>();

        instance.run_in_main_thread(move || {
            let allocated_size = self.execute_info.control_script_size;

            let memory = cpp!(unsafe [allocated_size as "std::size_t"] -> *mut c_void as "void *" {
                return ::operator new(allocated_size, std::nothrow);
            });
    
            if memory.is_null() {
                return;
            }

            unsafe {
                memory.write_bytes(0, allocated_size);
            
                (self.constructor)(memory);

                let mut modified_vtable = self.modified_vtable;

                let execute_info = Box::new(ExecuteInfo {
                    value_reply: Some(tx),
                    ..self.execute_info
                });

                Self::set_execute_info(modified_vtable.as_mut_slice(), execute_info);

                (memory as *mut *const *const c_void).write(modified_vtable.as_ptr().add(1));

                (memory.add(self.target_obj_offset) as *mut i32).write(-2);
                
                
                let script_buf = memory.add(self.execute_info.script_offset);
                (script_buf as *mut u8).write(1);
                //(memory.add(self.execute_info.script_offset) as *mut u8).write(1);

                let bytes = script.as_bytes_with_nul();
                (script_buf.add(8) as *mut *const c_char).write(bytes.as_ptr() as *const c_char);
                (script_buf.add(16) as *mut usize).write(bytes.len());
                (self.buf_copy)(script_buf);

                (self.do_input)(self.game_control, 0x80 | 0x08, memory, 4);
            }
        });

        rx.await?.and_then(|value| {
            unsafe {
                CStr::from_ptr(value.0).to_str()
                .map(|s| s.to_owned())
                .map_err(|e| e.into())
            }
        })
    }
}

unsafe impl Send for Script {}
unsafe impl Sync for Script {}

pub extern "win64" fn control_script_execute(control: *mut C4ControlScript) {
    let execute_info = unsafe {
        let vtable = *(control as *const *const *const c_void);
        let execute_info = vtable.add(VTABLE_ENTRIES).read();
        &mut *(execute_info as *mut ExecuteInfo)
    };

    let script = unsafe {
        (&*((control as *const u8).add(execute_info.script_offset) as *const StdStrBuf)).data
    };

    let script_engine = execute_info.script_engine;
    let direct_exec = execute_info.direct_exec as *const c_void;
    let context = b"LCTwitch\0".as_ptr() as *const i8;
    let strictness = C4AulScriptStrict::Strict3;

    let buf = {
        let mut buf = MaybeUninit::<AutoFree<c_char>>::uninit();
        let buf_ptr = buf.as_mut_ptr();
        let get_data_string = execute_info.get_data_string as *const c_void;
        let c4value_destructor = execute_info.c4value_destructor as *const c_void;
        let stdstrbuf_destructor = execute_info.stdstrbuf_destructor as *const c_void;
        let grab_pointer = execute_info.grab_pointer as *const c_void;

        cpp!(unsafe [script_engine as "C4AulScriptEngine *", direct_exec as "DirectExecFunc", context as "const char *", script as "const char *", strictness as "std::int32_t", buf_ptr as "void **", get_data_string as "GetDataStringFunc", c4value_destructor as "C4ValueDestructorFunc", stdstrbuf_destructor as "StdStrBufDestructorFunc", grab_pointer as "GrabPointerFunc"] {
            C4Value value{(script_engine->*direct_exec)(nullptr, script, context, false, strictness)};
            StdStrBuf buf{(value.*get_data_string)()};

            *buf_ptr = (buf.*grab_pointer)();
            (buf.*stdstrbuf_destructor)();
            (value.*c4value_destructor)();
        });

        unsafe { buf.assume_init() }
    };

    let value_reply = std::mem::replace(&mut execute_info.value_reply, None);
    let _ = value_reply.unwrap().send(Ok(buf));
}