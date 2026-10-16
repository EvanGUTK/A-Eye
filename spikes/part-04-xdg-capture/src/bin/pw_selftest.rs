//! Developer self-test for the PipeWire consumer, WITHOUT the portal and
//! WITHOUT touching the screen. It consumes a synthetic test-pattern node:
//!
//! ```text
//! gst-launch-1.0 videotestsrc pattern=solid-color foreground-color=0xff3366cc is-live=true \
//!   ! video/x-raw,format=BGRx,width=320,height=200 \
//!   ! pipewiresink stream-properties="props,media.class=Video/Source"   # keep running
//! cargo run --manifest-path spikes/part-04-xdg-capture/Cargo.toml --bin pw_selftest -- <node-id>
//! ```
//!
//! The pattern is solid ARGB 0xff3366cc, so every decoded pixel must be
//! RGB (0x33, 0x66, 0xcc). This validates format negotiation, buffer type,
//! stride handling and channel order against a real PipeWire daemon. It is
//! NOT evidence about the portal or about real screen capture.

use std::time::Duration;

use spike_04_xdg_capture::pw;

fn main() {
    let Some(node) = std::env::args().nth(1).and_then(|a| a.parse::<u32>().ok()) else {
        eprintln!("usage: pw_selftest <synthetic-pipewire-node-id>");
        std::process::exit(2);
    };
    println!("pw_selftest: synthetic node {node} (NOT a portal stream, NOT the screen)");
    match pw::capture_synthetic_node(node, Duration::from_secs(10)) {
        Ok(c) => {
            let m = &c.meta;
            println!(
                "format={} {}x{} stride={} chunk_size={} offset={} buffer={} framerate={}/{} modifier={} pts={:?} seq={:?} skipped={} connect_to_frame={:?}",
                m.negotiated_format, m.width, m.height, m.stride, m.chunk_size, m.chunk_offset,
                m.buffer_type, m.framerate.0, m.framerate.1, m.modifier, m.pts_ns, m.seq,
                m.skipped_buffers, m.connect_to_frame
            );
            let f = &c.frame;
            let bpp = f.format.bytes_per_pixel();
            let (ro, go, bo) = f.format.rgb_offsets();
            let mut bad = 0usize;
            for y in 0..f.height as usize {
                for x in 0..f.width as usize {
                    let p = &f.data[y * f.stride + x * bpp..][..bpp];
                    if (p[ro], p[go], p[bo]) != (0x33, 0x66, 0xcc) {
                        bad += 1;
                    }
                }
            }
            println!(
                "pixels checked={} mismatching={bad} (expected RGB 33 66 cc)",
                f.width as usize * f.height as usize
            );
            std::process::exit(if bad == 0 { 0 } else { 1 });
        }
        Err(e) => {
            eprintln!("pw_selftest FAILED: {e}");
            std::process::exit(1);
        }
    }
}
