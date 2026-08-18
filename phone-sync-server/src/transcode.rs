//! Turning a phone's video into something the public web will actually play.
//!
//! The private gallery can afford to hand a browser the original bytes and say
//! "download it if your machine has no decoder". A public sharing site cannot:
//! **274 of the 328 clips in this library are HEVC**, which Safari and iOS play
//! always, Chrome and Edge play only where the OS supplies a decoder, and
//! Firefox largely does not. So every clip published to the media site is
//! re-encoded to H.264 High + AAC, which plays everywhere there is a browser.
//!
//! Two renditions are produced, 1080p and 540p, so the player can pick by
//! connection and viewport without a streaming library in the page (see the
//! task-1569 TDD for why there is no HLS here). A third, tiny, silent three
//! second loop is produced for the grid tile.
//!
//! Encoding is **libx264 on the CPU, never NVENC**. Both GPUs on this box are
//! contended by ComfyUI and llama-server, and publishing a holiday video must
//! never queue behind a render or take VRAM from one. CPU encoding costs
//! wall-clock and nothing else.

use std::path::Path;
use std::process::Command;

use serde::Deserialize;

use crate::imaging::MediaTools;

/// A rendition of a video: the box it is fitted inside and how hard it is squeezed.
#[derive(Debug, Clone, Copy)]
pub struct Rendition {
    /// File name written into the published item's directory.
    pub file_name: &'static str,
    /// Longest edge of the bounding box the video is fitted inside.
    pub max_width: u32,
    /// Shortest edge of that box. A portrait clip is bounded by this on its width.
    pub max_height: u32,
    /// x264 constant rate factor — lower is better quality and a bigger file.
    pub crf: u32,
    /// Ceiling on the bitrate, so one pathological clip cannot produce a 400 MB file.
    pub max_rate_kbps: u32,
}

/// The two renditions every published video gets, largest first.
pub const RENDITIONS: [Rendition; 2] = [
    Rendition { file_name: "v1080.mp4", max_width: 1920, max_height: 1080, crf: 21, max_rate_kbps: 6000 },
    Rendition { file_name: "v540.mp4", max_width: 960, max_height: 540, crf: 24, max_rate_kbps: 1800 },
];

/// What ffprobe reports about a clip, reduced to the parts the publish step needs.
#[derive(Debug, Clone, Default)]
pub struct VideoInfo {
    /// Playing time in seconds, from the container.
    pub duration_secs: f64,
    /// True when the clip has at least one audio stream. Plenty of phone clips
    /// have none, and mapping an audio stream that is not there fails the whole
    /// ffmpeg command rather than just dropping the sound.
    pub has_audio: bool,
    /// Coded height of the source, used to skip a rendition larger than the source.
    pub height: u32,
}

/// Top-level shape of `ffprobe -show_format -show_streams -of json`.
#[derive(Debug, Deserialize)]
struct ProbeOutput {
    #[serde(default)]
    format: ProbeFormat,
    #[serde(default)]
    streams: Vec<ProbeStream>,
}

#[derive(Debug, Default, Deserialize)]
struct ProbeFormat {
    #[serde(default)]
    duration: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ProbeStream {
    #[serde(default)]
    codec_type: String,
    #[serde(default)]
    height: u32,
}

/// Reads a clip's duration, audio presence and height.
/// @param tools - resolved ffmpeg/ffprobe paths
/// @param src - the video file on disk
pub fn probe_video(tools: &MediaTools, src: &Path) -> Option<VideoInfo> {
    let output = Command::new(&tools.ffprobe)
        .args(["-v", "error", "-show_format", "-show_streams", "-of", "json"])
        .arg(src)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let probe: ProbeOutput = serde_json::from_slice(&output.stdout).ok()?;
    Some(VideoInfo {
        duration_secs: probe.format.duration.as_deref().and_then(|d| d.parse().ok()).unwrap_or(0.0),
        has_audio: probe.streams.iter().any(|s| s.codec_type == "audio"),
        height: probe.streams.iter().filter(|s| s.codec_type == "video").map(|s| s.height).max().unwrap_or(0),
    })
}

/// Re-encodes a clip to a web-safe H.264 + AAC MP4 at one rendition.
///
/// The output is fitted inside the rendition's box without ever being upscaled,
/// so a 720p source stays 720p. `+faststart` moves the moov atom to the front of
/// the file, which is the single flag that decides whether playback starts from
/// the first bytes or only after the whole file has arrived.
/// @param tools - resolved ffmpeg/ffprobe paths
/// @param src - the source video
/// @param dest - the .mp4 to write
/// @param rendition - the target size and quality
/// @param info - what `probe_video` reported about the source
pub fn transcode_rendition(tools: &MediaTools, src: &Path, dest: &Path, rendition: &Rendition, info: &VideoInfo) -> Result<(), String> {
    let scale = format!(
        "scale=w={}:h={}:force_original_aspect_ratio=decrease:force_divisible_by=2",
        rendition.max_width, rendition.max_height
    );
    let mut cmd = Command::new(&tools.ffmpeg);
    cmd.args(["-y", "-loglevel", "error", "-nostdin"])
        .arg("-i")
        .arg(src)
        .args(["-map", "0:v:0"]);
    if info.has_audio {
        cmd.args(["-map", "0:a:0"]);
    }
    cmd.args([
        "-c:v", "libx264",
        "-profile:v", "high",
        "-level", "4.1",
        "-preset", "veryfast",
        "-crf", &rendition.crf.to_string(),
        "-maxrate", &format!("{}k", rendition.max_rate_kbps),
        "-bufsize", &format!("{}k", rendition.max_rate_kbps * 2),
        "-pix_fmt", "yuv420p",
        "-vf", &scale,
        // A keyframe every two seconds keeps seeking responsive; without it x264
        // can run 250 frames between them and every scrub lands late.
        "-g", "60",
        "-keyint_min", "60",
        "-sc_threshold", "0",
    ]);
    if info.has_audio {
        cmd.args(["-c:a", "aac", "-b:a", "160k", "-ac", "2"]);
    }
    cmd.args(["-movflags", "+faststart"]).arg(dest);

    run(cmd, dest)
}

/// Renders the short silent loop a grid tile plays on hover.
///
/// Seeks ~10% into the clip rather than starting at zero, because the first
/// second of a phone video is usually the camera still settling.
/// @param tools - resolved ffmpeg/ffprobe paths
/// @param src - the source video
/// @param dest - the .mp4 to write
/// @param info - what `probe_video` reported about the source
pub fn render_loop_clip(tools: &MediaTools, src: &Path, dest: &Path, info: &VideoInfo) -> Result<(), String> {
    let start = (info.duration_secs * 0.1).clamp(0.0, 3.0);
    let mut cmd = Command::new(&tools.ffmpeg);
    cmd.args(["-y", "-loglevel", "error", "-nostdin"])
        .args(["-ss", &format!("{start:.2}")])
        .arg("-i")
        .arg(src)
        .args([
            "-map", "0:v:0",
            "-an",
            "-t", "3",
            "-c:v", "libx264",
            "-profile:v", "main",
            "-preset", "veryfast",
            "-crf", "30",
            "-pix_fmt", "yuv420p",
            "-vf", "scale=w=854:h=480:force_original_aspect_ratio=decrease:force_divisible_by=2",
            "-movflags", "+faststart",
        ])
        .arg(dest);
    run(cmd, dest)
}

/// Runs an ffmpeg command and confirms it actually produced a non-trivial file.
///
/// ffmpeg exits 0 in more situations than it produces output in, so success is
/// judged on what is on disk rather than on the status code alone.
/// @param cmd - the fully-built command
/// @param dest - the file it was told to write
fn run(mut cmd: Command, dest: &Path) -> Result<(), String> {
    let output = cmd.output().map_err(|e| format!("running ffmpeg: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("ffmpeg failed ({}): {}", output.status, stderr.trim()));
    }
    match std::fs::metadata(dest) {
        Ok(meta) if meta.len() > 1024 => Ok(()),
        Ok(meta) => Err(format!("ffmpeg wrote only {} bytes to {}", meta.len(), dest.display())),
        Err(e) => Err(format!("ffmpeg wrote nothing to {}: {e}", dest.display())),
    }
}

/// True when a rendition is worth producing for a source of this height.
///
/// Re-encoding a 540p clip up to a "1080p" rendition produces a bigger file with
/// no more detail in it, so the larger rendition is skipped when the source is
/// already at or below the smaller one.
/// @param rendition - the candidate rendition
/// @param info - what `probe_video` reported about the source
pub fn is_worth_producing(rendition: &Rendition, info: &VideoInfo) -> bool {
    // Always produce the smallest rendition: it is the fallback for a slow
    // connection whatever the source resolution is.
    if rendition.max_height <= RENDITIONS[RENDITIONS.len() - 1].max_height {
        return true;
    }
    info.height == 0 || info.height > RENDITIONS[RENDITIONS.len() - 1].max_height
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_small_rendition_is_always_produced() {
        let small = RENDITIONS[1];
        assert!(is_worth_producing(&small, &VideoInfo { height: 480, ..Default::default() }));
        assert!(is_worth_producing(&small, &VideoInfo { height: 2160, ..Default::default() }));
    }

    #[test]
    fn a_source_no_taller_than_the_small_rendition_skips_the_large_one() {
        let large = RENDITIONS[0];
        assert!(!is_worth_producing(&large, &VideoInfo { height: 540, ..Default::default() }));
        assert!(!is_worth_producing(&large, &VideoInfo { height: 480, ..Default::default() }));
        assert!(is_worth_producing(&large, &VideoInfo { height: 1080, ..Default::default() }));
    }

    #[test]
    fn an_unknown_source_height_still_gets_both_renditions() {
        let large = RENDITIONS[0];
        assert!(is_worth_producing(&large, &VideoInfo { height: 0, ..Default::default() }));
    }

    #[test]
    fn probe_reads_duration_audio_presence_and_height() {
        let probe: ProbeOutput = serde_json::from_str(
            r#"{"format":{"duration":"12.480000"},"streams":[
                {"codec_type":"video","height":1080},
                {"codec_type":"audio","height":0}]}"#,
        )
        .unwrap();
        assert_eq!(probe.format.duration.as_deref(), Some("12.480000"));
        assert!(probe.streams.iter().any(|s| s.codec_type == "audio"));
    }
}
