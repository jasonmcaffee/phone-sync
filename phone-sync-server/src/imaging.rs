//! Turning a phone's media into JPEGs a browser will actually display.
//!
//! The library is overwhelmingly HEIC and HEVC, neither of which any browser
//! renders and neither of which the pure-Rust `image` crate decodes, so both go
//! through ffmpeg. HEIC in particular needs real work rather than a plain
//! `ffmpeg -i photo.heic out.jpg`:
//!
//! * An iPhone stores the photo as a **tile grid** — a 4032x3024 shot is 48
//!   separate 512x512 HEVC streams — alongside a grayscale HDR gain map and a
//!   washed-out 10-bit HDR preview. ffmpeg's default stream pick lands on the
//!   *gain map*, which produces a black-and-white ghost of the photo. We ask
//!   ffprobe for the primary stream group's exact tile offsets and reassemble it
//!   with `xstack`.
//! * Stitching bypasses ffmpeg's auto-rotation, so the orientation is read
//!   separately out of the file's `irot` property (see [`crate::heif`]) and
//!   applied as a filter. Without it every portrait photo is sideways.
//!
//! Everything is rendered straight to stdout as JPEG, so no temp files are
//! involved and a failed render leaves nothing behind.

use std::path::Path;
use std::process::Command;

use serde::Deserialize;

use crate::orientation::{self, Orientation};

/// Paths to the external tools used to decode media.
#[derive(Debug, Clone)]
pub struct MediaTools {
    /// Path to the `ffmpeg` binary.
    pub ffmpeg: String,
    /// Path to the `ffprobe` binary.
    pub ffprobe: String,
}

/// A stream group as reported by `ffprobe -show_stream_groups`, i.e. one derived
/// image (the photo, the gain map, the HDR rendition) assembled from tiles.
#[derive(Debug, Deserialize)]
struct StreamGroup {
    #[serde(default)]
    disposition: GroupDisposition,
    #[serde(default)]
    components: Vec<GroupComponent>,
}

/// The dispositions we care about; `default` marks the file's primary image.
#[derive(Debug, Default, Deserialize)]
struct GroupDisposition {
    #[serde(default)]
    default: u8,
}

/// The assembled geometry of a tile grid: the visible size plus where each tile
/// belongs within it.
#[derive(Debug, Deserialize)]
struct GroupComponent {
    width: u32,
    height: u32,
    #[serde(default)]
    subcomponents: Vec<Subcomponent>,
}

/// One tile: which stream carries it and where it sits in the grid.
#[derive(Debug, Deserialize)]
struct Subcomponent {
    stream_index: u32,
    #[serde(default)]
    tile_horizontal_offset: u32,
    #[serde(default)]
    tile_vertical_offset: u32,
}

/// Top-level shape of `ffprobe -show_stream_groups -show_streams` output.
#[derive(Debug, Deserialize)]
struct ProbeOutput {
    #[serde(default)]
    stream_groups: Vec<StreamGroup>,
    #[serde(default)]
    streams: Vec<ProbeStream>,
}

/// A plain (non-grouped) stream, used by the fallback path.
#[derive(Debug, Deserialize)]
struct ProbeStream {
    index: u32,
    #[serde(default)]
    codec_type: String,
    #[serde(default)]
    width: u32,
    #[serde(default)]
    height: u32,
    #[serde(default)]
    pix_fmt: String,
}

/// Renders a still image to a JPEG that fits inside `max_dim` on its longest
/// side, correctly oriented. Handles HEIC tile grids, and falls back to the
/// largest genuine (non-grayscale) image stream for anything else ffmpeg reads.
/// Returns None if the file cannot be decoded at all.
/// @param tools - resolved ffmpeg/ffprobe paths
/// @param src - the image file on disk
/// @param max_dim - longest-edge bound for the output
pub fn render_still(tools: &MediaTools, src: &Path, max_dim: u32) -> Option<Vec<u8>> {
    let probe = probe_file(tools, src);
    let orientation = orientation::read(src);

    if let Some(filter) = probe.as_ref().and_then(|p| tile_grid_filter(p, &orientation, max_dim)) {
        if let Some(jpeg) = run_ffmpeg_still(tools, src, FilterPlan::Complex(filter)) {
            return Some(jpeg);
        }
    }

    // Not a tile grid (or the stitch failed): pick the best real image stream
    // ourselves, because ffmpeg's own default would happily choose a gain map.
    let plan = probe
        .as_ref()
        .and_then(best_image_stream)
        .map(|index| FilterPlan::Mapped { index, chain: scale_chain(&orientation, max_dim) })
        .unwrap_or_else(|| FilterPlan::Default { chain: scale_chain(&orientation, max_dim) });
    run_ffmpeg_still(tools, src, plan)
}

/// Renders a poster frame from a video, seeking ~1s in for something more
/// representative than a black first frame and falling back to frame zero for
/// very short clips. ffmpeg applies a video's own rotation matrix, so no
/// orientation handling is needed here.
/// @param tools - resolved ffmpeg/ffprobe paths
/// @param src - the video file on disk
/// @param max_dim - longest-edge bound for the output
pub fn render_video_frame(tools: &MediaTools, src: &Path, max_dim: u32) -> Option<Vec<u8>> {
    let chain = scale_chain(&Orientation::default(), max_dim);
    seek_frame(tools, src, Some("00:00:01"), &chain).or_else(|| seek_frame(tools, src, None, &chain))
}

/// Builds the `[rotation,]scale=...` chain bounding the output to `max_dim`.
/// `force_original_aspect_ratio=decrease` keeps the aspect and never upscales
/// past the source, so a small image stays small rather than being blown up.
/// @param orientation - transform to apply before scaling
/// @param max_dim - longest-edge bound for the output
fn scale_chain(orientation: &Orientation, max_dim: u32) -> String {
    let scale = format!("scale={max_dim}:{max_dim}:force_original_aspect_ratio=decrease");
    match orientation.ffmpeg_filters() {
        Some(filters) => format!("{filters},{scale}"),
        None => scale,
    }
}

/// How the still renderer should point ffmpeg at the right pixels.
enum FilterPlan {
    /// A `-filter_complex` graph producing `[o]` (the stitched tile grid).
    Complex(String),
    /// A single input stream selected by index, plus a simple filter chain.
    Mapped { index: u32, chain: String },
    /// Let ffmpeg choose the stream (last resort), plus a simple filter chain.
    Default { chain: String },
}

/// Builds the `xstack` graph that reassembles the primary tile grid, applies the
/// orientation and scales the result. Returns None when the file has no primary
/// tile grid (a plain single-item HEIC, a JPEG, a PNG).
fn tile_grid_filter(probe: &ProbeOutput, orientation: &Orientation, max_dim: u32) -> Option<String> {
    let group = probe
        .stream_groups
        .iter()
        .find(|g| g.disposition.default == 1)
        .or_else(|| probe.stream_groups.first())?;
    let component = group.components.first()?;
    if component.subcomponents.is_empty() || component.width == 0 || component.height == 0 {
        return None;
    }

    let inputs: String = component
        .subcomponents
        .iter()
        .map(|tile| format!("[0:{}]", tile.stream_index))
        .collect();
    let layout: Vec<String> = component
        .subcomponents
        .iter()
        .map(|tile| format!("{}_{}", tile.tile_horizontal_offset, tile.tile_vertical_offset))
        .collect();

    // The tiles cover a padded canvas (4096x3072 for a 4032x3024 photo), so the
    // stitched result is cropped back to the visible size before scaling.
    Some(format!(
        "{inputs}xstack=inputs={count}:layout={layout}:fill=black[stitched];[stitched]crop={w}:{h}:0:0,{chain}[o]",
        count = component.subcomponents.len(),
        layout = layout.join("|"),
        w = component.width,
        h = component.height,
        chain = scale_chain(orientation, max_dim),
    ))
}

/// Picks the largest non-grayscale image stream, so a fallback render lands on
/// the photo rather than on a depth map or an HDR gain map (both of which iPhone
/// HEICs carry, and both of which are stored as `gray` pixel formats).
fn best_image_stream(probe: &ProbeOutput) -> Option<u32> {
    probe
        .streams
        .iter()
        .filter(|s| s.codec_type == "video" && s.width > 0 && !s.pix_fmt.starts_with("gray"))
        .max_by_key(|s| s.width as u64 * s.height as u64)
        .map(|s| s.index)
}

/// Runs `ffprobe` for the stream groups and streams of a file.
fn probe_file(tools: &MediaTools, src: &Path) -> Option<ProbeOutput> {
    let output = Command::new(&tools.ffprobe)
        .args(["-v", "error", "-show_stream_groups", "-show_streams", "-of", "json"])
        .arg(src)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    serde_json::from_slice(&output.stdout).ok()
}

/// Executes a single-frame ffmpeg render, returning the JPEG from stdout.
fn run_ffmpeg_still(tools: &MediaTools, src: &Path, plan: FilterPlan) -> Option<Vec<u8>> {
    let mut cmd = Command::new(&tools.ffmpeg);
    cmd.args(["-y", "-loglevel", "error"]).arg("-i").arg(src);
    match &plan {
        FilterPlan::Complex(graph) => {
            cmd.arg("-filter_complex").arg(graph).args(["-map", "[o]"]);
        }
        FilterPlan::Mapped { index, chain } => {
            cmd.args(["-map", &format!("0:{index}")]).arg("-vf").arg(chain);
        }
        FilterPlan::Default { chain } => {
            cmd.arg("-vf").arg(chain);
        }
    }
    capture_jpeg(cmd)
}

/// Renders one frame from `seek` (or the start) of a video.
fn seek_frame(tools: &MediaTools, src: &Path, seek: Option<&str>, chain: &str) -> Option<Vec<u8>> {
    let mut cmd = Command::new(&tools.ffmpeg);
    cmd.args(["-y", "-loglevel", "error"]);
    if let Some(position) = seek {
        // Before -i, so this is an input seek: near-instant even on a 5 GB clip.
        cmd.arg("-ss").arg(position);
    }
    cmd.arg("-i").arg(src).args(["-map", "0:v:0"]).arg("-vf").arg(chain);
    capture_jpeg(cmd)
}

/// Finishes an ffmpeg command as a one-frame MJPEG written to stdout and returns
/// the bytes, or None if ffmpeg failed or produced something that is not a JPEG.
fn capture_jpeg(mut cmd: Command) -> Option<Vec<u8>> {
    let output = cmd
        .args(["-frames:v", "1", "-q:v", "3", "-f", "image2pipe", "-vcodec", "mjpeg", "-"])
        .output()
        .ok()?;
    let jpeg = output.stdout;
    // ffmpeg can exit 0 having written nothing when a filter graph degenerates,
    // so the magic bytes are checked rather than trusted.
    (output.status.success() && jpeg.starts_with(&[0xff, 0xd8, 0xff])).then_some(jpeg)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a probe result with one default tile-grid group.
    fn probe_with_grid() -> ProbeOutput {
        serde_json::from_str(
            r#"{"stream_groups":[{"disposition":{"default":1},"components":[{"width":4032,"height":3024,
                "subcomponents":[{"stream_index":0,"tile_horizontal_offset":0,"tile_vertical_offset":0},
                                 {"stream_index":1,"tile_horizontal_offset":512,"tile_vertical_offset":0}]}]}],
                "streams":[]}"#,
        )
        .unwrap()
    }

    #[test]
    fn tile_grid_graph_uses_the_reported_offsets_and_crops_off_the_padding() {
        let graph = tile_grid_filter(&probe_with_grid(), &Orientation::default(), 512).unwrap();
        assert!(graph.contains("[0:0][0:1]xstack=inputs=2:layout=0_0|512_0"));
        assert!(graph.contains("crop=4032:3024:0:0"));
        assert!(graph.ends_with("force_original_aspect_ratio=decrease[o]"));
    }

    #[test]
    fn a_portrait_photo_is_rotated_before_it_is_scaled() {
        let portrait = Orientation { rotation_ccw: 270, mirror: None };
        let graph = tile_grid_filter(&probe_with_grid(), &portrait, 512).unwrap();
        let rotate = graph.find("transpose=1").unwrap();
        let scale = graph.find("scale=512").unwrap();
        assert!(rotate < scale, "rotation must precede scaling: {graph}");
    }

    #[test]
    fn files_without_a_tile_grid_get_no_graph() {
        let probe: ProbeOutput = serde_json::from_str(r#"{"stream_groups":[],"streams":[]}"#).unwrap();
        assert!(tile_grid_filter(&probe, &Orientation::default(), 512).is_none());
    }

    #[test]
    fn the_fallback_stream_pick_skips_grayscale_gain_maps() {
        // The gain map is the largest stream here; the photo must still win.
        let probe: ProbeOutput = serde_json::from_str(
            r#"{"stream_groups":[],"streams":[
                {"index":0,"codec_type":"video","width":4032,"height":3024,"pix_fmt":"gray"},
                {"index":1,"codec_type":"video","width":1024,"height":768,"pix_fmt":"yuvj420p"},
                {"index":2,"codec_type":"audio","width":0,"height":0,"pix_fmt":""}]}"#,
        )
        .unwrap();
        assert_eq!(best_image_stream(&probe), Some(1));
    }
}
