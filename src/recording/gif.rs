//! GIF palette generation and recording-option normalization.
// Ported from omnishot/gif_options.py and omnishot/recording.py (RecordSetup.options)

/// The GIF widths the setup dialog offers, in order (0 means original).
pub const WIDTHS: &[u32] = &[0, 400, 600, 800, 960, 1280, 1920];
/// The frame rates the setup dialog offers, in order.
pub const FPS: &[u32] = &[5, 10, 15, 24, 30, 60];
/// The quality choices the setup dialog offers, as `(value, label)`.
pub const QUALITIES: &[(&str, &str)] = &[
    ("medium", "Medium"),
    ("high", "High"),
    ("very_high", "Very high"),
    ("ultra", "Ultra"),
];

/// Turn a filter chain into a two-pass GIF palette chain.
///
/// The colour count scales with the requested quality, and `optimize`
/// selects the rectangle diff mode that only rewrites changed regions.
#[must_use]
pub fn palette_chain(filters: &[String], gif_quality: u32, gif_optimize: bool) -> String {
    let quality = gif_quality.clamp(1, 100) as f64;
    let colors = python_round(16.0 + quality * 2.4).clamp(4, 256);
    let use_filter = if gif_optimize {
        "dither=sierra2_4a:diff_mode=rectangle"
    } else {
        "dither=sierra2_4a"
    };
    format!(
        "{},split[s0][s1];[s0]palettegen=max_colors={colors}[p];[s1][p]paletteuse={use_filter}",
        filters.join(",")
    )
}

/// Python's `round()`: ties go to the even integer.
fn python_round(value: f64) -> u32 {
    value.round_ties_even() as u32
}

/// A GIF take never records audio or scales its resolution.
///
/// The setup dialog applies this after reading the widgets, so a GIF take
/// carries native size and no audio even when the MP4 controls were set.
pub fn normalize_for_gif(options: &mut super::recorder::RecordOptions) {
    options.size = "Native".to_owned();
    options.scale_video = false;
    options.system = false;
    options.microphone = false;
    options.separate_audio = false;
    options.mono = false;
}

/// Apply the GIF profile to `options` when its format is GIF.
pub fn apply_format(options: &mut super::recorder::RecordOptions) {
    if options.format == super::recorder::CaptureFormat::Gif {
        normalize_for_gif(options);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::recorder::{CaptureFormat, RecordOptions};

    #[test]
    fn palette_colour_count_scales_with_quality() {
        let chain = palette_chain(&["fps=15".to_owned()], 80, true);
        assert!(chain.starts_with("fps=15,split[s0][s1]"));
        assert!(chain.contains("palettegen=max_colors=208[p]"));
        assert!(chain.contains("paletteuse=dither=sierra2_4a:diff_mode=rectangle"));
    }

    #[test]
    fn unoptimized_palette_omits_the_rectangle_diff() {
        let chain = palette_chain(&[], 100, false);
        assert!(chain.contains("max_colors=256"));
        assert!(!chain.contains("diff_mode"));
    }

    #[test]
    fn quality_and_width_choices_are_offered_verbatim() {
        assert_eq!(WIDTHS, &[0, 400, 600, 800, 960, 1280, 1920]);
        assert_eq!(FPS, &[5, 10, 15, 24, 30, 60]);
        assert_eq!(QUALITIES[3], ("ultra", "Ultra"));
    }

    #[test]
    fn gif_format_clears_audio_and_scaling() {
        let mut options = RecordOptions {
            format: CaptureFormat::Gif,
            system: true,
            microphone: true,
            separate_audio: true,
            mono: true,
            size: "1920x1080".to_owned(),
            scale_video: true,
            ..RecordOptions::default()
        };
        apply_format(&mut options);
        assert_eq!(options.size, "Native");
        assert!(!options.scale_video);
        assert!(!options.system);
        assert!(!options.microphone);
        assert!(!options.mono);
    }

    #[test]
    fn mp4_format_keeps_its_audio_and_scaling() {
        let mut options = RecordOptions {
            format: CaptureFormat::Mp4,
            system: true,
            size: "1920x1080".to_owned(),
            scale_video: true,
            ..RecordOptions::default()
        };
        apply_format(&mut options);
        assert_eq!(options.size, "1920x1080");
        assert!(options.scale_video);
        assert!(options.system);
    }
}
