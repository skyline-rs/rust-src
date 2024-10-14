//! System bindings for the wasm/web platform
//!
//! This module contains the facade (aka platform-specific) implementations of
//! OS level functionality for wasm. Note that this wasm is *not* the emscripten
//! wasm, so we have no runtime here.
//!
//! This is all super highly experimental and not actually intended for
//! wide/production use yet, it's still all in the experimental category. This
//! will likely change over time.
//!
//! Currently all functions here are basically stubs that immediately return
//! errors. The hope is that with a portability lint we can turn actually just
//! remove all this and just omit parts of the standard library if we're
//! compiling for wasm. That way it's a compile time error for something that's
//! guaranteed to be a runtime error!

use crate::os::raw::c_char;
use crate::io::ErrorKind;

//global_asm!("
//.section .nro_header
//.global __nro_header_start
//.word 0
//.word _mod_header
//.word 0
//.word 0
//
//.section .rodata.mod0
//.global _mod_header
//_mod_header:
//    .ascii \"MOD0\"
//    .word __dynamic_start - _mod_header
//    .word __bss_start - _mod_header
//    .word __bss_end - _mod_header
//    .word __eh_frame_hdr_start - _mod_header
//    .word __eh_frame_hdr_end - _mod_header
//    .word __nx_module_runtime - _mod_header // runtime-generated module object offset
//.global IS_NRO
//IS_NRO:
//    .word 1
//
//.section .bss.module_runtime
//.space 0xD0
//");

pub mod args;
pub mod env;
pub mod fs;
pub mod io;
pub mod memchr;
pub mod net;
pub mod os;
pub mod pipe;
pub mod process;
pub mod stdio;
pub mod thread;
pub mod time;

pub mod fd;

// pub use crate::sys_common::os_str_bytes as os_str;

// cfg_if::cfg_if! {
//     if #[cfg(target_feature = "atomics")] {
//         #[path = "condvar_atomics.rs"]
//         pub mod condvar;
//         #[path = "mutex_atomics.rs"]
//         pub mod mutex;
//         #[path = "rwlock_atomics.rs"]
//         pub mod rwlock;
//     } else {
//         pub mod condvar;
//         pub mod mutex;
//         pub mod rwlock;
//     }
// }

#[cfg(not(test))]
pub fn init(_argc: isize, _argv: *const *const u8, _sigpipe: u8) {}

pub unsafe fn cleanup() {}

pub fn unsupported<T>() -> crate::io::Result<T> {
    Err(unsupported_err())
}

pub fn unsupported_err() -> crate::io::Error {
    crate::io::Error::new(crate::io::ErrorKind::Other, "operation not supported on wasm yet")
}

pub fn decode_error_kind(_code: i32) -> crate::io::ErrorKind {
    crate::io::ErrorKind::Other
}

#[inline]
pub(crate) fn is_interrupted(errno: i32) -> bool {
    errno == libc::EINTR
}

// This enum is used as the storage for a bunch of types which can't actually
// exist.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub enum Void {}

pub unsafe fn strlen(mut s: *const c_char) -> usize {
    let mut n = 0;
    while *s != 0 {
        n += 1;
        s = s.offset(1);
    }
    return n;
}

pub fn abort_internal() -> ! {
    unsafe { libc::abort(); }
}

// We don't have randomness yet, but I totally used a random number generator to
// generate these numbers.
//
// More seriously though this is just for DOS protection in hash maps. It's ok
// if we don't do that on switch just yet.
pub fn hashmap_random_keys() -> (u64, u64) {
    (1, 2)
}

#[doc(hidden)]
pub trait IsMinusOne {
    fn is_minus_one(&self) -> bool;
}

macro_rules! impl_is_minus_one {
    ($($t:ident)*) => ($(impl IsMinusOne for $t {
        fn is_minus_one(&self) -> bool {
            *self == -1
        }
    })*)
}

impl_is_minus_one! { i8 i16 i32 i64 isize }

pub fn cvt<T: IsMinusOne>(t: T) -> crate::io::Result<T> {
    if t.is_minus_one() { Err(crate::io::Error::last_os_error()) } else { Ok(t) }
}

pub fn cvt_r<T, F>(mut f: F) -> crate::io::Result<T>
where
    T: IsMinusOne,
    F: FnMut() -> T,
{
    loop {
        match cvt(f()) {
            Err(ref e) if e.kind() == ErrorKind::Interrupted => {}
            other => return other,
        }
    }
}

#[allow(dead_code)] // Not used on all platforms.
/// Zero means `Ok()`, all other values are treated as raw OS errors. Does not look at `errno`.
pub fn cvt_nz(error: libc::c_int) -> crate::io::Result<()> {
    if error == 0 { Ok(()) } else { Err(crate::io::Error::from_raw_os_error(error)) }
}