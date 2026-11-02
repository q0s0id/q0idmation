//! Headless check of native H.264/AAC output. Argument: output.mp4.
#[cfg(windows)]
fn main() -> Result<(), String> {
    use q0video::mp4::{Mp4AudioSpec, Mp4Encoder, Mp4Spec};
    let path = std::env::args_os().nth(1).ok_or("missing output.mp4")?;
    let mut encoder = Mp4Encoder::create_with_audio(
        std::path::Path::new(&path),
        Mp4Spec {
            width: 64,
            height: 64,
            fps: 24,
            bitrate: 500_000,
        },
        Some(Mp4AudioSpec::default()),
    )?;
    for frame in 0..60 {
        encoder.write_rgba_frame(&[255, 0, 0, 255].repeat(64 * 64))?;
        let pcm: Vec<i16> = (frame * 2000..(frame + 1) * 2000)
            .flat_map(|index| {
                let value = ((index as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin()
                    * 12000.0) as i16;
                [value, -value]
            })
            .collect();
        encoder.write_audio_pcm_i16(&pcm)?;
    }
    encoder.finish()
}

#[cfg(not(windows))]
fn main() {
    eprintln!("this example requires windows media foundation");
}
