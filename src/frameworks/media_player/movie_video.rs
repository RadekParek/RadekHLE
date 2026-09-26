/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Video output for `MPMoviePlayerController`.
//!
//! Movies are decoded in-process: [super::movie_demux] extracts the H.264
//! track from the MP4/MOV container and `openh264` decodes it to RGBA frames
//! on a dedicated thread, paced to real time. This works on every platform,
//! including Android, where no external `ffmpeg` binary exists.
//!
//! If the container can't be demuxed (e.g. an MPEG-4 Part 2 video track) and
//! an `ffmpeg` binary is available on the host, we fall back to spawning it
//! as a subprocess, like the previous implementation did.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::movie_demux::{avcc_sample_to_annexb, demux_video_track, DemuxedVideoTrack};

/// A running decoder. The decoded frame is flipped vertically (bottom row
/// first), which is the row order the compositor expects for raw pixels.
pub struct MovieVideo {
    /// Native decoder thread handle (`openh264` path).
    decoder_thread: Option<std::thread::JoinHandle<()>>,
    stop_signal: Option<Arc<AtomicBool>>,
    /// Fallback decoder subprocess (`ffmpeg` path, desktop only).
    ffmpeg_child: Option<Child>,
    latest_frame: Arc<Mutex<Option<Vec<u8>>>>,
    pub width: u32,
    pub height: u32,
    /// Real duration of the movie in seconds, when the container provides it.
    pub duration: Option<f64>,
    temp_file: Option<PathBuf>,
}

impl MovieVideo {
    /// Start decoding `movie_bytes` in real time. Returns `None` (after
    /// logging why) if the movie can't be decoded.
    pub fn start(movie_bytes: &[u8], looping: bool) -> Option<MovieVideo> {
        // Native in-process H.264 path first: works everywhere.
        if let Some(track) = demux_video_track(movie_bytes) {
            if let Some(video) = Self::start_native(movie_bytes.to_vec(), track, looping) {
                return Some(video);
            }
        }

        // Fallback: ffmpeg subprocess (desktop convenience for non-H.264
        // codecs like MPEG-4 Part 2).
        Self::start_ffmpeg(movie_bytes, looping)
    }

    fn start_native(
        movie_bytes: Vec<u8>,
        track: DemuxedVideoTrack,
        looping: bool,
    ) -> Option<MovieVideo> {
        let (width, height) = (track.width, track.height);
        let duration = if track.duration_seconds.is_finite() && track.duration_seconds > 0.0 {
            Some(track.duration_seconds)
        } else {
            None
        };

        let latest_frame: Arc<Mutex<Option<Vec<u8>>>> = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));

        let sink = latest_frame.clone();
        let stop_sig = stop.clone();
        let thread = std::thread::Builder::new()
            .name("movie-h264-decoder".to_owned())
            .spawn(move || {
                run_native_decode(movie_bytes, track, looping, sink, stop_sig);
            })
            .ok()?;

        log!(
            "MPMoviePlayerController video: native H.264 decode started ({}x{}, looping: {}, duration: {}s)",
            width,
            height,
            looping,
            duration.map(|d| format!("{d:.1}")).unwrap_or_else(|| "?".to_owned())
        );

        Some(MovieVideo {
            decoder_thread: Some(thread),
            stop_signal: Some(stop),
            ffmpeg_child: None,
            latest_frame,
            width,
            height,
            duration,
            temp_file: None,
        })
    }

    fn start_ffmpeg(movie_bytes: &[u8], looping: bool) -> Option<MovieVideo> {
        static TEMP_COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = TEMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let temp_file = std::env::temp_dir().join(format!(
            "touchHLE_movie_{}_{}.mp4",
            std::process::id(),
            n
        ));
        if let Err(e) = std::fs::write(&temp_file, movie_bytes) {
            log!("MPMoviePlayerController video: couldn't write temp file: {}", e);
            return None;
        }

        let probe = Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=width,height",
                "-of",
                "csv=p=0",
            ])
            .arg(&temp_file)
            .stderr(Stdio::null())
            .output();
        let Some((width, height)) = probe.ok().and_then(|out| {
            let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let (w, h) = text.split_once(',')?;
            Some((w.trim().parse::<u32>().ok()?, h.trim().parse::<u32>().ok()?))
        }) else {
            log_once!(
                "MPMoviePlayerController video: no H.264 track and ffprobe/ffmpeg not found on PATH; movies will not be displayed."
            );
            let _ = std::fs::remove_file(&temp_file);
            return None;
        };

        let mut command = Command::new("ffmpeg");
        command.args(["-loglevel", "error", "-nostdin", "-re"]);
        if looping {
            command.args(["-stream_loop", "-1"]);
        }
        command
            .arg("-i")
            .arg(&temp_file)
            .args(["-an", "-vf", "vflip", "-f", "rawvideo", "-pix_fmt", "rgba", "-"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(e) => {
                log!("MPMoviePlayerController video: couldn't start ffmpeg: {}", e);
                let _ = std::fs::remove_file(&temp_file);
                return None;
            }
        };

        let latest_frame = Arc::new(Mutex::new(None));
        let mut stdout = child.stdout.take().unwrap();
        let frame_size = (width * height * 4) as usize;
        let sink = latest_frame.clone();
        std::thread::spawn(move || {
            let mut buffer = vec![0u8; frame_size];
            while stdout.read_exact(&mut buffer).is_ok() {
                *sink.lock().unwrap() = Some(buffer.clone());
            }
        });

        log!(
            "MPMoviePlayerController video: decoding {}x{} movie with ffmpeg (looping: {})",
            width,
            height,
            looping
        );
        Some(MovieVideo {
            decoder_thread: None,
            stop_signal: None,
            ffmpeg_child: Some(child),
            latest_frame,
            width,
            height,
            duration: None,
            temp_file: Some(temp_file),
        })
    }

    /// Take the most recent frame, if a new one was decoded since last time.
    pub fn take_frame(&self) -> Option<Vec<u8>> {
        self.latest_frame.lock().unwrap().take()
    }
}

impl Drop for MovieVideo {
    fn drop(&mut self) {
        if let Some(stop) = &self.stop_signal {
            stop.store(true, Ordering::Relaxed);
        }
        if let Some(mut child) = self.ffmpeg_child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(temp) = self.temp_file.take() {
            let _ = std::fs::remove_file(temp);
        }
    }
}

/// Real-time paced H.264 decode loop: demuxed samples → Annex-B → openh264 →
/// RGBA (flipped) → shared slot.
fn run_native_decode(
    movie: Vec<u8>,
    track: DemuxedVideoTrack,
    looping: bool,
    sink: Arc<Mutex<Option<Vec<u8>>>>,
    stop: Arc<AtomicBool>,
) {
    use openh264::decoder::Decoder;

    let width = track.width;
    let height = track.height;
    let frame_len = (width as usize) * (height as usize) * 4;
    let mut rgba = vec![0u8; frame_len];
    let mut flipped = vec![0u8; frame_len];
    let stride = width as usize * 4;

    let mut decoder = match Decoder::new() {
        Ok(d) => d,
        Err(e) => {
            log!("MPMoviePlayerController video: openh264 init failed: {}", e);
            return;
        }
    };

    let mut annexb: Vec<u8> = Vec::with_capacity(1 << 20);

    'loops: loop {
        // Fresh decoder per pass so a restarted loop re-learns the stream.
        if let Err(e) = std::mem::replace(
            &mut decoder,
            match Decoder::new() {
                Ok(d) => d,
                Err(_) => break 'loops,
            },
        )
        .flush_remaining()
        {
            log_dbg!("MPMoviePlayerController video: flush failed: {}", e);
        }
        // Feed parameter sets first.
        annexb.clear();
        annexb.extend_from_slice(&track.parameter_sets_annexb);
        if let Err(e) = decoder.decode(&annexb) {
            log_dbg!("MPMoviePlayerController video: SPS/PPS decode error: {}", e);
        }

        let start = Instant::now();
        let mut media_time: f64 = 0.0;

        for sample in &track.samples {
            if stop.load(Ordering::Relaxed) {
                return;
            }

            // Real-time pacing based on per-sample durations.
            let target = Duration::from_secs_f64(media_time);
            let played = start.elapsed();
            if played + Duration::from_millis(2) < target {
                std::thread::sleep(target - played);
            }

            let Some(data) = movie.get(sample.offset as usize..sample.offset as usize + sample.size as usize) else {
                continue;
            };
            annexb.clear();
            avcc_sample_to_annexb(data, track.nal_length_size, &mut annexb);
            if annexb.is_empty() {
                media_time += sample.duration as f64 / track.timescale as f64;
                continue;
            }
            match decoder.decode(&annexb) {
                Ok(Some(yuv)) => {
                    yuv.write_rgba8(&mut rgba);
                    // Flip vertically for the compositor's bottom-first rows.
                    for row in 0..height as usize {
                        let src = (height as usize - 1 - row) * stride;
                        let dst = row * stride;
                        flipped[dst..dst + stride].copy_from_slice(&rgba[src..src + stride]);
                    }
                    *sink.lock().unwrap() = Some(flipped.clone());
                }
                Ok(None) => {}
                Err(e) => {
                    log_dbg!("MPMoviePlayerController video: decode error: {}", e);
                }
            }
            media_time += sample.duration as f64 / track.timescale as f64;
        }

        // Keep the final frame on screen for the tail of the real duration.
        if !looping {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            let target = Duration::from_secs_f64(media_time);
            let played = start.elapsed();
            if played < target {
                std::thread::sleep(target - played);
            }
            return;
        }
    }
}
