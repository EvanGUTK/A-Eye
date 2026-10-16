//! Raw frame -> PNG conversion. Pure functions, no desktop required.
//!
//! Rules:
//! - `stride` is the byte distance between row starts and may exceed
//!   `width * bytes_per_pixel`; padding bytes are dropped.
//! - Formats with an `x` (padding) channel ignore that byte. Formats with an
//!   alpha channel have alpha discarded: screen evidence is opaque and
//!   compositors do not guarantee a meaningful alpha value. The PNG is RGB8.
//! - Formats are named by SPA/PipeWire convention: the name lists channels in
//!   *memory byte order* (so `BGRx` is bytes `B,G,R,x`).

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    Bgrx,
    Bgra,
    Rgbx,
    Rgba,
    Xrgb,
    Xbgr,
    Argb,
    Abgr,
    Rgb,
    Bgr,
}

impl PixelFormat {
    pub const ALL: [PixelFormat; 10] = [
        PixelFormat::Bgrx,
        PixelFormat::Bgra,
        PixelFormat::Rgbx,
        PixelFormat::Rgba,
        PixelFormat::Xrgb,
        PixelFormat::Xbgr,
        PixelFormat::Argb,
        PixelFormat::Abgr,
        PixelFormat::Rgb,
        PixelFormat::Bgr,
    ];

    pub fn bytes_per_pixel(self) -> usize {
        match self {
            PixelFormat::Rgb | PixelFormat::Bgr => 3,
            _ => 4,
        }
    }

    /// Byte offsets of (R, G, B) within one pixel.
    pub fn rgb_offsets(self) -> (usize, usize, usize) {
        match self {
            PixelFormat::Bgrx | PixelFormat::Bgra => (2, 1, 0),
            PixelFormat::Rgbx | PixelFormat::Rgba => (0, 1, 2),
            PixelFormat::Xrgb | PixelFormat::Argb => (1, 2, 3),
            PixelFormat::Xbgr | PixelFormat::Abgr => (3, 2, 1),
            PixelFormat::Rgb => (0, 1, 2),
            PixelFormat::Bgr => (2, 1, 0),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            PixelFormat::Bgrx => "BGRx",
            PixelFormat::Bgra => "BGRA",
            PixelFormat::Rgbx => "RGBx",
            PixelFormat::Rgba => "RGBA",
            PixelFormat::Xrgb => "xRGB",
            PixelFormat::Xbgr => "xBGR",
            PixelFormat::Argb => "ARGB",
            PixelFormat::Abgr => "ABGR",
            PixelFormat::Rgb => "RGB",
            PixelFormat::Bgr => "BGR",
        }
    }
}

impl fmt::Display for PixelFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum FrameError {
    ZeroSize,
    StrideTooSmall { stride: usize, min: usize },
    BufferTooSmall { have: usize, need: usize },
    Overflow,
    Encode(String),
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FrameError::ZeroSize => write!(f, "frame has zero width or height"),
            FrameError::StrideTooSmall { stride, min } => {
                write!(f, "stride {stride} is smaller than one row ({min} bytes)")
            }
            FrameError::BufferTooSmall { have, need } => {
                write!(f, "buffer holds {have} bytes but the frame needs {need}")
            }
            FrameError::Overflow => write!(f, "frame size arithmetic overflowed"),
            FrameError::Encode(e) => write!(f, "PNG encode failed: {e}"),
        }
    }
}

impl std::error::Error for FrameError {}

/// A tightly validated CPU-memory frame. `data` starts at the first row.
#[derive(Debug, Clone)]
pub struct RawFrame {
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub format: PixelFormat,
    pub data: Vec<u8>,
}

impl RawFrame {
    /// Validate geometry against the buffer. The last row may be shorter than
    /// `stride` (only `width * bpp` bytes are required for it).
    pub fn new(
        width: u32,
        height: u32,
        stride: usize,
        format: PixelFormat,
        data: Vec<u8>,
    ) -> Result<Self, FrameError> {
        if width == 0 || height == 0 {
            return Err(FrameError::ZeroSize);
        }
        let row_bytes = (width as usize)
            .checked_mul(format.bytes_per_pixel())
            .ok_or(FrameError::Overflow)?;
        if stride < row_bytes {
            return Err(FrameError::StrideTooSmall {
                stride,
                min: row_bytes,
            });
        }
        let need = stride
            .checked_mul(height as usize - 1)
            .and_then(|v| v.checked_add(row_bytes))
            .ok_or(FrameError::Overflow)?;
        if data.len() < need {
            return Err(FrameError::BufferTooSmall {
                have: data.len(),
                need,
            });
        }
        Ok(Self {
            width,
            height,
            stride,
            format,
            data,
        })
    }

    /// Tightly packed RGB8 (width * height * 3 bytes), padding and alpha dropped.
    pub fn to_rgb8(&self) -> Vec<u8> {
        let bpp = self.format.bytes_per_pixel();
        let (r, g, b) = self.format.rgb_offsets();
        let w = self.width as usize;
        let mut out = Vec::with_capacity(w * self.height as usize * 3);
        for y in 0..self.height as usize {
            let row = &self.data[y * self.stride..y * self.stride + w * bpp];
            for px in row.chunks_exact(bpp) {
                out.extend_from_slice(&[px[r], px[g], px[b]]);
            }
        }
        out
    }

    /// True when every pixel has identical RGB (blank/black frame heuristic).
    /// Returns only a boolean; no pixel content leaves this function.
    pub fn is_uniform(&self) -> bool {
        let rgb = self.to_rgb8();
        let (px, _) = rgb.as_chunks::<3>();
        match px.first() {
            Some(first) => px.iter().all(|p| p == first),
            None => true,
        }
    }

    pub fn encode_png(&self) -> Result<Vec<u8>, FrameError> {
        let rgb = self.to_rgb8();
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, self.width, self.height);
            enc.set_color(png::ColorType::Rgb);
            enc.set_depth(png::BitDepth::Eight);
            let mut w = enc
                .write_header()
                .map_err(|e| FrameError::Encode(e.to_string()))?;
            w.write_image_data(&rgb)
                .map_err(|e| FrameError::Encode(e.to_string()))?;
        }
        Ok(out)
    }
}

/// Header-only PNG metadata (no pixel decoding): used to verify a saved file
/// without ever looking at the (possibly private) pixels.
#[derive(Debug, PartialEq, Eq)]
pub struct PngHeader {
    pub width: u32,
    pub height: u32,
    pub color_type: String,
    pub bit_depth: String,
}

pub fn read_png_header(bytes: &[u8]) -> Result<PngHeader, String> {
    let dec = png::Decoder::new(std::io::Cursor::new(bytes));
    let reader = dec.read_info().map_err(|e| e.to_string())?;
    let info = reader.info();
    Ok(PngHeader {
        width: info.width,
        height: info.height,
        color_type: format!("{:?}", info.color_type),
        bit_depth: format!("{:?}", info.bit_depth),
    })
}

#[derive(Debug)]
pub struct SavedPng {
    pub path: std::path::PathBuf,
    pub bytes: u64,
    pub encode_time: std::time::Duration,
    pub header: PngHeader,
}

/// Encode and write `frame` to `dir/name` (dir mode 0700, file mode 0600:
/// screen captures are private). Re-reads only the PNG header to verify it.
pub fn save_png(frame: &RawFrame, dir: &std::path::Path, name: &str) -> Result<SavedPng, String> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

    let t = std::time::Instant::now();
    let png = frame.encode_png().map_err(|e| e.to_string())?;
    let encode_time = t.elapsed();

    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|e| format!("create {}: {e}", dir.display()))?;
    let path = dir.join(name);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)
        .map_err(|e| format!("open {}: {e}", path.display()))?;
    f.write_all(&png)
        .map_err(|e| format!("write {}: {e}", path.display()))?;
    drop(f);

    let on_disk = std::fs::read(&path).map_err(|e| format!("re-read {}: {e}", path.display()))?;
    let header = read_png_header(&on_disk)?;
    Ok(SavedPng {
        path,
        bytes: on_disk.len() as u64,
        encode_time,
        header,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic distinct RGB per pixel so channel swaps are detectable.
    fn expected_rgb(x: u32, y: u32) -> [u8; 3] {
        [
            (x * 37 + y * 11 + 1) as u8,
            (x * 5 + y * 91 + 2) as u8,
            (x * 113 + y * 7 + 3) as u8,
        ]
    }

    /// Build a synthetic frame in `format` with `pad` extra bytes per row
    /// (stride = width*bpp + pad). Padding and x/alpha channels are filled
    /// with sentinel values that must NOT leak into the PNG.
    fn synth(format: PixelFormat, w: u32, h: u32, pad: usize) -> RawFrame {
        let bpp = format.bytes_per_pixel();
        let stride = w as usize * bpp + pad;
        let (ro, go, bo) = format.rgb_offsets();
        let mut data = vec![0xEE; stride * h as usize]; // padding sentinel
        for y in 0..h {
            for x in 0..w {
                let base = y as usize * stride + x as usize * bpp;
                for i in 0..bpp {
                    data[base + i] = 0x7F; // x/alpha sentinel
                }
                let [r, g, b] = expected_rgb(x, y);
                data[base + ro] = r;
                data[base + go] = g;
                data[base + bo] = b;
            }
        }
        RawFrame::new(w, h, stride, format, data).unwrap()
    }

    fn decode(png_bytes: &[u8]) -> (u32, u32, Vec<u8>) {
        let dec = png::Decoder::new(std::io::Cursor::new(png_bytes));
        let mut reader = dec.read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!(info.color_type, png::ColorType::Rgb);
        assert_eq!(info.bit_depth, png::BitDepth::Eight);
        buf.truncate(info.buffer_size());
        (info.width, info.height, buf)
    }

    fn assert_roundtrip(format: PixelFormat, w: u32, h: u32, pad: usize) {
        let frame = synth(format, w, h, pad);
        let png = frame.encode_png().unwrap();
        let (dw, dh, px) = decode(&png);
        assert_eq!((dw, dh), (w, h), "{format} dims");
        assert_eq!(px.len(), (w * h * 3) as usize, "{format} byte count");
        for y in 0..h {
            for x in 0..w {
                let o = ((y * w + x) * 3) as usize;
                assert_eq!(
                    &px[o..o + 3],
                    &expected_rgb(x, y),
                    "{format} pad={pad} pixel ({x},{y})"
                );
            }
        }
    }

    #[test]
    fn every_supported_format_roundtrips_tight_stride() {
        for f in PixelFormat::ALL {
            assert_roundtrip(f, 7, 5, 0);
        }
    }

    #[test]
    fn every_supported_format_roundtrips_padded_stride() {
        // stride != width*bpp (e.g. 64-byte row alignment on a 7px-wide frame)
        for f in PixelFormat::ALL {
            assert_roundtrip(f, 7, 5, 5);
            assert_roundtrip(f, 7, 5, 36);
        }
    }

    #[test]
    fn stride_not_width_times_four_is_really_exercised() {
        let f = synth(PixelFormat::Bgrx, 10, 3, 24);
        assert_ne!(f.stride, f.width as usize * 4);
        assert_roundtrip(PixelFormat::Bgrx, 10, 3, 24);
    }

    #[test]
    fn last_row_may_omit_trailing_padding() {
        let mut f = synth(PixelFormat::Bgra, 4, 3, 16);
        let need = f.stride * 2 + 4 * 4;
        f.data.truncate(need);
        let again = RawFrame::new(4, 3, f.stride, f.format, f.data.clone()).unwrap();
        let png = again.encode_png().unwrap();
        let (_, _, px) = decode(&png);
        assert_eq!(&px[0..3], &expected_rgb(0, 0));
    }

    #[test]
    fn single_pixel_and_single_row_edges() {
        assert_roundtrip(PixelFormat::Rgba, 1, 1, 0);
        assert_roundtrip(PixelFormat::Rgb, 1, 9, 3);
        assert_roundtrip(PixelFormat::Bgr, 13, 1, 1);
    }

    #[test]
    fn rejects_bad_geometry() {
        assert_eq!(
            RawFrame::new(0, 4, 16, PixelFormat::Bgrx, vec![0; 64]).unwrap_err(),
            FrameError::ZeroSize
        );
        assert_eq!(
            RawFrame::new(4, 4, 15, PixelFormat::Bgrx, vec![0; 64]).unwrap_err(),
            FrameError::StrideTooSmall {
                stride: 15,
                min: 16
            }
        );
        assert_eq!(
            RawFrame::new(4, 4, 16, PixelFormat::Bgrx, vec![0; 63]).unwrap_err(),
            FrameError::BufferTooSmall { have: 63, need: 64 }
        );
        assert_eq!(
            RawFrame::new(u32::MAX, u32::MAX, usize::MAX, PixelFormat::Rgba, vec![]).unwrap_err(),
            FrameError::Overflow
        );
    }

    #[test]
    fn alpha_and_padding_bytes_do_not_leak_into_pixels() {
        // sentinel 0x7F x/alpha and 0xEE padding must never appear unless the
        // expected pixel value coincidentally equals them (checked by design:
        // expected_rgb is verified exactly in assert_roundtrip).
        let f = synth(PixelFormat::Argb, 3, 3, 8);
        let rgb = f.to_rgb8();
        assert_eq!(rgb.len(), 27);
        assert_eq!(&rgb[0..3], &expected_rgb(0, 0));
    }

    #[test]
    fn uniform_detection() {
        let uniform = RawFrame::new(2, 2, 8, PixelFormat::Bgrx, vec![9; 16]).unwrap();
        assert!(uniform.is_uniform());
        assert!(!synth(PixelFormat::Bgrx, 4, 4, 0).is_uniform());
    }

    #[test]
    fn png_header_reader_matches_encoder() {
        let f = synth(PixelFormat::Rgbx, 9, 4, 4);
        let png = f.encode_png().unwrap();
        let h = read_png_header(&png).unwrap();
        assert_eq!((h.width, h.height), (9, 4));
        assert_eq!(h.color_type, "Rgb");
        assert_eq!(h.bit_depth, "Eight");
    }

    #[test]
    fn save_png_writes_private_file_with_correct_header() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("aeye-spike04-test-{}", std::process::id()));
        let f = synth(PixelFormat::Bgrx, 11, 6, 20);
        let saved = save_png(&f, &dir, "synthetic.png").unwrap();
        assert_eq!((saved.header.width, saved.header.height), (11, 6));
        assert!(saved.bytes > 0);
        let mode = std::fs::metadata(&saved.path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let (w, h, px) = decode(&std::fs::read(&saved.path).unwrap());
        assert_eq!((w, h), (11, 6));
        assert_eq!(&px[0..3], &expected_rgb(0, 0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn format_table_is_consistent() {
        for f in PixelFormat::ALL {
            let (r, g, b) = f.rgb_offsets();
            let bpp = f.bytes_per_pixel();
            assert!(r < bpp && g < bpp && b < bpp);
            assert!(r != g && g != b && r != b, "{f} offsets must be distinct");
        }
    }
}
