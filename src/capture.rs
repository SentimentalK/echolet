//! Platform-neutral audio capture contract.
//!
//! Minimal, dependency-free definitions modeling voice capture: a decoded PCM
//! chunk, a live capture source (dropping it stops and releases the capture),
//! and the factory that binds a producer to ONE session's queue.
//!
//! This module MUST NOT depend on any specific platform: no cpal, no Slint,
//! no OS services, no model registry, no native FFI. Keeping the contract
//! neutral lets the session engine (`crate::session`) and any future mobile
//! audio adapter share the exact same types.

pub struct AudioChunk {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

/// A live capture source. Dropping this instance stops and releases the
/// underlying audio capture stream.
pub trait AudioSource: 'static {
    // No methods: implementation-defined capture ownership.
}

/// Producer factory; invoked on each capture start with the fresh, unshared
/// queue sender of the session being armed.
pub type AudioStarter =
    Box<dyn Fn(crossbeam_channel::Sender<AudioChunk>) -> Result<Box<dyn AudioSource>, String>>;
