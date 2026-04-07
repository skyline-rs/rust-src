use crate::cmp;
use crate::ffi::CStr;
use crate::io;
use crate::mem;
use crate::num::NonZero;
use crate::num::NonZeroUsize;
use crate::ptr;
use crate::sys::os;
use crate::sys::unsupported;
use crate::thread::ThreadInit;
use crate::time::Duration;

use nnsdk::{os::SleepThread, TimeSpan};

// TODO: Move to nnsdk?
unsafe extern "C" {
    #[link_name = "_ZN2nn2os17SetThreadCoreMaskEPNS0_10ThreadTypeEim"]
    fn nn_os_SetThreadCoreMask(thread: *mut nnsdk::os::ThreadType, ideal_core: i32, mask: u64);
}

#[cfg(not(target_os = "l4re"))]
pub const DEFAULT_MIN_STACK_SIZE: usize = 2 * 1024 * 1024;
#[cfg(target_os = "l4re")]
pub const DEFAULT_MIN_STACK_SIZE: usize = 1024 * 1024;

pub struct Thread {
    id: libc::pthread_t,
}

// Some platforms may have pthread_t as a pointer in which case we still want
// a thread to be Send/Sync
unsafe impl Send for Thread {}
unsafe impl Sync for Thread {}

impl Thread {
    // unsafe: see thread::Builder::spawn_unchecked for safety requirements
    pub unsafe fn new(stack: usize, init: Box<ThreadInit>) -> io::Result<Thread> {
        let p = Box::into_raw(init);
        let mut native: libc::pthread_t = mem::zeroed();
        let mut attr: libc::pthread_attr_t = mem::zeroed();
        assert_eq!(libc::pthread_attr_init(&mut attr), 0);

        let stack_size = cmp::max(stack, min_stack_size(&attr));

        match libc::pthread_attr_setstacksize(&mut attr, stack_size) {
            0 => {}
            n => {
                assert_eq!(n, libc::EINVAL);
                // EINVAL means |stack_size| is either too small or not a
                // multiple of the system page size.  Because it's definitely
                // >= PTHREAD_STACK_MIN, it must be an alignment issue.
                // Round up to the nearest page and try again.
                let page_size = os::page_size();
                let stack_size =
                    (stack_size + page_size - 1) & (-(page_size as isize - 1) as usize - 1);
                assert_eq!(libc::pthread_attr_setstacksize(&mut attr, stack_size), 0);
            }
        };

        let ret = libc::pthread_create(&mut native, &attr, thread_start, p as *mut _);
        // Note: if the thread creation fails and this assert fails, then p will
        // be leaked. However, an alternative design could cause double-free
        // which is clearly worse.
        assert_eq!(libc::pthread_attr_destroy(&mut attr), 0);

        return if ret != 0 {
            // The thread failed to start and as a result p was not consumed. Therefore, it is
            // safe to reconstruct the box so that it gets deallocated.
            drop(Box::from_raw(p));
            Err(io::Error::from_raw_os_error(ret))
        } else {
            Ok(Thread { id: native })
        };

        extern "C" fn thread_start(main: *mut libc::c_void) -> *mut libc::c_void {
            unsafe {
                let init = Box::from_raw(main as *mut ThreadInit);
                let rust_start = init.init();

                rust_start();
            }
            ptr::null_mut()
        }
    }

    pub fn join(self) {
        unsafe {
            let ret = libc::pthread_join(self.id, ptr::null_mut());
            mem::forget(self);
            assert!(ret == 0, "failed to join thread: {}", io::Error::from_raw_os_error(ret));
        }
    }
}

pub fn current_os_id() -> Option<u64> {
    Some(unsafe { libc::pthread_self() } as u64)
}

pub fn yield_now() {
    let ret = unsafe { libc::sched_yield() };
    debug_assert_eq!(ret, 0);
}

pub fn set_name(name: &CStr) {
        let cname = crate::ffi::CString::new(&b"%s"[..]).unwrap();
        
        unsafe {
            libc::pthread_setname_np(
                libc::pthread_self(),
                cname.as_ptr() as *const u8,
                name.as_ptr() as *mut libc::c_void,
            );
        }
    }

pub fn sleep(dur: Duration) {
    let time_span = TimeSpan::nano(dur.as_nanos() as u64);

    unsafe {
        SleepThread(time_span);
    }
}

pub fn available_parallelism() -> io::Result<NonZeroUsize> {
    // Not sure this is better than hardcoding the cores, but with the Switch 2 being a thing nwo maybe it's better to query the OS.
    let mask = unsafe { nnsdk::os::GetThreadAvailableCoreMask() };
    NonZero::new(mask.count_ones() as usize)
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "no usable cores"))
}

pub fn pin_to_core(core_id: i32) {
    debug_assert!(core_id >= 0, "core_id must be non-negative");
    unsafe {
        nn_os_SetThreadCoreMask(nnsdk::os::GetCurrentThread(), core_id, 1u64 << core_id);
    }
}

impl Drop for Thread {
    fn drop(&mut self) {
        let ret = unsafe { libc::pthread_detach(self.id) };
        debug_assert_eq!(ret, 0);
    }
}

#[cfg_attr(test, allow(dead_code))]
pub mod guard {
    use crate::ops::Range;
    pub type Guard = Range<usize>;
    pub unsafe fn current() -> Option<Guard> {
        None
    }
    pub unsafe fn init() -> Option<Guard> {
        None
    }
}

fn min_stack_size(_: *const libc::pthread_attr_t) -> usize {
    os::page_size()
}
