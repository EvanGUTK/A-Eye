//! PipeWire consumer (issue #14): connect with the portal-provided fd + node
//! id, negotiate `video/raw` with SHM-friendly buffers, receive ONE complete
//! frame, copy it to CPU memory, disconnect.
//!
//! DMA-BUF is never requested: the EnumFormat offered carries no modifier
//! property, so a producer cannot satisfy it with a DMA-BUF allocation, and
//! the Buffers param restricts `dataType` to MemPtr|MemFd. If a producer
//! still hands us DMA-BUF the capture fails loudly instead of guessing.

use std::cell::RefCell;
use std::io::Cursor;
use std::rc::Rc;
use std::time::{Duration, Instant};

use pipewire as pw;
use pw::loop_::Timeout;
use pw::properties::properties;
use pw::spa;
use spa::buffer::meta::MetaHeader;
use spa::buffer::DataType;
use spa::param::video::{VideoFormat, VideoInfoRaw};
use spa::pod::{Object, Pod, Property, Value};

use crate::frame::{PixelFormat, RawFrame};
use crate::portal::CaptureTarget;

/// Upper bound for the first complete frame after the stream is connected.
pub const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
pub struct FrameMeta {
    /// Negotiated SPA video format name, e.g. "BGRx".
    pub negotiated_format: String,
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub chunk_size: u32,
    pub chunk_offset: u32,
    /// "MemFd" | "MemPtr" | "DmaBuf" | ...
    pub buffer_type: String,
    pub framerate: (u32, u32),
    pub modifier: u64,
    /// Producer presentation timestamp (SPA header meta), if provided.
    pub pts_ns: Option<i64>,
    pub seq: Option<u64>,
    /// Buffers skipped before this one (empty/corrupted chunks).
    pub skipped_buffers: u32,
    /// Stream connect -> first complete frame.
    pub connect_to_frame: Duration,
    /// Portal confirmation -> first complete frame (None for non-portal sources).
    pub confirm_to_frame: Option<Duration>,
}

#[derive(Debug)]
pub struct Captured {
    pub meta: FrameMeta,
    pub frame: RawFrame,
}

#[derive(Debug)]
pub enum CaptureError {
    Setup(String),
    Stream(String),
    UnsupportedFormat(String),
    UnsupportedBuffer(String),
    BadFrame(String),
    Timeout {
        waited: Duration,
        stream_state: String,
        negotiated: Option<String>,
    },
}

impl std::fmt::Display for CaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CaptureError::Setup(m) => write!(f, "pipewire setup failed: {m}"),
            CaptureError::Stream(m) => write!(f, "pipewire stream error: {m}"),
            CaptureError::UnsupportedFormat(m) => write!(f, "unsupported video format: {m}"),
            CaptureError::UnsupportedBuffer(m) => write!(f, "unsupported buffer: {m}"),
            CaptureError::BadFrame(m) => write!(f, "invalid frame: {m}"),
            CaptureError::Timeout {
                waited,
                stream_state,
                negotiated,
            } => write!(
                f,
                "no frame within {:.1}s (stream state {stream_state}, negotiated {})",
                waited.as_secs_f64(),
                negotiated.as_deref().unwrap_or("nothing")
            ),
        }
    }
}

impl std::error::Error for CaptureError {}

/// Map a SPA video format to a supported pixel format.
fn map_format(f: VideoFormat) -> Option<PixelFormat> {
    let raw = f.as_raw();
    [
        (VideoFormat::BGRx, PixelFormat::Bgrx),
        (VideoFormat::BGRA, PixelFormat::Bgra),
        (VideoFormat::RGBx, PixelFormat::Rgbx),
        (VideoFormat::RGBA, PixelFormat::Rgba),
        (VideoFormat::xRGB, PixelFormat::Xrgb),
        (VideoFormat::xBGR, PixelFormat::Xbgr),
        (VideoFormat::ARGB, PixelFormat::Argb),
        (VideoFormat::ABGR, PixelFormat::Abgr),
        (VideoFormat::RGB, PixelFormat::Rgb),
        (VideoFormat::BGR, PixelFormat::Bgr),
    ]
    .into_iter()
    .find(|(v, _)| v.as_raw() == raw)
    .map(|(_, p)| p)
}

struct State {
    format: VideoInfoRaw,
    pixel: Option<PixelFormat>,
    unsupported: Option<String>,
    negotiated: Option<String>,
    skipped: u32,
    stream_state: String,
    stream_error: Option<String>,
    core_error: Option<String>,
    connected_at: Instant,
    confirmed_at: Option<Instant>,
    result: Option<Result<Captured, CaptureError>>,
}

fn serialize(obj: Object) -> Vec<u8> {
    spa::pod::serialize::PodSerializer::serialize(Cursor::new(Vec::new()), &Value::Object(obj))
        .expect("pod serialization of a static object cannot fail")
        .0
        .into_inner()
}

fn enum_format_pod() -> Vec<u8> {
    use spa::param::format::{FormatProperties, MediaSubtype, MediaType};
    use spa::param::video::VideoFormat as V;
    use spa::utils::{Fraction, Rectangle, SpaTypes};
    serialize(spa::pod::object!(
        SpaTypes::ObjectParamFormat,
        spa::param::ParamType::EnumFormat,
        spa::pod::property!(FormatProperties::MediaType, Id, MediaType::Video),
        spa::pod::property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
        spa::pod::property!(
            FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            V::BGRx,
            V::BGRx,
            V::BGRA,
            V::RGBx,
            V::RGBA,
            V::xRGB,
            V::xBGR,
            V::ARGB,
            V::ABGR,
            V::RGB,
            V::BGR,
        ),
        spa::pod::property!(
            FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            Rectangle {
                width: 1920,
                height: 1080
            },
            Rectangle {
                width: 1,
                height: 1
            },
            Rectangle {
                width: 16384,
                height: 16384
            }
        ),
        spa::pod::property!(
            FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            Fraction { num: 30, denom: 1 },
            Fraction { num: 0, denom: 1 },
            Fraction {
                num: 1000,
                denom: 1
            }
        ),
    ))
}

/// Restrict buffer memory to MemPtr|MemFd (no DMA-BUF). Only `dataType` is
/// constrained; other buffer properties are taken from the producer.
fn shm_buffers_pod() -> Vec<u8> {
    let mask = (1i32 << DataType::MemPtr.as_raw()) | (1i32 << DataType::MemFd.as_raw());
    serialize(Object {
        type_: spa::utils::SpaTypes::ObjectParamBuffers.as_raw(),
        id: spa::param::ParamType::Buffers.as_raw(),
        properties: vec![Property::new(
            spa::sys::SPA_PARAM_BUFFERS_dataType,
            Value::Int(mask),
        )],
    })
}

/// Ask producers to attach the SPA header meta (pts/seq/flags) to buffers.
fn header_meta_pod() -> Vec<u8> {
    serialize(Object {
        type_: spa::sys::SPA_TYPE_OBJECT_ParamMeta,
        id: spa::param::ParamType::Meta.as_raw(),
        properties: vec![
            Property::new(
                spa::sys::SPA_PARAM_META_type,
                Value::Id(spa::utils::Id(spa::sys::SPA_META_Header)),
            ),
            Property::new(
                spa::sys::SPA_PARAM_META_size,
                Value::Int(std::mem::size_of::<spa::sys::spa_meta_header>() as i32),
            ),
        ],
    })
}

/// Capture exactly one frame from the node the user selected in the portal.
/// Requires a [`CaptureTarget`], which only a started portal session can mint.
pub fn capture_one_frame(
    target: CaptureTarget,
    timeout: Duration,
) -> Result<Captured, CaptureError> {
    let node_id = target.node_id();
    let confirmed_at = target.confirmed_at();
    let fd = target.into_fd();
    run(Some(fd), node_id, timeout, Some(confirmed_at))
}

/// Developer self-test: consume a node of the user's default PipeWire daemon
/// (e.g. a `gst-launch-1.0 videotestsrc ! pipewiresink` test pattern) WITHOUT
/// the portal. Never used by the main binary; it cannot reach the screen
/// unless the caller passes a screen node id, so callers must only pass
/// synthetic sources. Results from it must never be recorded as portal PASS.
pub fn capture_synthetic_node(node_id: u32, timeout: Duration) -> Result<Captured, CaptureError> {
    run(None, node_id, timeout, None)
}

fn run(
    fd: Option<std::os::fd::OwnedFd>,
    node_id: u32,
    timeout: Duration,
    confirmed_at: Option<Instant>,
) -> Result<Captured, CaptureError> {
    pw::init();
    let setup = |e: pw::Error| CaptureError::Setup(e.to_string());
    let mainloop = pw::main_loop::MainLoopBox::new(None).map_err(setup)?;
    let context = pw::context::ContextBox::new(mainloop.loop_(), None).map_err(setup)?;
    let core = match fd {
        Some(fd) => context.connect_fd(fd, None),
        None => context.connect(None),
    }
    .map_err(setup)?;

    let state = Rc::new(RefCell::new(State {
        format: VideoInfoRaw::default(),
        pixel: None,
        unsupported: None,
        negotiated: None,
        skipped: 0,
        stream_state: "Unconnected".to_string(),
        stream_error: None,
        core_error: None,
        connected_at: Instant::now(),
        confirmed_at,
        result: None,
    }));

    let core_state = state.clone();
    let _core_listener = core
        .add_listener_local()
        .error(move |id, seq, res, msg| {
            core_state.borrow_mut().core_error =
                Some(format!("object {id} seq {seq} res {res}: {msg}"));
        })
        .register();

    let stream = pw::stream::StreamBox::new(
        &core,
        "a-eye-spike-04",
        properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )
    .map_err(setup)?;

    let _listener = stream
        .add_local_listener_with_user_data(state.clone())
        .state_changed(|_, st, _old, new| {
            let mut s = st.borrow_mut();
            s.stream_state = match &new {
                pw::stream::StreamState::Error(e) => {
                    s.stream_error = Some(e.clone());
                    format!("Error({e})")
                }
                other => format!("{other:?}"),
            };
        })
        .param_changed(|stream, st, id, param| {
            let Some(param) = param else { return };
            if id != spa::param::ParamType::Format.as_raw() {
                return;
            }
            let Ok((mt, ms)) = spa::param::format_utils::parse_format(param) else {
                return;
            };
            if mt != spa::param::format::MediaType::Video
                || ms != spa::param::format::MediaSubtype::Raw
            {
                return;
            }
            let mut s = st.borrow_mut();
            if s.format.parse(param).is_err() {
                s.unsupported = Some("could not parse negotiated video/raw format".to_string());
                return;
            }
            let vf = s.format.format();
            s.negotiated = Some(format!("{vf:?}"));
            match map_format(vf) {
                Some(p) => s.pixel = Some(p),
                None => {
                    s.unsupported = Some(format!("{vf:?} is not one of the supported formats"));
                    return;
                }
            }
            drop(s);
            let buffers = shm_buffers_pod();
            let meta = header_meta_pod();
            if let (Some(b), Some(m)) = (Pod::from_bytes(&buffers), Pod::from_bytes(&meta)) {
                let _ = stream.update_params(&mut [b, m]);
            }
        })
        .process(|stream, st| {
            if st.borrow().result.is_some() {
                return;
            }
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let hdr = buffer
                .find_meta::<MetaHeader>()
                .map(|h| (h.pts(), h.seq(), h.flags()));
            let datas = buffer.datas_mut();
            let Some(d) = datas.first_mut() else { return };

            let mut s = st.borrow_mut();
            let (Some(pixel), None) = (s.pixel, s.unsupported.as_ref()) else {
                return; // format not negotiated (or unsupported): wait / time out
            };
            let size = s.format.size();
            let (w, h) = (size.width, size.height);
            let buffer_type = format!("{:?}", d.type_());
            let chunk_size = d.chunk().size();
            let chunk_offset = d.chunk().offset();
            let chunk_stride = d.chunk().stride();
            let chunk_corrupt = d
                .chunk()
                .flags()
                .contains(spa::buffer::ChunkFlags::CORRUPTED);
            let hdr_corrupt = hdr
                .as_ref()
                .is_some_and(|(_, _, f)| f.contains(spa::buffer::meta::MetaHeaderFlags::CORRUPTED));
            if chunk_size == 0 || chunk_corrupt || hdr_corrupt {
                s.skipped += 1; // metadata-only / empty / corrupted buffer
                return;
            }
            if d.type_() == DataType::DmaBuf {
                s.result = Some(Err(CaptureError::UnsupportedBuffer(
                    "producer delivered DmaBuf although only MemPtr|MemFd were requested"
                        .to_string(),
                )));
                return;
            }
            let Some(mapped) = d.data() else {
                s.result = Some(Err(CaptureError::UnsupportedBuffer(format!(
                    "{buffer_type} buffer is not CPU-mapped"
                ))));
                return;
            };
            let off = chunk_offset as usize;
            let end = off.saturating_add(chunk_size as usize).min(mapped.len());
            if off >= end {
                s.result = Some(Err(CaptureError::BadFrame(format!(
                    "chunk offset {off} size {chunk_size} outside mapped {} bytes",
                    mapped.len()
                ))));
                return;
            }
            let bpp = pixel.bytes_per_pixel();
            let stride = if chunk_stride > 0 {
                chunk_stride as usize
            } else {
                w as usize * bpp
            };
            let data = mapped[off..end].to_vec();
            let result = RawFrame::new(w, h, stride, pixel, data)
                .map_err(|e| CaptureError::BadFrame(e.to_string()))
                .map(|frame| {
                    let fr = s.format.framerate();
                    Captured {
                        meta: FrameMeta {
                            negotiated_format: s.negotiated.clone().unwrap_or_default(),
                            width: w,
                            height: h,
                            stride,
                            chunk_size,
                            chunk_offset,
                            buffer_type: buffer_type.clone(),
                            framerate: (fr.num, fr.denom),
                            modifier: s.format.modifier(),
                            pts_ns: hdr.as_ref().map(|(p, _, _)| *p),
                            seq: hdr.as_ref().map(|(_, q, _)| *q),
                            skipped_buffers: s.skipped,
                            connect_to_frame: s.connected_at.elapsed(),
                            confirm_to_frame: s.confirmed_at.map(|c| c.elapsed()),
                        },
                        frame,
                    }
                });
            s.result = Some(result);
        })
        .register()
        .map_err(setup)?;

    let fmt_bytes = enum_format_pod();
    let mut params = [Pod::from_bytes(&fmt_bytes).expect("static format pod is valid")];
    state.borrow_mut().connected_at = Instant::now();
    stream
        .connect(
            spa::utils::Direction::Input,
            Some(node_id),
            pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .map_err(setup)?;

    let started = Instant::now();
    let outcome = loop {
        {
            let mut s = state.borrow_mut();
            if let Some(r) = s.result.take() {
                break r;
            }
            if let Some(e) = s.unsupported.take() {
                break Err(CaptureError::UnsupportedFormat(e));
            }
            if let Some(e) = s.stream_error.take().or_else(|| s.core_error.take()) {
                break Err(CaptureError::Stream(e));
            }
            if started.elapsed() >= timeout {
                break Err(CaptureError::Timeout {
                    waited: started.elapsed(),
                    stream_state: s.stream_state.clone(),
                    negotiated: s.negotiated.clone(),
                });
            }
        }
        mainloop
            .loop_()
            .iterate(Timeout::Finite(Duration::from_millis(50)));
    };

    let _ = stream.disconnect();
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_supported_spa_format_maps_to_a_distinct_pixel_format() {
        let spa_formats = [
            VideoFormat::BGRx,
            VideoFormat::BGRA,
            VideoFormat::RGBx,
            VideoFormat::RGBA,
            VideoFormat::xRGB,
            VideoFormat::xBGR,
            VideoFormat::ARGB,
            VideoFormat::ABGR,
            VideoFormat::RGB,
            VideoFormat::BGR,
        ];
        let mapped: Vec<PixelFormat> = spa_formats
            .iter()
            .map(|f| map_format(*f).expect("supported"))
            .collect();
        for p in PixelFormat::ALL {
            assert_eq!(mapped.iter().filter(|m| **m == p).count(), 1, "{p}");
        }
    }

    #[test]
    fn unsupported_formats_are_rejected_not_guessed() {
        assert!(map_format(VideoFormat::YUY2).is_none());
        assert!(map_format(VideoFormat::I420).is_none());
        assert!(map_format(VideoFormat::RGBA_F32).is_none());
    }

    #[test]
    fn negotiation_pods_are_valid_and_exclude_dmabuf() {
        for bytes in [enum_format_pod(), shm_buffers_pod(), header_meta_pod()] {
            assert!(Pod::from_bytes(&bytes).is_some());
        }
        // MemPtr(1) | MemFd(2) => bits 1 and 2 => 0b110; DmaBuf(3) bit must be clear.
        let mask = (1i32 << DataType::MemPtr.as_raw()) | (1i32 << DataType::MemFd.as_raw());
        assert_eq!(mask, 0b110);
        assert_eq!(mask & (1i32 << DataType::DmaBuf.as_raw()), 0);
    }

    #[test]
    fn timeout_error_names_state_and_negotiation() {
        let e = CaptureError::Timeout {
            waited: Duration::from_secs(10),
            stream_state: "Paused".into(),
            negotiated: Some("BGRx".into()),
        };
        let s = e.to_string();
        assert!(s.contains("10.0s") && s.contains("Paused") && s.contains("BGRx"));
    }
}
