
use std::{error::Error, mem::MaybeUninit};
use windows::{core::w, Win32::System::Registry::{RegCloseKey,RegOpenKeyExW, RegQueryValueExW, HKEY_CURRENT_USER, HKEY, KEY_READ}};

pub struct Config {
    port: u16
}

impl Config {
    pub fn new() -> Result<Config, Box<dyn Error>> {
        let port = unsafe {
            Self::read_port_from_registry().ok()
        }.unwrap_or_else(|| 11116);
        
        Ok(Config {
            port
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    unsafe fn read_port_from_registry() -> Result<u16, Box<dyn Error>> {
        let key = {
            let mut key = MaybeUninit::<HKEY>::uninit();
            RegOpenKeyExW(HKEY_CURRENT_USER, w!("Software\\LegacyClonk Team\\LCTwitch"), 0, KEY_READ, key.as_mut_ptr()).ok()?;
            key.assume_init()
        };

        let port = {
            let mut port = MaybeUninit::<u32>::uninit();
            let mut size = std::mem::size_of::<u32>() as u32;
            RegQueryValueExW(key, w!("HttpServerPort"), None, None, Some(port.as_mut_ptr() as *mut u8), Some(&mut size as *mut _)).ok().map(|_| port.assume_init())
        };

        RegCloseKey(key);

        port.map_err(|e| e.into())
        .and_then(|port| u16::try_from(port).map_err(|e| e.into()))
    }
}

unsafe impl Send for Config {}
unsafe impl Sync for Config {}