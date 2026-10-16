//! Source-frame clip boundaries, independent of playback navigation.
// Ported from omnishot/video_splits.py
//!
//! A split is a frame-aligned source instant that never changes what plays
//! back: it only groups the retained material into clips for per-clip edits.
//! Trimming a clip rewrites the baseline cuts as the union or difference of a
//! gesture, using the cuts from the gesture's start so reversing a drag
//! restores every frame.

use crate::json::Value;

use super::studio::kept_segments;

/// Normalize split values: finite numbers strictly inside a day, deduplicated
/// and sorted. Booleans and non-numeric values are dropped.
// Ported from omnishot/video_splits.py (clean_splits)
#[must_use]
pub fn clean_splits(values: &[Value]) -> Vec<f64> {
    let mut out = Vec::new();
    for value in values {
        let Value::Number(number) = value else {
            continue;
        };
        if number.is_finite() && *number > 0.0 && *number < 86400.0 {
            out.push(*number);
        }
    }
    out.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    out.dedup();
    out
}

/// Split the retained source intervals at each split boundary.
///
/// Returns `(start, end)` pairs in source order; the cuts come from the
/// recording's `kept_segments`.
// Ported from omnishot/video_splits.py (clips)
#[must_use]
pub fn clips(start: f64, end: f64, cuts: &[(f64, f64)], splits: &[f64]) -> Vec<(f64, f64)> {
    let mut result = Vec::new();
    for (a, b) in kept_segments(start, end, cuts) {
        let mut edges = vec![a];
        edges.extend(splits.iter().copied().filter(|s| a < *s && *s < b));
        edges.push(b);
        for pair in edges.windows(2) {
            result.push((pair[0], pair[1]));
        }
    }
    result
}

/// Resize one retained source interval without consuming a neighboring clip.
///
/// `bounds` is the clip's current `(a, b)`, `edge` is `"start"` or `"end"`,
/// and `value` is the gesture's new instant in seconds. Returns the new cut
/// list and the clip's new bounds.
// Ported from omnishot/video_splits.py (trim_clip)
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn trim_clip(
    start: f64,
    end: f64,
    cuts: &[(f64, f64)],
    splits: &[f64],
    bounds: (f64, f64),
    edge: &str,
    value: f64,
    fps: f64,
) -> (Vec<(f64, f64)>, (f64, f64)) {
    let (a, b) = bounds;
    let frames = if fps > 0.0 { fps } else { 30.0 };
    let frame = 1.0 / frames;
    let neighbors = clips(start, end, cuts, splits);

    let mut lo = start;
    for s in splits.iter().copied().filter(|s| *s <= a) {
        lo = lo.max(s);
    }
    for (_, y) in neighbors.iter().copied().filter(|(_, y)| *y <= a) {
        lo = lo.max(y);
    }
    let mut hi = end;
    for s in splits.iter().copied().filter(|s| *s >= b) {
        hi = hi.min(s);
    }
    for (x, _) in neighbors.iter().copied().filter(|(x, _)| *x >= b) {
        hi = hi.min(x);
    }

    let value = (value * frames).round() / frames;
    let (new_a, new_b) = if edge == "start" {
        (lo.max((b - frame).min(value)), b)
    } else {
        (a, hi.min((a + frame).max(value)))
    };
    if (new_a - a).abs() < 1e-9 && (new_b - b).abs() < 1e-9 {
        return (cuts.to_vec(), (a, b));
    }

    // Union an excluded part, or subtract a restored part, from the baseline.
    let removed = if edge == "start" {
        (a, new_a)
    } else {
        (new_b, b)
    };
    let restored = if edge == "start" {
        (new_a, a)
    } else {
        (b, new_b)
    };
    let (u, v) = restored;
    let mut result: Vec<(f64, f64)> = Vec::new();
    for (x, y) in cuts.iter().copied() {
        if v <= u || y <= u || x >= v {
            result.push((x, y));
        } else {
            if x < u {
                result.push((x, u));
            }
            if y > v {
                result.push((v, y));
            }
        }
    }
    if removed.1 > removed.0 {
        result.push(removed);
    }
    result.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mut merged: Vec<(f64, f64)> = Vec::new();
    for (x, y) in result {
        if let Some(last) = merged.last_mut()
            && x <= last.1 + 1e-9
        {
            last.1 = y.max(last.1);
            continue;
        }
        merged.push((x, y));
    }
    (merged, (new_a, new_b))
}

/// Validate a splits list the way the video project manifest does.
pub fn validate_splits(values: &[Value]) -> Result<(), String> {
    for value in values {
        let valid = matches!(value, Value::Number(n) if n.is_finite() && *n > 0.0 && *n < 86400.0);
        if !valid {
            return Err("Invalid clip boundary".to_owned());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn numbers(values: &[f64]) -> Vec<Value> {
        values.iter().map(|v| Value::Number(*v)).collect()
    }

    #[test]
    fn split_normalization_drops_booleans_duplicates_and_out_of_range() {
        let input = vec![
            Value::Number(3.0),
            Value::Number(2.0),
            Value::Number(2.0),
            Value::Bool(true),
            Value::String("2".to_owned()),
            Value::Number(f64::NAN),
            Value::Number(f64::INFINITY),
            Value::Number(-1.0),
            Value::Number(90000.0),
        ];
        assert_eq!(clean_splits(&input), vec![2.0, 3.0]);
        assert_eq!(clean_splits(&[]), Vec::<f64>::new());
    }

    #[test]
    fn clips_split_retained_intervals_at_boundaries() {
        let cuts = [(2.0, 4.0), (3.0, 5.0), (8.0, 10.0)];
        let splits = [3.0, 6.0, 7.0];
        assert_eq!(
            clips(1.0, 9.0, &cuts, &splits),
            vec![(1.0, 2.0), (5.0, 6.0), (6.0, 7.0), (7.0, 8.0)]
        );
    }

    #[test]
    fn trim_restores_and_intersects_neighboring_clips() {
        // A single retained interval split into three clips.
        let splits = [4.0, 8.0];
        let (cuts, bounds) = trim_clip(0.0, 12.0, &[], &splits, (4.0, 8.0), "start", 5.0, 30.0);
        assert_eq!(bounds, (5.0, 8.0));
        assert_eq!(cuts, vec![(4.0, 5.0)]);
        let (restored, bounds) =
            trim_clip(0.0, 12.0, &cuts, &splits, (5.0, 8.0), "start", 4.0, 30.0);
        assert_eq!(bounds, (4.0, 8.0));
        assert!(restored.is_empty());

        // A clip's start cannot cross the previous retained edge.
        let (_, bounds) = trim_clip(0.0, 12.0, &[], &splits, (4.0, 8.0), "start", 0.0, 30.0);
        assert!(bounds.0 >= 4.0 - 1e-9);
        // A gap inside the region can be restored to the next kept edge.
        let (cuts, bounds) = trim_clip(
            0.0,
            12.0,
            &[(4.0, 5.0)],
            &splits,
            (5.0, 8.0),
            "start",
            4.0,
            30.0,
        );
        assert_eq!(bounds, (4.0, 8.0));
        assert!(cuts.is_empty());
    }

    #[test]
    fn trim_normalizes_to_the_frame_grid_and_no_ops_at_its_edge() {
        let (cuts, bounds) = trim_clip(0.0, 12.0, &[], &[4.0, 8.0], (4.0, 8.0), "end", 8.0, 30.0);
        assert_eq!(bounds, (4.0, 8.0));
        assert!(cuts.is_empty());
        let (_, bounds) = trim_clip(0.0, 12.0, &[], &[4.0, 8.0], (4.0, 8.0), "end", 9.019, 30.0);
        assert!((bounds.1 * 30.0).fract().abs() < 1e-9);
    }

    #[test]
    fn manifest_split_validation_rejects_booleans_and_out_of_range() {
        assert!(validate_splits(&numbers(&[2.0, 3.0])).is_ok());
        for invalid in [
            Value::Bool(true),
            Value::Number(f64::NAN),
            Value::Number(-1.0),
            Value::Number(90000.0),
            Value::String("3".to_owned()),
        ] {
            assert!(validate_splits(&[invalid]).is_err());
        }
        assert!(validate_splits(&[]).is_ok());
    }
}
