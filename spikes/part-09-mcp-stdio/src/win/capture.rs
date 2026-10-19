//! One-shot monitor capture through Windows.Graphics.Capture.
//!
//! Adapted from spike 17 (`create_gpu`, `capture_monitor`), which proved real frames on 4 monitors.
//! Differences: returns the platform-neutral `Frame`, and a drop guard closes the capture session
//! and frame pool on every path, so an error never leaves a capture running.

use std::thread::sleep;
use std::time::{Duration, Instant};

use windows::core::Interface;
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;

use super::monitors::Monitor;
use crate::backend::{Frame, ToolError};

const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(5);
const FRAME_POLL_INTERVAL: Duration = Duration::from_millis(15);
const BYTES_PER_PIXEL: usize = 4;
const OPAQUE_ALPHA: u8 = 0xff;
/// HRESULT used for "no frame arrived" (WAIT_TIMEOUT-like), since no OS call failed.
const E_NO_FRAME: i32 = 0x8000_0007u32 as i32;

fn os_error(what: &str, e: windows::core::Error) -> ToolError {
    ToolError::new(format!("{what} failed: {e}"))
}

pub struct Gpu {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    winrt_device: IDirect3DDevice,
}

impl Gpu {
    pub fn new() -> Result<Self, ToolError> {
        let create = || -> windows::core::Result<Gpu> {
            let mut device: Option<ID3D11Device> = None;
            let mut context: Option<ID3D11DeviceContext> = None;
            // SAFETY: out-pointers are valid for the call.
            unsafe {
                D3D11CreateDevice(
                    None,
                    D3D_DRIVER_TYPE_HARDWARE,
                    HMODULE::default(),
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    None,
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    None,
                    Some(&mut context),
                )?;
            }
            let device = device.ok_or_else(windows::core::Error::empty)?;
            let context = context.ok_or_else(windows::core::Error::empty)?;
            let dxgi: IDXGIDevice = device.cast()?;
            // SAFETY: dxgi is a live IDXGIDevice.
            let winrt_device: IDirect3DDevice =
                unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi)? }.cast()?;
            Ok(Gpu {
                device,
                context,
                winrt_device,
            })
        };
        create().map_err(|e| os_error("creating the D3D11 device", e))
    }
}

/// Closes the session and pool when dropped, whichever way `capture` returns.
struct SessionGuard {
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}

pub fn capture(gpu: &Gpu, monitor: &Monitor) -> Result<Frame, ToolError> {
    capture_inner(gpu, monitor).map_err(|e| os_error("capture", e))
}

fn capture_inner(gpu: &Gpu, monitor: &Monitor) -> windows::core::Result<Frame> {
    let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
    // SAFETY: monitor.handle came from EnumDisplayMonitors in this process.
    let item: GraphicsCaptureItem = unsafe { interop.CreateForMonitor(monitor.handle)? };
    let item_size = item.Size()?;

    let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
        &gpu.winrt_device,
        DirectXPixelFormat::B8G8R8A8UIntNormalized,
        1,
        item_size,
    )?;
    let session = pool.CreateCaptureSession(&item)?;
    let guard = SessionGuard { pool, session };
    // The cursor position has its own tool; keep it out of the pixels.
    guard.session.SetIsCursorCaptureEnabled(false)?;
    let started = Instant::now();
    guard.session.StartCapture()?;

    let frame = loop {
        match guard.pool.TryGetNextFrame() {
            Ok(frame) => break frame,
            Err(_) if started.elapsed() < FIRST_FRAME_TIMEOUT => sleep(FRAME_POLL_INTERVAL),
            Err(_) => {
                return Err(windows::core::Error::new(
                    windows::core::HRESULT(E_NO_FRAME),
                    format!("no frame within {} s", FIRST_FRAME_TIMEOUT.as_secs()),
                ))
            }
        }
    };

    let access: IDirect3DDxgiInterfaceAccess = frame.Surface()?.cast()?;
    // SAFETY: the surface wraps an ID3D11Texture2D.
    let texture: ID3D11Texture2D = unsafe { access.GetInterface()? };
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    // SAFETY: valid texture and out-pointer.
    unsafe { texture.GetDesc(&mut desc) };
    desc.Usage = D3D11_USAGE_STAGING;
    desc.BindFlags = 0;
    desc.MiscFlags = 0;
    desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;

    let mut staging: Option<ID3D11Texture2D> = None;
    // SAFETY: desc is a fully initialised staging description.
    unsafe {
        gpu.device
            .CreateTexture2D(&desc, None, Some(&mut staging))?
    };
    let staging = staging.ok_or_else(windows::core::Error::empty)?;
    // SAFETY: both textures belong to gpu.device.
    unsafe { gpu.context.CopyResource(&staging, &texture) };

    let (width, height) = (desc.Width as usize, desc.Height as usize);
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    // SAFETY: staging is CPU-readable; the map is released before returning.
    unsafe {
        gpu.context
            .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?
    };
    let row_bytes = width * BYTES_PER_PIXEL;
    let mut rgba = vec![0u8; row_bytes * height];
    for row in 0..height {
        // SAFETY: each mapped row holds at least width * 4 bytes.
        let src = unsafe {
            std::slice::from_raw_parts(
                (mapped.pData as *const u8).add(row * mapped.RowPitch as usize),
                row_bytes,
            )
        };
        let dst = &mut rgba[row * row_bytes..(row + 1) * row_bytes];
        let (dst_px, _) = dst.as_chunks_mut::<BYTES_PER_PIXEL>();
        let (src_px, _) = src.as_chunks::<BYTES_PER_PIXEL>();
        for (d, s) in dst_px.iter_mut().zip(src_px) {
            // BGRA -> RGBA, forced opaque: the desktop has no meaningful alpha.
            *d = [s[2], s[1], s[0], OPAQUE_ALPHA];
        }
    }
    // SAFETY: matches the Map above.
    unsafe { gpu.context.Unmap(&staging, 0) };

    Ok(Frame {
        width: desc.Width,
        height: desc.Height,
        rgba,
    })
}
