use crate::{Error, Result};
use std::{path::Path, ptr};
use windows_sys::Win32::{
    Foundation::LocalFree,
    Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    },
    System::Registry::*,
};
fn crypt(input: &[u8], protect: bool) -> Result<Vec<u8>> {
    let length = u32::try_from(input.len()).map_err(|_| Error::Crypto)?;
    let source = CRYPT_INTEGER_BLOB {
        cbData: length,
        pbData: input.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: ptr::null_mut(),
    };
    // SAFETY: Input is live for this call; output is initialized; all optional
    // pointers are null. DPAPI allocates output; the current-user scope is used.
    let ok = unsafe {
        if protect {
            CryptProtectData(
                &source,
                ptr::null(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &source,
                ptr::null_mut(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
    };
    if ok == 0 || output.pbData.is_null() {
        return Err(Error::Crypto);
    }
    // SAFETY: Successful DPAPI output owns cbData initialized bytes until LocalFree.
    let bytes =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize) }.to_vec();
    // SAFETY: This allocation was returned by DPAPI and is freed exactly once.
    unsafe {
        LocalFree(output.pbData.cast());
    }
    Ok(bytes)
}
pub fn protect(bytes: &[u8]) -> Result<Vec<u8>> {
    crypt(bytes, true)
}
pub fn unprotect(bytes: &[u8]) -> Result<Vec<u8>> {
    crypt(bytes, false)
}
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
const KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const NAME: &str = "Assi";
struct RegistryKey(HKEY);
impl Drop for RegistryKey {
    fn drop(&mut self) {
        // SAFETY: This RAII value owns a successfully opened registry key and closes it once.
        unsafe {
            RegCloseKey(self.0);
        }
    }
}
fn key(create: bool) -> Result<Option<RegistryKey>> {
    let mut handle = ptr::null_mut();
    // SAFETY: Strings are nul-terminated and handle is a writable output pointer.
    let result = unsafe {
        if create {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                wide(KEY).as_ptr(),
                0,
                ptr::null(),
                0,
                KEY_SET_VALUE,
                ptr::null(),
                &mut handle,
                ptr::null_mut(),
            )
        } else {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                wide(KEY).as_ptr(),
                0,
                KEY_QUERY_VALUE | KEY_SET_VALUE,
                &mut handle,
            )
        }
    };
    if result == 2 {
        return Ok(None);
    }
    if result != 0 {
        return Err(Error::Autostart);
    }
    Ok(Some(RegistryKey(handle)))
}
pub fn autostart_enabled() -> Result<bool> {
    let Some(key) = key(false)? else {
        return Ok(false);
    };
    // SAFETY: Query only existence; nullable data pointers are permitted.
    let result = unsafe {
        RegQueryValueExW(
            key.0,
            wide(NAME).as_ptr(),
            ptr::null(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    match result {
        0 => Ok(true),
        2 => Ok(false),
        _ => Err(Error::Autostart),
    }
}
pub fn set_autostart(enabled: bool, executable: &Path) -> Result<()> {
    let Some(key) = key(enabled)? else {
        return Ok(());
    };
    if !enabled {
        // SAFETY: Owned open key and nul-terminated value name.
        let result = unsafe { RegDeleteValueW(key.0, wide(NAME).as_ptr()) };
        return if result == 0 || result == 2 {
            Ok(())
        } else {
            Err(Error::Autostart)
        };
    }
    let path = executable.canonicalize().map_err(|_| Error::Autostart)?;
    let value = path.to_str().ok_or(Error::Autostart)?;
    if value.chars().any(char::is_control) || value.contains('"') || !path.is_file() {
        return Err(Error::Autostart);
    }
    let quoted = format!("\"{value}\"");
    if quoted.encode_utf16().count() > 259 {
        return Err(Error::Autostart);
    }
    let encoded = wide(&quoted);
    // SAFETY: UTF-16 buffer includes NUL; byte length corresponds to initialized buffer.
    let result = unsafe {
        RegSetValueExW(
            key.0,
            wide(NAME).as_ptr(),
            0,
            REG_SZ,
            encoded.as_ptr().cast(),
            (encoded.len() * 2) as u32,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(Error::Autostart)
    }
}
