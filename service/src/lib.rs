//! CamFunge engine: camera in, music-inspired GPU filters, virtual camera out. Used by the
//! `camfunge-service` binary; split into a library so every piece is testable on its own.

pub mod audio;
pub mod camera;
pub mod engine;
pub mod filters;
pub mod protocol;
pub mod reactivity;
pub mod renderer;
pub mod server;
pub mod service;
pub mod shaders;
pub mod vcam;
