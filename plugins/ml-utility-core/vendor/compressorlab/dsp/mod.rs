//! Pinned CompressorLab models and electrical components. See ../README.md.
pub mod components;
pub mod fet;
pub mod opto;
pub mod vu;

/// The donor DSP zeros inaudible values to avoid denormals.
#[inline]
pub fn flush(x: f32) -> f32 {
    if x.abs() < 1e-12 { 0.0 } else { x }
}
