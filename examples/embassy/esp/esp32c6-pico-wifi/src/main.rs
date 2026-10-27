#![no_std]
#![no_main]

#[cfg(all(feature = "cli", feature = "scoped-session-test"))]
compile_error!("scoped-session-test requires --no-default-features (without cli)");
// One target application, with its frontend selected at compile time.
#[cfg(feature = "cli")]
include!("console.rs");
#[cfg(not(feature = "cli"))]
include!("headless.rs");
