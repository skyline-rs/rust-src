use crate::convert::TryFrom;
use crate::fmt;
use crate::cmp;
use crate::ffi::CStr;
use crate::io::{self, IoSlice, IoSliceMut, BorrowedBuf, BorrowedCursor};
use crate::mem;
use crate::net::{Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr};
use crate::os::switch::io::{AsRawFd, FromRawFd, IntoRawFd, RawFd};
use crate::str;
use crate::sys::fd::FileDesc;
use crate::sys::pal::IsMinusOne;
use super::{getsockopt, setsockopt, socket_addr_from_c, socket_addr_to_c};
use crate::sys::{AsInner, FromInner, IntoInner};


use crate::time::{Duration, Instant};

use crate::sys::unsupported;


use nnsdk as nn;

use libc::{c_int, c_void, size_t, sockaddr, socklen_t, EAI_SYSTEM, MSG_PEEK};

pub use crate::sys::{cvt, cvt_r};

#[allow(unused_extern_crates)]
pub extern crate libc as netc;

#[allow(non_camel_case_types)]
pub type wrlen_t = size_t;

pub struct Socket(FileDesc);

pub fn init() {
    unsafe {
        nn::nifm::Initialize();
        nn::nifm::SubmitNetworkRequest();

        while (nn::nifm::IsNetworkRequestOnHold()) {
            nn::os::SleepThread(nnsdk::TimeSpan::nano(1000000000));
        }
    }
}

pub fn cvt_gai(err: c_int) -> io::Result<()> {
    if err == 0 {
        return Ok(());
    }

    // We may need to trigger a glibc workaround. See on_resolver_failure() for details.
    on_resolver_failure();

    if err == EAI_SYSTEM {
        return Err(io::Error::last_os_error());
    } else if err == 7 { // EAI_NODATA
        // This doesn't make much sense, considering they should've used EAI_SYSTEM and provided a ENETDOWN error.
        // No point in trying to connect the socket if the network is down
        return if unsafe { !nn::nifm::IsNetworkAvailable() } {
            // return Err(io::Error::new(io::ErrorKind::NetworkDown, "failed to connect to the network, consider removing airplane mode or configuring your network settings"));
            Err(io::Error::from(io::ErrorKind::NetworkDown))
        } else {
            Err(io::Error::new(io::ErrorKind::Other, "an unknown networking error has occurred"))
        }

    }

    let detail = unsafe {
        str::from_utf8(CStr::from_ptr(libc::gai_strerror(err)).to_bytes()).unwrap().to_owned()
    };
    Err(io::Error::new(
        io::ErrorKind::Other,
        &format!("failed to lookup address information: {}", detail)[..],
    ))
}

impl Socket {
    pub fn new(fam: c_int, ty: c_int) -> io::Result<Socket> {
        unsafe {
            let fd = cvt(libc::socket(fam, ty, 0))?;
            let fd = FileDesc::new(fd);
            fd.set_cloexec()?;
            let socket = Socket(fd);

            Ok(socket)
        }
    }

    pub fn new_pair(fam: c_int, ty: c_int) -> io::Result<(Socket, Socket)> {
        let mut fds = [0, 0];

        cvt(libc::socketpair(fam, ty, 0, fds.as_mut_ptr()))?;
        let a = FileDesc::new(fds[0]);
        let b = FileDesc::new(fds[1]);
        a.set_cloexec()?;
        b.set_cloexec()?;
        Ok((Socket(a), Socket(b)))
    }

    pub fn connect(&self, addr: &SocketAddr) -> io::Result<()> {
        let (addr, len) = socket_addr_to_c(addr);
        loop {
            let result = unsafe { libc::connect(self.as_raw_fd(), addr.as_ptr(), len) };
            if result.is_minus_one() {
                let err = crate::sys::io::errno();
                match err {
                    libc::EINTR => continue,
                    libc::EISCONN => return Ok(()),
                    _ => return Err(io::Error::from_raw_os_error(err)),
                }
            }
            return Ok(());
        }
    }

    pub fn connect_timeout(&self, addr: &SocketAddr, timeout: Duration) -> io::Result<()> {
        self.set_nonblocking(true)?;
        let r = unsafe {
            let (addr, len) = socket_addr_to_c(addr);
            cvt(libc::connect(self.0.raw(), addr.as_ptr(), len))
        };
        self.set_nonblocking(false)?;

        match r {
            Ok(_) => return Ok(()),
            // there's no ErrorKind for EINPROGRESS :(
            Err(ref e) if e.raw_os_error() == Some(libc::EINPROGRESS) => {}
            Err(e) => return Err(e),
        }

        let mut pollfd = libc::pollfd { fd: self.0.raw(), events: libc::POLLOUT, revents: 0 };

        if timeout.as_secs() == 0 && timeout.subsec_nanos() == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cannot set a 0 duration timeout",
            ));
        }

        let start = Instant::now();

        loop {
            let elapsed = start.elapsed();
            if elapsed >= timeout {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "connection timed out"));
            }

            let timeout = timeout - elapsed;
            let mut timeout = timeout
                .as_secs()
                .saturating_mul(1_000)
                .saturating_add(timeout.subsec_nanos() as u64 / 1_000_000);
            if timeout == 0 {
                timeout = 1;
            }

            let timeout = cmp::min(timeout, c_int::MAX as u64) as c_int;

            match unsafe { libc::poll(&mut pollfd, 1, timeout) } {
                -1 => {
                    let err = io::Error::last_os_error();
                    if err.kind() != io::ErrorKind::Interrupted {
                        return Err(err);
                    }
                }
                0 => {}
                _ => {
                    // linux returns POLLOUT|POLLERR|POLLHUP for refused connections (!), so look
                    // for POLLHUP rather than read readiness
                    // if pollfd.revents & libc::POLLHUP != 0 {
                    //     let e = self.take_error()?.unwrap_or_else(|| {
                    //         io::Error::new(io::ErrorKind::Other, "no error set after POLLHUP")
                    //     });
                    //     return Err(e);
                    // }

                    return Ok(());
                }
            }
        }
    }

    pub fn accept(&self, storage: *mut sockaddr, len: *mut socklen_t) -> io::Result<Socket> {
        let fd = cvt_r(|| unsafe { libc::accept(self.0.raw(), storage, len) })?;
        let fd = FileDesc::new(fd);
        fd.set_cloexec()?;
        Ok(Socket(fd))
    }

    pub fn duplicate(&self) -> io::Result<Socket> {
        self.0.duplicate().map(Socket)
    }

    fn recv_with_flags(&self, mut buf: BorrowedCursor<'_>, flags: c_int) -> io::Result<()> {
        let ret = cvt(unsafe {
            libc::recv(self.0.raw(), buf.as_mut().as_mut_ptr() as *mut c_void, buf.capacity(), flags)
        })?;
        unsafe {
            buf.advance(ret as usize);
        }
        Ok(())
    }

    pub fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        let mut buf = BorrowedBuf::from(buf);
        self.recv_with_flags(buf.unfilled(), 0)?;
        Ok(buf.len())
    }

    pub fn peek(&self, buf: &mut [u8]) -> io::Result<usize> {
        let mut buf = BorrowedBuf::from(buf);
        self.recv_with_flags(buf.unfilled(), MSG_PEEK)?;
        Ok(buf.len())
    }

    pub fn read_buf(&self, buf: BorrowedCursor<'_>) -> io::Result<()> {
        self.recv_with_flags(buf, 0)
    }

    pub fn read_vectored(&self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        self.0.read_vectored(bufs)
    }

    #[inline]
    pub fn is_read_vectored(&self) -> bool {
        self.0.is_read_vectored()
    }

    fn recv_from_with_flags(
        &self,
        buf: &mut [u8],
        flags: c_int,
    ) -> io::Result<(usize, SocketAddr)> {
        let mut storage: libc::sockaddr_storage = unsafe { mem::zeroed() };
        let mut addrlen = mem::size_of_val(&storage) as libc::socklen_t;

        let n = cvt(unsafe {
            libc::recvfrom(
                self.0.raw(),
                buf.as_mut_ptr() as *mut c_void,
                buf.len(),
                flags,
                &mut storage as *mut _ as *mut _,
                &mut addrlen,
            )
        })?;
        Ok((
            n as usize,
            unsafe { socket_addr_from_c(&storage, mem::size_of_val(&storage) as libc::socklen_t as usize)? },
        ))
    }

    pub fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        self.recv_from_with_flags(buf, 0)
    }

    pub fn peek_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        self.recv_from_with_flags(buf, MSG_PEEK)
    }

    pub fn write(&self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    pub fn write_vectored(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        self.0.write_vectored(bufs)
    }

    #[inline]
    pub fn is_write_vectored(&self) -> bool {
        self.0.is_write_vectored()
    }

    pub fn set_timeout(&self, dur: Option<Duration>, kind: libc::c_int) -> io::Result<()> {
        let timeout = match dur {
            Some(dur) => {
                if dur.as_secs() == 0 && dur.subsec_nanos() == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "cannot set a 0 duration timeout",
                    ));
                }

                let secs = if dur.as_secs() > libc::time_t::MAX as u64 {
                    libc::time_t::MAX
                } else {
                    dur.as_secs() as libc::time_t
                };
                let mut timeout = libc::timeval {
                    tv_sec: secs,
                    tv_usec: dur.subsec_micros() as libc::suseconds_t,
                };
                if timeout.tv_sec == 0 && timeout.tv_usec == 0 {
                    timeout.tv_usec = 1;
                }
                timeout
            }
            None => libc::timeval { tv_sec: 0, tv_usec: 0 },
        };
        unsafe { setsockopt(self, libc::SOL_SOCKET, kind, timeout) }
    }

    pub fn timeout(&self, kind: libc::c_int) -> io::Result<Option<Duration>> {
        let raw: libc::timeval = unsafe { getsockopt(self, libc::SOL_SOCKET, kind)? };
        if raw.tv_sec == 0 && raw.tv_usec == 0 {
            Ok(None)
        } else {
            let sec = raw.tv_sec as u64;
            let nsec = (raw.tv_usec as u32) * 1000;
            Ok(Some(Duration::new(sec, nsec)))
        }
    }

    pub fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        let how = match how {
            Shutdown::Write => libc::SHUT_WR,
            Shutdown::Read => libc::SHUT_RD,
            Shutdown::Both => libc::SHUT_RDWR,
        };
        cvt(unsafe { libc::shutdown(self.0.raw(), how) })?;
        Ok(())
    }

    // pub fn set_linger(&self, linger: Option<Duration>) -> io::Result<()> {
    //     let linger = libc::linger {
    //         l_onoff: linger.is_some() as libc::c_int,
    //         l_linger: linger.unwrap_or_default().as_secs() as libc::c_int,
    //     };

    //     setsockopt(self, libc::SOL_SOCKET, libc::SO_LINGER, linger)
    // }

    // pub fn linger(&self) -> io::Result<Option<Duration>> {
    //     let val: libc::linger = getsockopt(self, libc::SOL_SOCKET, SO_LINGER)?;

    //     Ok((val.l_onoff != 0).then(|| Duration::from_secs(val.l_linger as u64)))
    // }

    pub fn set_linger(&self, linger: Option<Duration>) -> io::Result<()> {
        unsupported()
    }

    pub fn linger(&self) -> io::Result<Option<Duration>> {
        unsupported()
    }

    pub fn set_nodelay(&self, nodelay: bool) -> io::Result<()> {
        unsafe { setsockopt(self, libc::IPPROTO_TCP, libc::TCP_NODELAY, nodelay as c_int) }
    }

    pub fn nodelay(&self) -> io::Result<bool> {
        let raw: c_int = unsafe { getsockopt(self, libc::IPPROTO_TCP, libc::TCP_NODELAY)? };
        Ok(raw != 0)
    }

    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        unsafe {
            let previous = cvt(libc::fcntl(self.as_raw_fd(), libc::F_GETFL))?;
            let new = if nonblocking {
                previous | libc::O_NONBLOCK
            } else {
                previous & !libc::O_NONBLOCK
            };
            if new != previous {
                cvt(libc::fcntl(self.as_raw_fd(), libc::F_SETFL, new))?;
            }

            Ok(())
        }
    }

    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        let raw: c_int = unsafe { getsockopt(self, libc::SOL_SOCKET, libc::SO_ERROR)? };
        if raw == 0 { Ok(None) } else { Ok(Some(io::Error::from_raw_os_error(raw as i32))) }
    }

    // This is used by sys_common code to abstract over Windows and Unix.
    pub fn as_raw(&self) -> c_int {
        *self.as_inner()
    }
}

impl AsInner<c_int> for Socket {
    fn as_inner(&self) -> &c_int {
        self.0.as_inner()
    }
}

impl FromInner<c_int> for Socket {
    fn from_inner(fd: c_int) -> Socket {
        Socket(FileDesc::new(fd))
    }
}

impl IntoInner<c_int> for Socket {
    fn into_inner(self) -> c_int {
        self.0.into_raw()
    }
}

// impl AsFd for Socket {
//     fn as_fd(&self) -> BorrowedFd<'_> {
//         self.0.as_fd()
//     }
// }

impl AsRawFd for Socket {
    fn as_raw_fd(&self) -> RawFd {
        *self.0.as_inner()
    }
}

impl IntoRawFd for Socket {
    fn into_raw_fd(self) -> RawFd {
        self.as_raw_fd()
    }
}

impl FromRawFd for Socket {
    unsafe fn from_raw_fd(raw_fd: RawFd) -> Self {
        Self(FileDesc::new(raw_fd))
    }
}

// In versions of glibc prior to 2.26, there's a bug where the DNS resolver
// will cache the contents of /etc/resolv.conf, so changes to that file on disk
// can be ignored by a long-running program. That can break DNS lookups on e.g.
// laptops where the network comes and goes. See
// https://sourceware.org/bugzilla/show_bug.cgi?id=984. Note however that some
// distros including Debian have patched glibc to fix this for a long time.
//
// A workaround for this bug is to call the res_init libc function, to clear
// the cached configs. Unfortunately, while we believe glibc's implementation
// of res_init is thread-safe, we know that other implementations are not
// (https://github.com/rust-lang/rust/issues/43592). Code here in libstd could
// try to synchronize its res_init calls with a Mutex, but that wouldn't
// protect programs that call into libc in other ways. So instead of calling
// res_init unconditionally, we call it only when we detect we're linking
// against glibc version < 2.26. (That is, when we both know its needed and
// believe it's thread-safe).
#[cfg(target_env = "gnu")]
fn on_resolver_failure() {
    use crate::sys;

    // If the version fails to parse, we treat it the same as "not glibc".
    if let Some(version) = sys::os::glibc_version() {
        if version < (2, 26) {
            unsafe { libc::res_init() };
        }
    }
}

#[cfg(not(target_env = "gnu"))]
fn on_resolver_failure() {}