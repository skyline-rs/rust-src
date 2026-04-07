use crate::io;
use crate::sys::thread::pin_to_core;
use crate::thread::{Builder, JoinHandle, Scope, ScopedJoinHandle};

/// # Example
///
/// ```no_run
/// use std::os::switch::thread::BuilderExt;
/// use std::thread;
/// 
/// thread::scope(|s| {
///     thread::Builder::new()
///         .ideal_core(1)
///         .spawn_scoped(s, || {
///             // ...
///         })
///         .unwrap();
/// });
/// ```
#[stable(feature = "switch_thread_ext", since = "1.0.0")]
pub trait BuilderExt: Sized {
    /// Pin the spawned thread to a single CPU core.
    ///
    /// `core_id` must be a core that this process is allowed to run on
    #[stable(feature = "switch_thread_ext", since = "1.0.0")]
    fn ideal_core(self, core_id: i32) -> CoreBoundBuilder;
}

#[stable(feature = "switch_thread_ext", since = "1.0.0")]
impl BuilderExt for Builder {
    fn ideal_core(self, core_id: i32) -> CoreBoundBuilder {
        CoreBoundBuilder { inner: self, core_id }
    }
}

/// A [`Builder`] that pins every thread it spawns to a specific CPU core.
#[stable(feature = "switch_thread_ext", since = "1.0.0")]
#[must_use = "must call `.spawn(...)` or `.spawn_scoped(...)` to actually start a thread"]
pub struct CoreBoundBuilder {
    inner: Builder,
    core_id: i32,
}


impl CoreBoundBuilder {
    /// Spawn a thread pinned to the requested core.
    ///
    /// Equivalent to [`Builder::spawn`], except the spawned thread migrates
    /// to the requested core before `f` is called.
    #[stable(feature = "switch_thread_ext", since = "1.0.0")]
    pub fn spawn<F, T>(self, f: F) -> io::Result<JoinHandle<T>>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        let core_id = self.core_id;
        self.inner.spawn(move || {
            pin_to_core(core_id);
            f()
        })
    }

    /// Spawn a scoped thread pinned to the requested core.
    ///
    /// Equivalent to [`Builder::spawn_scoped`], except the spawned thread
    /// migrates to the requested core before `f` is called.
    #[stable(feature = "switch_thread_ext", since = "1.0.0")]
    pub fn spawn_scoped<'scope, 'env, F, T>(
        self,
        scope: &'scope Scope<'scope, 'env>,
        f: F,
    ) -> io::Result<ScopedJoinHandle<'scope, T>>
    where
        F: FnOnce() -> T + Send + 'scope,
        T: Send + 'scope,
    {
        let core_id = self.core_id;
        self.inner.spawn_scoped(scope, move || {
            pin_to_core(core_id);
            f()
        })
    }
}
