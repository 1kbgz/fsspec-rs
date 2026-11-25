//! Thread bounds for native backends and browser-local JavaScript handles.
//!
//! Native targets require `Send`/`Sync`. Browser WASM backends stay on their
//! originating thread and can own JavaScript handles and await local promises.

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
mod bounds {
    /// `Send` on native targets; no thread-transfer requirement in browser WASM.
    pub trait MaybeSend: Send {}
    impl<T: Send + ?Sized> MaybeSend for T {}

    /// `Sync` on native targets; no cross-thread sharing requirement in browser WASM.
    pub trait MaybeSync: Sync {}
    impl<T: Sync + ?Sized> MaybeSync for T {}
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
mod bounds {
    /// Browser-local bound; does not make a JavaScript handle safe to send.
    pub trait MaybeSend {}
    impl<T: ?Sized> MaybeSend for T {}

    /// Browser-local bound; does not make a JavaScript handle safe to share.
    pub trait MaybeSync {}
    impl<T: ?Sized> MaybeSync for T {}
}

pub use bounds::{MaybeSend, MaybeSync};
