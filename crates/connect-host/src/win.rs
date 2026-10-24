//! Windows: who the current user is, a pipe only that user can open, who
//! owns a pipe we connected to, and the per-user registry keys the
//! browsers read host manifests from. Everything is HKEY_CURRENT_USER and
//! the user's own token: no administrator rights anywhere.

use std::ffi::c_void;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::RawHandle;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{
    CloseHandle, LocalFree, ERROR_FILE_NOT_FOUND, ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, HANDLE,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo,
    SDDL_REVISION_1, SE_KERNEL_OBJECT,
};
use windows_sys::Win32::Security::{
    EqualSid, GetTokenInformation, IsWellKnownSid, TokenUser, WinBuiltinAdministratorsSid,
    OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegGetValueW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

fn wide(s: &str) -> Vec<u16> {
    std::ffi::OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

fn win_err(code: u32) -> io::Error {
    io::Error::from_raw_os_error(code as i32)
}

/// The TOKEN_USER of this process, in a buffer that keeps the SID alive.
struct TokenUserBuf(Vec<u64>);

impl TokenUserBuf {
    fn read() -> io::Result<TokenUserBuf> {
        unsafe {
            let mut token: HANDLE = null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return Err(io::Error::last_os_error());
            }
            let mut need: u32 = 0;
            GetTokenInformation(token, TokenUser, null_mut(), 0, &mut need);
            let first = io::Error::last_os_error();
            if need == 0 || first.raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32) {
                CloseHandle(token);
                return Err(first);
            }
            // u64 storage: the structure holds a pointer and must be aligned.
            let mut buf = vec![0u64; (need as usize).div_ceil(8)];
            let ok = GetTokenInformation(token, TokenUser, buf.as_mut_ptr() as *mut c_void, need, &mut need);
            let err = io::Error::last_os_error();
            CloseHandle(token);
            if ok == 0 {
                return Err(err);
            }
            Ok(TokenUserBuf(buf))
        }
    }

    fn sid(&self) -> PSID {
        unsafe { (*(self.0.as_ptr() as *const TOKEN_USER)).User.Sid }
    }
}

/// The current user's SID as text ("S-1-5-21-...").
pub fn user_sid_string() -> io::Result<String> {
    let user = TokenUserBuf::read()?;
    unsafe {
        let mut text: *mut u16 = null_mut();
        if ConvertSidToStringSidW(user.sid(), &mut text) == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut len = 0usize;
        while *text.add(len) != 0 {
            len += 1;
        }
        let out = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
        LocalFree(text as *mut c_void);
        Ok(out)
    }
}

/// Security attributes for a pipe that only the current user may open:
/// a protected access list with one entry. The default list of a pipe
/// also lets everyone else on the machine open it for reading.
pub struct PipeSecurity {
    descriptor: PSECURITY_DESCRIPTOR,
    attributes: SECURITY_ATTRIBUTES,
}

// The descriptor is a private heap block that nothing else touches.
unsafe impl Send for PipeSecurity {}
unsafe impl Sync for PipeSecurity {}

impl PipeSecurity {
    pub fn for_current_user() -> io::Result<PipeSecurity> {
        let sid = user_sid_string()?;
        let sddl = wide(&format!("D:P(A;;GA;;;{sid})"));
        let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut descriptor, null_mut())
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(PipeSecurity {
            descriptor,
            attributes: SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor,
                bInheritHandle: 0,
            },
        })
    }

    /// For `ServerOptions::create_with_security_attributes_raw`.
    pub fn as_raw(&mut self) -> *mut c_void {
        &mut self.attributes as *mut SECURITY_ATTRIBUTES as *mut c_void
    }
}

impl Drop for PipeSecurity {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.descriptor);
        }
    }
}

/// Was the pipe behind this handle created by the current user? A pipe
/// name is machine-wide, so another account could create ours first and
/// wait for the helper; the helper hangs up on a pipe it does not own.
/// (An elevated process of the same person owns its objects as the
/// Administrators group; that counts as the same person.)
pub fn pipe_owner_is_current_user(handle: RawHandle) -> io::Result<bool> {
    let user = TokenUserBuf::read()?;
    unsafe {
        let mut owner: PSID = null_mut();
        let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
        let rc = GetSecurityInfo(
            handle as HANDLE,
            SE_KERNEL_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            null_mut(),
            null_mut(),
            &mut descriptor,
        );
        if rc != ERROR_SUCCESS {
            return Err(win_err(rc));
        }
        let same = !owner.is_null()
            && (EqualSid(owner, user.sid()) != 0 || IsWellKnownSid(owner, WinBuiltinAdministratorsSid) != 0);
        LocalFree(descriptor);
        Ok(same)
    }
}

// --- registry (HKEY_CURRENT_USER only) ---

struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            RegCloseKey(self.0);
        }
    }
}

/// Set the default value of `HKCU\<subkey>` (created when missing).
pub fn reg_set_default(subkey: &str, value: &str) -> io::Result<()> {
    let path = wide(subkey);
    let mut key: HKEY = null_mut();
    let rc = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            path.as_ptr(),
            0,
            null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            null(),
            &mut key,
            null_mut(),
        )
    };
    if rc != ERROR_SUCCESS {
        return Err(win_err(rc));
    }
    let key = Key(key);
    let data = wide(value);
    let rc = unsafe { RegSetValueExW(key.0, null(), 0, REG_SZ, data.as_ptr() as *const u8, (data.len() * 2) as u32) };
    if rc != ERROR_SUCCESS {
        return Err(win_err(rc));
    }
    Ok(())
}

/// The default value of `HKCU\<subkey>`; None when the key is not there.
pub fn reg_get_default(subkey: &str) -> io::Result<Option<String>> {
    let path = wide(subkey);
    let mut size: u32 = 0;
    let rc = unsafe { RegGetValueW(HKEY_CURRENT_USER, path.as_ptr(), null(), RRF_RT_REG_SZ, null_mut(), null_mut(), &mut size) };
    if rc == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if rc != ERROR_SUCCESS {
        return Err(win_err(rc));
    }
    let mut buf = vec![0u16; (size as usize).div_ceil(2) + 1];
    let mut size = (buf.len() * 2) as u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            path.as_ptr(),
            null(),
            RRF_RT_REG_SZ,
            null_mut(),
            buf.as_mut_ptr() as *mut c_void,
            &mut size,
        )
    };
    if rc == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if rc != ERROR_SUCCESS {
        return Err(win_err(rc));
    }
    let len = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
    Ok(Some(String::from_utf16_lossy(&buf[..len])))
}

/// Delete `HKCU\<subkey>` and what is under it. Ok(false) when it was
/// not there.
pub fn reg_delete(subkey: &str) -> io::Result<bool> {
    let path = wide(subkey);
    match unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, path.as_ptr()) } {
        ERROR_SUCCESS => Ok(true),
        ERROR_FILE_NOT_FOUND => Ok(false),
        rc => Err(win_err(rc)),
    }
}
