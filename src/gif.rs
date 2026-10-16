//! GIF palette filter chain shared by recording and export.
// Ported from omnishot/gif_options.py

use std::collections::BTreeMap;

use crate::json::Value;

/// The offered GIF widths; zero means `Native`.
pub const WIDTHS: &[u32] = &[0, 400, 600, 800, 960, 1280, 1920];

/// The offered GIF frame rates.
pub const FPS: &[u32] = &[5, 10, 15, 24, 30, 60];

/// The palette plus dither chain appended to a GIF filter graph.
///
/// `filters` are the pre-palette filters; the returned string is the whole
/// graph, with the palette extraction and use stages the encoder needs.
// Ported from omnishot/gif_options.py (palette_chain)
#[must_use]
pub fn palette_chain(filters: &[String], options: &BTreeMap<String, Value>) -> String {
    let quality = options
        .get("gif_quality")
        .and_then(Value::as_f64)
        .unwrap_or(100.0)
        .clamp(1.0, 100.0) as i64;
    let colors = (16.0 + quality as f64 * 2.4).round().clamp(4.0, 256.0) as i64;
    let optimize = options
        .get("gif_optimize")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let use_clause = if optimize {
        "dither=sierra2_4a:diff_mode=rectangle"
    } else {
        "dither=sierra2_4a"
    };
    format!(
        "{},split[s0][s1];[s0]palettegen=max_colors={colors}[p];[s1][p]paletteuse={use_clause}",
        filters.join(",")
    )
}

/// Resolve the GIF width control: zero becomes `None` for the native width.
#[must_use]
pub fn gif_width(options: &BTreeMap<String, Value>) -> Option<u32> {
    let width = options
        .get("gif_width")
        .and_then(Value::as_f64)
        .unwrap_or(800.0);
    if width <= 0.0 {
        None
    } else {
        Some(width as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(entries: &[(&str, Value)]) -> BTreeMap<String, Value> {
        entries
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect()
    }

    #[test]
    fn chain_carries_filters_and_palette_stages() {
        let chain = palette_chain(
            &["fps=15".to_owned(), "scale=800:-1".to_owned()],
            &options(&[
                ("gif_quality", Value::Number(80.0)),
                ("gif_optimize", Value::Bool(true)),
            ]),
        );
        assert!(chain.starts_with("fps=15,scale=800:-1,split[s0][s1];"));
        assert!(chain.contains("palettegen=max_colors=208[p]"));
        assert!(chain.contains("paletteuse=dither=sierra2_4a:diff_mode=rectangle"));
    }

    #[test]
    fn quality_and_optimize_are_clamped() {
        let low = palette_chain(&[], &options(&[("gif_quality", Value::Number(-5.0))]));
        assert!(low.contains("max_colors=18[p]"));
        let high = palette_chain(&[], &options(&[("gif_quality", Value::Number(500.0))]));
        assert!(high.contains("max_colors=256[p]"));
        let plain = palette_chain(&[], &options(&[("gif_optimize", Value::Bool(false))]));
        assert!(plain.contains("paletteuse=dither=sierra2_4a"));
        assert!(!plain.contains("diff_mode"));
    }

    #[test]
    fn quality_is_truncated_to_an_integer_before_scaling() {
        // 80.9 truncates to 80, so colors = round(16 + 80 * 2.4) = 208.
        let chain = palette_chain(&[], &options(&[("gif_quality", Value::Number(80.9))]));
        assert!(chain.contains("max_colors=208[p]"), "{chain}");
    }

    #[test]
    fn native_width_is_none() {
        assert_eq!(
            gif_width(&options(&[("gif_width", Value::Number(0.0))])),
            None
        );
        assert_eq!(
            gif_width(&options(&[("gif_width", Value::Number(960.0))])),
            Some(960)
        );
    }
}
