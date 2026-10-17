//! Creating a GPU device without a window, for tests, the self-test and
//! command-line exports.

use wgpu::{Adapter, Device, Queue};

#[derive(Debug, thiserror::Error)]
pub enum DeviceError {
    #[error("no graphics adapter is available ({0})")]
    NoAdapter(String),
    #[error("the graphics adapter refused to start ({0})")]
    NoDevice(String),
}

/// A device on the best adapter, honouring `WGPU_BACKEND` and
/// `WGPU_ADAPTER_NAME`. With `software`, asks for a CPU adapter (WARP on
/// Windows, llvmpipe or lavapipe on Linux), as CI runners have no GPU.
pub fn headless(software: bool) -> Result<(Adapter, Device, Queue), DeviceError> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: software,
        compatible_surface: None,
        apply_limit_buckets: false,
    }))
    .or_else(|first| {
        if software {
            // Not every system has a CPU adapter; any adapter will do then
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
        } else {
            Err(first)
        }
    })
    .map_err(|e| DeviceError::NoAdapter(e.to_string()))?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("headless"),
        required_limits: wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
        ..Default::default()
    }))
    .map_err(|e| DeviceError::NoDevice(e.to_string()))?;
    Ok((adapter, device, queue))
}
