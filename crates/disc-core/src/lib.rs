//! Pure core of Discratcher: no Tauri types, so `cargo test -p disc-core` stays fast.

pub mod artwork;
pub mod calib;
pub mod carver;
pub mod drive;
pub mod ecma267;
pub mod iso9660;
pub mod preset;
pub mod preview;
pub mod rng;
pub mod spiral;
