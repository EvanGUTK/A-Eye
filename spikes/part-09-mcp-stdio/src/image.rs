//! Frame downscaling and PNG encoding. Pure CPU work, no OS calls.

use crate::backend::Frame;

const CHANNELS_IN: usize = 4;
const CHANNELS_OUT: usize = 3;

/// Returns `frame` unchanged when both sides are within `max_dimension`; otherwise a box-filtered
/// copy whose longest side is `max_dimension`, keeping the aspect ratio.
pub fn downscale(frame: Frame, max_dimension: u32) -> Frame {
    let longest = frame.width.max(frame.height);
    if longest <= max_dimension {
        return frame;
    }
    let scale = f64::from(max_dimension) / f64::from(longest);
    let dst_w = ((f64::from(frame.width) * scale).round() as u32).max(1);
    let dst_h = ((f64::from(frame.height) * scale).round() as u32).max(1);
    let (src_w, src_h) = (frame.width as usize, frame.height as usize);
    let mut rgba = Vec::with_capacity(dst_w as usize * dst_h as usize * CHANNELS_IN);
    for dy in 0..dst_h as usize {
        let y0 = dy * src_h / dst_h as usize;
        let y1 = ((dy + 1) * src_h)
            .div_ceil(dst_h as usize)
            .clamp(y0 + 1, src_h);
        for dx in 0..dst_w as usize {
            let x0 = dx * src_w / dst_w as usize;
            let x1 = ((dx + 1) * src_w)
                .div_ceil(dst_w as usize)
                .clamp(x0 + 1, src_w);
            let mut sums = [0u64; CHANNELS_IN];
            for sy in y0..y1 {
                let row =
                    &frame.rgba[(sy * src_w + x0) * CHANNELS_IN..(sy * src_w + x1) * CHANNELS_IN];
                for px in row.as_chunks::<CHANNELS_IN>().0 {
                    for (sum, &v) in sums.iter_mut().zip(px) {
                        *sum += u64::from(v);
                    }
                }
            }
            let count = ((y1 - y0) * (x1 - x0)) as u64;
            rgba.extend(sums.iter().map(|s| ((s + count / 2) / count) as u8));
        }
    }
    Frame {
        width: dst_w,
        height: dst_h,
        rgba,
    }
}

/// Encodes as 8-bit RGB: screen frames are opaque, so alpha would only add size.
pub fn encode_png(frame: &Frame) -> Result<Vec<u8>, String> {
    let rgb: Vec<u8> = frame
        .rgba
        .as_chunks::<CHANNELS_IN>()
        .0
        .iter()
        .flat_map(|px| px[..CHANNELS_OUT].iter().copied())
        .collect();
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, frame.width, frame.height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|e| format!("png header: {e}"))?;
    writer
        .write_image_data(&rgb)
        .map_err(|e| format!("png data: {e}"))?;
    writer.finish().map_err(|e| format!("png finish: {e}"))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: u32, height: u32, px: [u8; 4]) -> Frame {
        Frame {
            width,
            height,
            rgba: px.repeat((width * height) as usize),
        }
    }

    #[test]
    fn within_limit_is_untouched() {
        let f = downscale(solid(8, 4, [1, 2, 3, 255]), 8);
        assert_eq!((f.width, f.height), (8, 4));
    }

    #[test]
    fn longest_side_is_limited_and_aspect_kept() {
        let f = downscale(solid(100, 50, [9, 9, 9, 255]), 20);
        assert_eq!((f.width, f.height), (20, 10));
        assert_eq!(f.rgba.len(), 20 * 10 * 4);
    }

    #[test]
    fn box_filter_averages_a_checker() {
        // 2x2 black/white checker squeezed to 1x1 averages to mid grey.
        let rgba = vec![
            0, 0, 0, 255, 255, 255, 255, 255, //
            255, 255, 255, 255, 0, 0, 0, 255,
        ];
        let f = downscale(
            Frame {
                width: 2,
                height: 2,
                rgba,
            },
            1,
        );
        assert_eq!((f.width, f.height), (1, 1));
        assert_eq!(f.rgba, vec![128, 128, 128, 255]);
    }

    #[test]
    fn extreme_aspect_never_collapses_to_zero() {
        let f = downscale(solid(1000, 2, [5, 5, 5, 255]), 10);
        assert_eq!((f.width, f.height), (10, 1));
    }

    #[test]
    fn png_round_trips_pixels() {
        let rgba = vec![10, 20, 30, 255, 40, 50, 60, 255];
        let bytes = encode_png(&Frame {
            width: 2,
            height: 1,
            rgba,
        })
        .unwrap();
        let mut reader = png::Decoder::new(std::io::Cursor::new(bytes))
            .read_info()
            .unwrap();
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!((info.width, info.height), (2, 1));
        assert_eq!(&buf[..6], &[10, 20, 30, 40, 50, 60]);
    }
}
