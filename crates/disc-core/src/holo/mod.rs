//! Experimental: colour and view-dependent images from the spectrum of each sync
//! frame's marks.

pub mod efm;
#[rustfmt::skip]
pub mod efm_tables;
pub mod frame;
pub mod optics;
pub mod codebook;
pub mod swatch;
pub mod design;
pub mod scene;
pub mod testscene;
pub mod model3d;
pub mod lightfield;
pub mod ink;
pub mod turntable;
pub mod flat;
pub mod project;
#[cfg(test)]
mod color_study;
