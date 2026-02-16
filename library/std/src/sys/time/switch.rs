use crate::time::Duration;
use nnsdk::time::PosixTime;

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub struct Instant(Duration);

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
#[rustc_layout_scalar_valid_range_start(0)]
#[rustc_layout_scalar_valid_range_end(999_999_999)]
struct Nanoseconds(u32);

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub struct SystemTime(Duration);

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timespec {
    tv_sec: i64,
    tv_nsec: Nanoseconds,
}

const NSEC_PER_SEC: u64 = 1_000_000_000;

pub const UNIX_EPOCH: SystemTime = SystemTime(Duration::from_secs(0));

#[allow(dead_code)] // Used for pthread condvar timeouts
pub const TIMESPEC_MAX: libc::timespec =
    libc::timespec { tv_sec: <libc::time_t>::MAX, tv_nsec: 1_000_000_000 - 1 };

impl Instant {
    pub fn now() -> Instant {
        unsafe {
            let tick = nnsdk::os::GetSystemTick();

            let nanos = (tick * 625) / 12; // constant rate

            Instant(Duration::from_nanos(nanos))
        }
    }

    pub const fn zero() -> Instant {
        Instant(Duration::from_secs(0))
    }

    pub fn actually_monotonic() -> bool {
        false
    }

    pub fn checked_sub_instant(&self, other: &Instant) -> Option<Duration> {
        self.0.checked_sub(other.0)
    }

    pub fn checked_add_duration(&self, other: &Duration) -> Option<Instant> {
        Some(Instant(self.0.checked_add(*other)?))
    }

    pub fn checked_sub_duration(&self, other: &Duration) -> Option<Instant> {
        Some(Instant(self.0.checked_sub(*other)?))
    }
}

impl SystemTime {
    pub const MAX: SystemTime = SystemTime(Duration::MAX);

    pub const MIN: SystemTime = SystemTime(Duration::ZERO);

    pub fn now() -> SystemTime {
        unsafe {
            let mut ptime = PosixTime {
                time: 0,
            };

            nnsdk::time::StandardUserSystemClock::GetCurrentTime(&mut ptime);

            SystemTime(Duration::new(ptime.time, 0))
        }
    }

    pub fn from_posixtime(posix: PosixTime) -> SystemTime {
        SystemTime(Duration::from_secs(posix.time))
    }

    pub fn sub_time(&self, other: &SystemTime) -> Result<Duration, Duration> {
        self.0.checked_sub(other.0).ok_or_else(|| other.0 - self.0)
    }

    pub fn checked_add_duration(&self, other: &Duration) -> Option<SystemTime> {
        Some(SystemTime(self.0.checked_add(*other)?))
    }

    pub fn checked_sub_duration(&self, other: &Duration) -> Option<SystemTime> {
        Some(SystemTime(self.0.checked_sub(*other)?))
    }
}

impl Timespec {
    pub const fn zero() -> Timespec {
        Timespec::new(0, 0)
    }

    const fn new(tv_sec: i64, tv_nsec: i64) -> Timespec {
        // On Apple OS, dates before epoch are represented differently than on other
        // Unix platforms: e.g. 1/10th of a second before epoch is represented as `seconds=-1`
        // and `nanoseconds=100_000_000` on other platforms, but is `seconds=0` and
        // `nanoseconds=-900_000_000` on Apple OS.
        //
        // To compensate, we first detect this special case by checking if both
        // seconds and nanoseconds are in range, and then correct the value for seconds
        // and nanoseconds to match the common unix representation.
        //
        // Please note that Apple OS nonetheless accepts the standard unix format when
        // setting file times, which makes this compensation round-trippable and generally
        // transparent.
        #[cfg(any(
            target_os = "macos",
            target_os = "ios",
            target_os = "tvos",
            target_os = "watchos"
        ))]
        let (tv_sec, tv_nsec) =
            if (tv_sec <= 0 && tv_sec > i64::MIN) && (tv_nsec < 0 && tv_nsec > -1_000_000_000) {
                (tv_sec - 1, tv_nsec + 1_000_000_000)
            } else {
                (tv_sec, tv_nsec)
            };
        assert!(tv_nsec >= 0 && tv_nsec < NSEC_PER_SEC as i64);
        // SAFETY: The assert above checks tv_nsec is within the valid range
        Timespec { tv_sec, tv_nsec: unsafe { Nanoseconds(tv_nsec as u32) } }
    }

    pub fn now(clock: libc::clockid_t) -> Timespec {
        use crate::mem::MaybeUninit;
        use crate::sys::cvt;

        let mut t = MaybeUninit::uninit();
        cvt(unsafe { libc::clock_gettime(clock, t.as_mut_ptr()) }).unwrap();
        Timespec::from(unsafe { t.assume_init() })
    }

    pub fn checked_add_duration(&self, other: &Duration) -> Option<Timespec> {
        let mut secs = self.tv_sec.checked_add_unsigned(other.as_secs())?;

        // Nano calculations can't overflow because nanos are <1B which fit
        // in a u32.
        let mut nsec = other.subsec_nanos() + self.tv_nsec.0;
        if nsec >= NSEC_PER_SEC as u32 {
            nsec -= NSEC_PER_SEC as u32;
            secs = secs.checked_add(1)?;
        }
        Some(Timespec::new(secs, nsec.into()))
    }

    #[allow(dead_code)]
    pub fn to_timespec(&self) -> Option<libc::timespec> {
        Some(libc::timespec {
            tv_sec: self.tv_sec.try_into().ok()?,
            tv_nsec: self.tv_nsec.0.try_into().ok()?,
        })
    }
}

impl From<libc::timespec> for Timespec {
    fn from(t: libc::timespec) -> Timespec {
        Timespec::new(t.tv_sec as i64, t.tv_nsec as i64)
    }
}