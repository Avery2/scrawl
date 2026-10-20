#[cfg_attr(not(target_os = "macos"), path = "pill/other.rs")]
pub mod pill;

pub mod capsule;
#[cfg(target_os = "macos")]
pub mod swift_pill;
