//! Raw frame model shared by the PipeWire consumer and the PNG encoder.
//!
//! Rules:
//! - `stride` is the byte distance between row starts and may exceed
//!   `width * bytes_per_pixel`; padding bytes are not part of the image.
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

/// A validated CPU-memory frame. `data` starts at the first row.
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_bad_geometry() {
        assert_eq!(
            RawFrame::new(0, 4, 16, PixelFormat::Bgrx, vec![0; 64]).unwrap_err(),
            FrameError::ZeroSize
        );
        assert_eq!(
            RawFrame::new(4, 4, 15, PixelFormat::Bgrx, vec![0; 64]).unwrap_err(),
            FrameError::StrideTooSmall { stride: 15, min: 16 }
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
    fn last_row_may_omit_trailing_padding() {
        // 3 rows, stride 32, width 4 BGRA (16 bytes/row): last row needs only 16.
        let need = 32 * 2 + 16;
        assert!(RawFrame::new(4, 3, 32, PixelFormat::Bgra, vec![0; need]).is_ok());
        assert!(RawFrame::new(4, 3, 32, PixelFormat::Bgra, vec![0; need - 1]).is_err());
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
