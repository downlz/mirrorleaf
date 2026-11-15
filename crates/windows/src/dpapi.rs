//! The database key under Windows' Data Protection API (DPAPI), the Keychain's
//! role on the Mac (M11): Windows encrypts it with a key derived from the
//! user's login, so only this Windows user on this PC can read it back.

use windows::Win32::Foundation::{HLOCAL, LocalFree};
use windows::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
};
use windows::core::PCWSTR;

/// `data`, encrypted for this Windows user.
pub fn protect(data: &[u8]) -> windows::core::Result<Vec<u8>> {
    let input = blob(data);
    let mut output = CRYPT_INTEGER_BLOB::default();
    // SAFETY: `input` points at `data`, alive for the call; Windows allocates
    // `output`, which `take` copies and frees.
    unsafe {
        CryptProtectData(
            &input,
            PCWSTR::null(),
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )?;
        Ok(take(output))
    }
}

/// Data `protect` encrypted, for the same Windows user.
pub fn unprotect(data: &[u8]) -> windows::core::Result<Vec<u8>> {
    let input = blob(data);
    let mut output = CRYPT_INTEGER_BLOB::default();
    // SAFETY: as in `protect`.
    unsafe {
        CryptUnprotectData(
            &input,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )?;
        Ok(take(output))
    }
}

fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr().cast_mut(),
    }
}

/// Copies a blob Windows allocated, then frees it.
unsafe fn take(blob: CRYPT_INTEGER_BLOB) -> Vec<u8> {
    if blob.pbData.is_null() {
        return Vec::new();
    }
    // SAFETY: Windows returned `cbData` bytes at `pbData`, allocated with
    // LocalAlloc.
    unsafe {
        let bytes = std::slice::from_raw_parts(blob.pbData, blob.cbData as usize).to_vec();
        LocalFree(Some(HLOCAL(blob.pbData.cast())));
        bytes
    }
}
