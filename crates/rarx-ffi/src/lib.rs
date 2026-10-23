//! Stable C ABI for the RarX engine (AGENT.md §3), exposed through `rarx.h`.

#![allow(unsafe_code)] // `#[no_mangle]` exports are unsafe attributes (AGENT.md §18.6).
#![deny(missing_docs)]

/// Version of the C ABI. Bumped on every incompatible change to `rarx.h`.
pub const ABI_VERSION: u32 = 1;

/// Returns the C ABI version so callers can reject an incompatible library.
// SAFETY: the symbol name is unique to this crate and the function takes no pointers.
#[unsafe(no_mangle)]
pub extern "C" fn rarx_abi_version() -> u32 {
    ABI_VERSION
}
