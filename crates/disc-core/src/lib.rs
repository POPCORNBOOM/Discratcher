//! Pure core of Discratcher: no Tauri types, so `cargo test -p disc-core` stays fast.

pub mod artwork;
pub mod calib;
pub mod carver;
pub mod composition;
pub mod drive;
pub mod ecma267;
pub mod iso9660;
pub mod preset;
pub mod preview;
pub mod rng;
pub mod spiral;

/// The sibling discprobe checkout holds the real burned-image and real-run oracles.
/// Those tests skip themselves when it is not on this machine; the home directory is
/// read at run time so the suite still compiles where HOME is unset, as on Windows.
#[cfg(test)]
pub(crate) fn probe_root() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(std::path::Path::new(&home).join("Github").join("discprobe-macos"))
}
