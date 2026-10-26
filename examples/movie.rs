//! Decode an MPEG-1 movie with `fcop::mpeg`: `movie <file.mpg> [out_dir]` prints its streams
//! and the decoding speed; with out_dir, writes frames.rgb (all frames, 0RGB words as R G B
//! bytes) and audio.raw (interleaved stereo f32) there, for comparison with ffmpeg.

use fcop::mpeg;
use std::io::Write;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let data = std::fs::read(&args[1]).expect("cannot read the movie");
    let out = args.get(2);
    let t = Instant::now();
    let s = mpeg::demux(&data);
    println!("video {} bytes (start {:?}), audio {} bytes (start {:?})", s.video.len(), s.video_start, s.audio.len(), s.audio_start);

    let mut v = mpeg::video::Decoder::new(&s.video).expect("no video sequence header");
    println!("{}x{} at {} fps", v.width, v.height, v.fps);
    let mut rgb = Vec::new();
    let mut frames = out.map(|d| std::io::BufWriter::new(std::fs::File::create(format!("{d}/frames.rgb")).unwrap()));
    let mut n = 0;
    while let Some(f) = v.next_frame() {
        f.to_rgb(&mut rgb);
        if let Some(w) = &mut frames {
            for p in &rgb {
                w.write_all(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8]).unwrap();
            }
        }
        n += 1;
    }
    println!("{n} frames ({:.2} s)", n as f64 / v.fps);

    if let Some(mut a) = mpeg::mp2::Decoder::new(&s.audio) {
        let mut raw = out.map(|d| std::io::BufWriter::new(std::fs::File::create(format!("{d}/audio.raw")).unwrap()));
        let mut samples = 0;
        while let Some(frame) = a.next_frame() {
            if let Some(w) = &mut raw {
                for s in frame {
                    w.write_all(&s[0].to_le_bytes()).unwrap();
                    w.write_all(&s[1].to_le_bytes()).unwrap();
                }
            }
            samples += frame.len();
        }
        println!("audio {} Hz, {samples} samples ({:.2} s)", a.sample_rate, samples as f64 / a.sample_rate as f64);
    }
    println!("decoded in {:.2} s", t.elapsed().as_secs_f64());
}
