//! Windows-only implementation of spike 17.

use std::collections::HashSet;
use std::mem::size_of;
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use windows::core::{Interface, BOOL};
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::{HMODULE, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, MonitorFromPoint, HDC, HMONITOR, MONITORINFO,
    MONITORINFOEXW, MONITOR_DEFAULTTONULL,
};
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
use windows::Win32::UI::HiDpi::{
    GetDpiForMonitor, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    MDT_EFFECTIVE_DPI,
};
use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, MONITORINFOF_PRIMARY};

const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(5);
const FRAME_POLL_INTERVAL: Duration = Duration::from_millis(15);
const BASE_DPI: u32 = 96;
/// Distinct colours sampled before a frame is considered non-blank.
const MIN_DISTINCT_COLOURS: usize = 8;
/// Every 997th pixel (a prime) is sampled so the stride cannot alias with the row width.
const COLOUR_SAMPLE_STRIDE: usize = 997;
const BYTES_PER_PIXEL: usize = 4;
/// HRESULT used for "no frame arrived" (WAIT_TIMEOUT-like), since no OS call failed.
const E_NO_FRAME: i32 = 0x8000_0007u32 as i32;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
    Pass,
    Fail,
    Unverified,
}

impl Status {
    fn label(self) -> &'static str {
        match self {
            Status::Pass => "PASS",
            Status::Fail => "FAIL",
            Status::Unverified => "UNVERIFIED",
        }
    }
}

struct Check {
    id: String,
    name: &'static str,
    status: Status,
    detail: String,
}

fn check(id: impl Into<String>, name: &'static str, ok: bool, detail: impl Into<String>) -> Check {
    Check {
        id: id.into(),
        name,
        status: if ok { Status::Pass } else { Status::Fail },
        detail: detail.into(),
    }
}

#[derive(Clone)]
struct Monitor {
    index: usize,
    handle: HMONITOR,
    device: String,
    rect: RECT,
    primary: bool,
    effective_dpi: u32,
}

impl Monitor {
    fn width(&self) -> i32 {
        self.rect.right - self.rect.left
    }
    fn height(&self) -> i32 {
        self.rect.bottom - self.rect.top
    }
    fn scale_percent(&self) -> u32 {
        self.effective_dpi * 100 / BASE_DPI
    }
    fn hmonitor_hex(&self) -> String {
        format!("0x{:x}", self.handle.0 as usize)
    }
}

unsafe extern "system" fn collect_monitor(
    handle: HMONITOR,
    _hdc: HDC,
    _rect: *mut RECT,
    lparam: LPARAM,
) -> BOOL {
    // SAFETY: lparam is the &mut Vec<HMONITOR> passed by enumerate_monitors for this call only.
    let handles = unsafe { &mut *(lparam.0 as *mut Vec<HMONITOR>) };
    handles.push(handle);
    BOOL(1)
}

fn enumerate_monitors() -> windows::core::Result<Vec<Monitor>> {
    let mut handles: Vec<HMONITOR> = Vec::new();
    // SAFETY: the callback only writes through the pointer while this call runs.
    unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(collect_monitor),
            LPARAM(&mut handles as *mut Vec<HMONITOR> as isize),
        )
        .ok()?;
    }
    let mut monitors = Vec::new();
    for (index, handle) in handles.into_iter().enumerate() {
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        // SAFETY: MONITORINFOEXW starts with MONITORINFO and cbSize says it is the extended form.
        unsafe {
            GetMonitorInfoW(handle, &mut info as *mut MONITORINFOEXW as *mut MONITORINFO).ok()?
        };
        let (mut dpi_x, mut dpi_y) = (0u32, 0u32);
        // SAFETY: valid monitor handle and out-pointers.
        unsafe { GetDpiForMonitor(handle, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y)? };
        let name_len = info
            .szDevice
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(info.szDevice.len());
        monitors.push(Monitor {
            index,
            handle,
            device: String::from_utf16_lossy(&info.szDevice[..name_len]),
            rect: info.monitorInfo.rcMonitor,
            primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
            effective_dpi: dpi_x,
        });
    }
    Ok(monitors)
}

struct Frame {
    item_size: (i32, i32),
    content_size: (i32, i32),
    texture_size: (u32, u32),
    /// Tightly packed RGBA8.
    rgba: Vec<u8>,
    time_to_frame: Duration,
}

struct Gpu {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    winrt_device: IDirect3DDevice,
}

fn create_gpu() -> windows::core::Result<Gpu> {
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
}

fn capture_monitor(gpu: &Gpu, monitor: &Monitor) -> windows::core::Result<Frame> {
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
    // The cursor position is recorded separately; keep it out of the pixels.
    session.SetIsCursorCaptureEnabled(false)?;
    let started = Instant::now();
    session.StartCapture()?;

    let frame = loop {
        match pool.TryGetNextFrame() {
            Ok(frame) => break frame,
            Err(_) if started.elapsed() < FIRST_FRAME_TIMEOUT => sleep(FRAME_POLL_INTERVAL),
            Err(_) => {
                let _ = session.Close();
                let _ = pool.Close();
                return Err(windows::core::Error::new(
                    windows::core::HRESULT(E_NO_FRAME),
                    "no frame within timeout",
                ));
            }
        }
    };
    let time_to_frame = started.elapsed();
    let content_size = frame.ContentSize()?;

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
            *d = [s[2], s[1], s[0], 0xff];
        }
    }
    // SAFETY: matches the Map above.
    unsafe { gpu.context.Unmap(&staging, 0) };

    let _ = session.Close();
    let _ = pool.Close();
    Ok(Frame {
        item_size: (item_size.Width, item_size.Height),
        content_size: (content_size.Width, content_size.Height),
        texture_size: (desc.Width, desc.Height),
        rgba,
        time_to_frame,
    })
}

fn distinct_colours(rgba: &[u8]) -> usize {
    rgba.as_chunks::<BYTES_PER_PIXEL>()
        .0
        .iter()
        .step_by(COLOUR_SAMPLE_STRIDE)
        .map(|p| [p[0], p[1], p[2]])
        .collect::<HashSet<_>>()
        .len()
}

fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<(), String> {
    let file =
        std::fs::File::create(path).map_err(|e| format!("create {}: {e}", path.display()))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|e| format!("png header: {e}"))?;
    writer
        .write_image_data(rgba)
        .map_err(|e| format!("png data: {e}"))
}

fn read_png_dims(path: &Path) -> Result<(u32, u32), String> {
    let file = std::fs::File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let reader = png::Decoder::new(std::io::BufReader::new(file))
        .read_info()
        .map_err(|e| format!("png decode: {e}"))?;
    let info = reader.info();
    Ok((info.width, info.height))
}

struct Args {
    monitor: Option<usize>,
    all: bool,
}

fn parse_args() -> Args {
    let mut args = Args {
        monitor: None,
        all: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--all" => args.all = true,
            "--monitor" => args.monitor = it.next().and_then(|v| v.parse().ok()),
            other => eprintln!("ignoring unknown argument {other}"),
        }
    }
    args
}

fn print_monitors(monitors: &[Monitor]) {
    println!("== monitors (physical pixels, per-monitor DPI) ==");
    for m in monitors {
        println!(
            "  [{}] {} handle={} rect=({},{})..({},{}) {}x{} dpi={} scale={}%{}",
            m.index,
            m.device,
            m.hmonitor_hex(),
            m.rect.left,
            m.rect.top,
            m.rect.right,
            m.rect.bottom,
            m.width(),
            m.height(),
            m.effective_dpi,
            m.scale_percent(),
            if m.primary { " PRIMARY" } else { "" }
        );
    }
}

pub fn run() {
    let args = parse_args();
    let mut checks: Vec<Check> = Vec::new();

    // Per-monitor v2 makes every rect, size and cursor position physical pixels.
    // SAFETY: called once at startup before any window exists.
    let dpi_aware =
        unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    checks.push(check(
        "W1",
        "per-monitor DPI awareness v2 set",
        dpi_aware.is_ok(),
        format!("{dpi_aware:?}"),
    ));
    // SAFETY: first WinRT init on this thread.
    let ro = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
    checks.push(check(
        "W2",
        "RoInitialize (MTA)",
        ro.is_ok(),
        format!("{ro:?}"),
    ));

    let supported = GraphicsCaptureSession::IsSupported().unwrap_or(false);
    checks.push(check(
        "W3",
        "GraphicsCaptureSession::IsSupported",
        supported,
        supported.to_string(),
    ));

    let monitors = match enumerate_monitors() {
        Ok(m) if !m.is_empty() => m,
        other => {
            checks.push(check(
                "W4",
                "monitors enumerated",
                false,
                format!("{:?}", other.err()),
            ));
            return finish(checks, Vec::new(), Value::Null);
        }
    };
    checks.push(check(
        "W4",
        "monitors enumerated",
        true,
        format!("{} monitor(s)", monitors.len()),
    ));
    print_monitors(&monitors);

    let mut cursor = POINT::default();
    // SAFETY: valid out-pointer.
    let cursor_ok = unsafe { GetCursorPos(&mut cursor) }.is_ok();
    // SAFETY: plain query.
    let cursor_monitor = unsafe { MonitorFromPoint(cursor, MONITOR_DEFAULTTONULL) };
    let cursor_on = monitors
        .iter()
        .find(|m| m.handle == cursor_monitor)
        .map(|m| m.index);
    println!(
        "cursor: ({}, {}) physical px, on monitor {:?}",
        cursor.x, cursor.y, cursor_on
    );

    let selected: Vec<&Monitor> = if args.all {
        monitors.iter().collect()
    } else if let Some(i) = args.monitor {
        monitors.iter().filter(|m| m.index == i).collect()
    } else {
        monitors.iter().filter(|m| m.primary).collect()
    };
    if selected.is_empty() {
        checks.push(check(
            "W5",
            "monitor selected",
            false,
            "no monitor matches --monitor",
        ));
        return finish(checks, Vec::new(), Value::Null);
    }

    let gpu = match create_gpu() {
        Ok(g) => g,
        Err(e) => {
            checks.push(check("W5", "D3D11 device created", false, e.to_string()));
            return finish(checks, Vec::new(), Value::Null);
        }
    };
    checks.push(check(
        "W5",
        "D3D11 device created",
        true,
        "hardware, BGRA support",
    ));

    let out_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("out");
    if let Err(e) = std::fs::create_dir_all(&out_dir) {
        checks.push(check("W6", "out/ created", false, e.to_string()));
        return finish(checks, Vec::new(), Value::Null);
    }

    let mut captured_scales: HashSet<u32> = HashSet::new();
    let mut evidence = Vec::new();
    for m in selected {
        let id = format!("monitor{}", m.index);
        let frame = match capture_monitor(&gpu, m) {
            Ok(frame) => frame,
            Err(e) => {
                checks.push(check(
                    format!("{id}.capture"),
                    "frame captured",
                    false,
                    e.to_string(),
                ));
                continue;
            }
        };
        checks.push(check(
            format!("{id}.capture"),
            "frame captured",
            true,
            format!("first frame after {} ms", frame.time_to_frame.as_millis()),
        ));
        checks.push(check(
            format!("{id}.size"),
            "item size == monitor physical size",
            frame.item_size == (m.width(), m.height()),
            format!(
                "item={:?} monitor={}x{}",
                frame.item_size,
                m.width(),
                m.height()
            ),
        ));
        checks.push(check(
            format!("{id}.texture"),
            "texture size == item size",
            frame.texture_size == (frame.item_size.0 as u32, frame.item_size.1 as u32),
            format!(
                "texture={:?} content={:?}",
                frame.texture_size, frame.content_size
            ),
        ));
        let colours = distinct_colours(&frame.rgba);
        checks.push(check(
            format!("{id}.content"),
            "frame is not one uniform colour",
            colours >= MIN_DISTINCT_COLOURS,
            format!("{colours} distinct sampled colours (need >= {MIN_DISTINCT_COLOURS})"),
        ));
        let path = out_dir.join(format!("monitor{}.png", m.index));
        let written = write_png(
            &path,
            frame.texture_size.0,
            frame.texture_size.1,
            &frame.rgba,
        )
        .and_then(|()| read_png_dims(&path));
        checks.push(check(
            format!("{id}.png"),
            "PNG written and re-read with matching size",
            written.as_ref().is_ok_and(|d| *d == frame.texture_size),
            format!("{} -> {:?}", path.display(), written),
        ));
        captured_scales.insert(m.scale_percent());
        evidence.push(json!({
            "monitor_index": m.index,
            "device": m.device,
            "hmonitor": m.hmonitor_hex(),
            "primary": m.primary,
            "rect": [m.rect.left, m.rect.top, m.rect.right, m.rect.bottom],
            "effective_dpi": m.effective_dpi,
            "scale_percent": m.scale_percent(),
            "item_size": [frame.item_size.0, frame.item_size.1],
            "texture_size": [frame.texture_size.0, frame.texture_size.1],
            "png": path.display().to_string(),
            "cursor_in_this_monitor": cursor_on == Some(m.index),
        }));
    }

    checks.push(check(
        "M1",
        "monitor id, DPI scale and cursor position recorded",
        cursor_ok && !evidence.is_empty(),
        format!(
            "cursor=({}, {}) on monitor {:?}; see out/record.json",
            cursor.x, cursor.y, cursor_on
        ),
    ));
    checks.push(if captured_scales.len() >= 2 {
        check(
            "M2",
            "captured two monitors with different scale",
            true,
            format!("scales {captured_scales:?}%"),
        )
    } else {
        let attached: HashSet<u32> = monitors.iter().map(Monitor::scale_percent).collect();
        let hint = if attached.len() < 2 {
            "no attached monitor has a different scale, so M2 cannot be tested on this machine"
        } else {
            "monitors with different scales are attached; run with --all"
        };
        Check {
            id: "M2".into(),
            name: "captured two monitors with different scale",
            status: Status::Unverified,
            detail: format!(
                "captured scales {captured_scales:?}%; attached scales {attached:?}%; {hint}"
            ),
        }
    });

    let cursor_json = json!({ "x": cursor.x, "y": cursor.y, "monitor_index": cursor_on });
    finish(checks, evidence, cursor_json);
}

/// Writes `out/record.json` (always, so a stopped run never leaves an older record that looks
/// current), then prints the checks and the one-line spike status.
fn finish(mut checks: Vec<Check>, captures: Vec<Value>, cursor: Value) {
    let captured_any = !captures.is_empty();
    let record_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("out")
        .join("record.json");
    let record_check = write_record(&record_path, &checks, captures, cursor);
    checks.push(record_check);

    println!("\n== checks ==");
    for c in &checks {
        println!("  [{}] {} {}: {}", c.status.label(), c.id, c.name, c.detail);
    }
    let line = if let Some(c) = checks.iter().find(|c| c.status == Status::Fail) {
        format!(
            "SPIKE STATUS: capture NOT proven (reason: {} {} is FAIL: {})",
            c.id, c.name, c.detail
        )
    } else if !captured_any {
        "SPIKE STATUS: capture NOT proven (stopped before any capture)".to_string()
    } else if checks
        .iter()
        .any(|c| c.id == "M2" && c.status == Status::Pass)
    {
        "SPIKE STATUS: capture proven, including two monitors with different scale".to_string()
    } else {
        "SPIKE STATUS: capture proven EXCEPT M2 (two-monitor different-scale) is UNVERIFIED"
            .to_string()
    };
    println!("\n{line}");
}

/// The record lists the checks as they stand before this write; its own outcome is returned as
/// check R1 so a failed write turns the spike status into NOT proven.
fn write_record(path: &Path, checks: &[Check], captures: Vec<Value>, cursor: Value) -> Check {
    let record = json!({
        "spike": 17,
        "cursor": cursor,
        "captures": captures,
        "checks": checks.iter().map(|c| json!({
            "id": c.id, "name": c.name, "status": c.status.label(), "detail": c.detail
        })).collect::<Vec<_>>(),
    });
    let written = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .map_err(|e| format!("create out/: {e}"))
        .and_then(|()| serde_json::to_vec_pretty(&record).map_err(|e| format!("serialise: {e}")))
        .and_then(|bytes| std::fs::write(path, bytes).map_err(|e| format!("write: {e}")));
    check(
        "R1",
        "out/record.json written",
        written.is_ok(),
        match written {
            Ok(()) => path.display().to_string(),
            Err(e) => e,
        },
    )
}
