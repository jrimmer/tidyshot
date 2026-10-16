//! Zoomable, scrolling source-time tracks shared by preview and export.
// Ported from omnishot/timeline.py and omnishot/timeline_zooms.py
//!
//! The model owns the source clock, the split boundaries, the cuts and the
//! zooms. Geometry (view offset, visible span, pixels per second) is pure so
//! the same model backs painting, hit testing and tests without a Qt widget.
//! Split boundaries and clip trims are persisted through the video project
//! manifest; the model only ever produces the values that are written.

use crate::json::Value;

use crate::video_motion::Zoom;
use crate::video_splits::{clips as split_clips, trim_clip};

/// What the user has selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    Zoom(usize),
    Cut(usize),
    Clip(usize),
    Trim,
}

/// The horizontal inset of the source-time plot.
pub const PLOT_MARGIN: f64 = 16.0;

/// A source-time timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct Timeline {
    pub duration: f64,
    pub position: f64,
    pub start: f64,
    pub end: f64,
    pub fps: f64,
    pub zooms: Vec<Zoom>,
    pub cuts: Vec<(f64, f64)>,
    pub splits: Vec<f64>,
    pub cut_mode: bool,
    pub selected: Option<Selection>,
    pub zoom_level: i32,
    /// Horizontal view offset in seconds.
    pub scroll: f64,
    /// The widget width used for the geometry.
    pub width: f64,
    /// The normalized source image ratio, for thumbnail sizing.
    pub source_aspect: f64,
}

impl Default for Timeline {
    fn default() -> Self {
        Self {
            duration: 0.0,
            position: 0.0,
            start: 0.0,
            end: 0.0,
            fps: 30.0,
            zooms: Vec::new(),
            cuts: Vec::new(),
            splits: Vec::new(),
            cut_mode: false,
            selected: None,
            zoom_level: 0,
            scroll: 0.0,
            width: 900.0,
            source_aspect: 16.0 / 9.0,
        }
    }
}

impl Timeline {
    /// Set the duration from a player's millisecond position.
    pub fn set_duration(&mut self, milliseconds: f64) {
        self.duration = (milliseconds / 1000.0).max(0.0);
    }

    /// Set the frame rate when the player reports a positive one.
    pub fn set_frame_rate(&mut self, fps: f64) {
        if fps.is_finite() && fps > 0.0 {
            self.fps = fps;
        }
    }

    /// Set the playhead from a player's millisecond position.
    pub fn set_position(&mut self, milliseconds: f64) {
        self.position = milliseconds / 1000.0;
    }

    /// The rear split boundary or the duration, whichever bounds the clips.
    #[must_use]
    pub fn clip_end(&self) -> f64 {
        if self.end > 0.0 {
            self.end
        } else {
            self.duration
        }
    }

    /// The retained source intervals after cuts and splits.
    #[must_use]
    pub fn clips(&self) -> Vec<(f64, f64)> {
        split_clips(self.start, self.clip_end(), &self.cuts, &self.splits)
    }

    /// Select an item, clearing the selection with `None`.
    pub fn select(&mut self, selection: Option<Selection>) {
        self.selected = selection;
    }

    /// Toggle the cut tool.
    pub fn set_cut_mode(&mut self, enabled: bool) {
        self.cut_mode = enabled;
    }

    /// Split at `seconds`, or at the playhead when `None`.
    ///
    /// A split must leave at least one frame on each side of a clip; a
    /// candidate that falls in a cut or at a clip edge is rejected.
    // Ported from omnishot/timeline.py (split_at)
    pub fn split_at(&mut self, seconds: Option<f64>) -> bool {
        let candidate = ((seconds.unwrap_or(self.position)) * self.fps).round() / self.fps;
        let minimum = 1.0 / self.fps - 1e-7;
        let fits = self
            .clips()
            .iter()
            .any(|(a, b)| candidate - a >= minimum && b - candidate >= minimum);
        if !fits {
            return false;
        }
        self.splits.push(candidate);
        self.splits
            .sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        self.select(None);
        true
    }

    /// Delete the current selection, restoring a cut when a clip is selected.
    // Ported from omnishot/timeline.py (delete_selected)
    pub fn delete_selected(&mut self) -> bool {
        let Some(selected) = self.selected else {
            return false;
        };
        match selected {
            Selection::Zoom(index) => {
                if index >= self.zooms.len() {
                    return false;
                }
                self.zooms.remove(index);
            }
            Selection::Cut(index) => {
                if index >= self.cuts.len() {
                    return false;
                }
                self.cuts.remove(index);
            }
            Selection::Clip(index) => {
                let clips = self.clips();
                let Some(bounds) = clips.get(index) else {
                    return false;
                };
                self.cuts.push(*bounds);
                self.cuts
                    .sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            }
            Selection::Trim => return false,
        }
        self.select(None);
        true
    }

    /// Apply a clip-edge drag, returning the new bounds.
    ///
    /// The baseline cuts are the ones captured at gesture start, so reversing
    /// a drag restores every frame.
    // Ported from omnishot/timeline.py (apply_drag clip branch)
    pub fn trim_clip(
        &mut self,
        bounds: (f64, f64),
        edge: &str,
        value: f64,
        baseline: &[(f64, f64)],
    ) -> (f64, f64) {
        let (cuts, new_bounds) = trim_clip(
            self.start,
            self.clip_end(),
            baseline,
            &self.splits,
            bounds,
            edge,
            value,
            self.fps,
        );
        self.cuts = cuts;
        let anchor = (new_bounds.0 + new_bounds.1) / 2.0;
        let clips = self.clips();
        self.select(
            clips
                .iter()
                .position(|(x, y)| *x <= anchor && anchor < *y)
                .map(Selection::Clip),
        );
        new_bounds
    }

    /// The gap around `seconds` where a new zoom can be added.
    // Ported from omnishot/timeline_zooms.py (zoom_gap)
    #[must_use]
    pub fn zoom_gap(&self, seconds: f64) -> Option<(f64, f64)> {
        let mut lo = self.start;
        let mut hi = self.end.max(self.duration);
        if self.end <= 0.0 {
            hi = self.duration;
        }
        if !(lo <= seconds && seconds < hi) {
            return None;
        }
        for zoom in &self.zooms {
            if zoom.start <= seconds && seconds < zoom.end {
                return None;
            }
            if zoom.end <= seconds {
                lo = lo.max(zoom.end);
            }
            if zoom.start > seconds {
                hi = hi.min(zoom.start);
            }
        }
        ((hi - lo) >= 1.0 / self.fps).then_some((lo, hi))
    }

    /// The limits a zoom can be resized to without overlapping its neighbors.
    // Ported from omnishot/timeline_zooms.py (zoom_limits)
    #[must_use]
    pub fn zoom_limits(&self, index: usize) -> (f64, f64) {
        let Some(zoom) = self.zooms.get(index) else {
            return (0.0, self.duration);
        };
        let mut lo = 0.0_f64;
        let mut hi = self.duration;
        for (other_index, other) in self.zooms.iter().enumerate() {
            if other_index == index {
                continue;
            }
            if other.end <= zoom.start + 1e-7 {
                lo = lo.max(other.end);
            }
            if other.start >= zoom.end - 1e-7 {
                hi = hi.min(other.start);
            }
        }
        (lo, hi)
    }

    /// Add a zoom at `seconds`, returning its index.
    // Ported from omnishot/timeline_zooms.py (add_zoom_at)
    pub fn add_zoom_at(&mut self, seconds: f64) -> Option<usize> {
        let (lo, hi) = self.zoom_gap(seconds)?;
        let minimum = 1.0 / self.fps;
        let a = lo
            .max((seconds * self.fps).round() / self.fps)
            .min(hi - minimum);
        let b = hi.min(a + 3.0);
        self.zooms.push(Zoom {
            start: a,
            end: b,
            scale: 1.5,
            x: 0.5,
            y: 0.5,
            follow: true,
        });
        let index = self.zooms.len() - 1;
        self.select(Some(Selection::Zoom(index)));
        Some(index)
    }

    /// The selected zoom, if a zoom is selected.
    #[must_use]
    pub fn selected_zoom(&self) -> Option<&Zoom> {
        match self.selected {
            Some(Selection::Zoom(index)) => self.zooms.get(index),
            _ => None,
        }
    }

    /// Apply a scale change to every zoom.
    // Ported from omnishot/timeline_zooms.py (apply_zoom_level_to_all)
    pub fn apply_zoom_level_to_all(&mut self) -> bool {
        let Some(scale) = self.selected_zoom().map(|zoom| zoom.scale) else {
            return false;
        };
        for zoom in &mut self.zooms {
            zoom.scale = scale;
        }
        true
    }

    /// Split the zoom under the playhead, returning `true` when split.
    // Ported from omnishot/timeline_zooms.py (split_zoom_at_playhead)
    pub fn split_zoom_at_playhead(&mut self) -> bool {
        let t = (self.position * self.fps).round() / self.fps;
        let minimum = 1.0 / self.fps - 1e-7;
        let fits = |zoom: &Zoom| zoom.start + minimum <= t && t <= zoom.end - minimum;
        let index = match self.selected {
            Some(Selection::Zoom(index)) if self.zooms.get(index).is_some_and(fits) => Some(index),
            _ => self.zooms.iter().position(fits),
        };
        let Some(index) = index else {
            return false;
        };
        let mut duplicate = self.zooms[index].clone();
        duplicate.start = t;
        self.zooms[index].end = t;
        self.zooms.push(duplicate);
        self.select(Some(Selection::Zoom(self.zooms.len() - 1)));
        true
    }

    /// Duplicate the selected zoom into the next gap that fits it.
    // Ported from omnishot/timeline_zooms.py (duplicate_zoom)
    pub fn duplicate_zoom(&mut self) -> bool {
        let Some(zoom) = self.selected_zoom().cloned() else {
            return false;
        };
        let length = zoom.end - zoom.start;
        let mut edges = vec![zoom.end, self.start];
        edges.extend(self.zooms.iter().map(|z| z.end));
        for start in edges {
            if let Some((lo, hi)) = self.zoom_gap(start)
                && start >= lo
                && hi - start >= length - 1e-7
            {
                let mut duplicate = zoom.clone();
                duplicate.start = start;
                duplicate.end = start + length;
                self.zooms.push(duplicate);
                self.select(Some(Selection::Zoom(self.zooms.len() - 1)));
                return true;
            }
        }
        false
    }

    /// The maximum zoom factor derived from the recording's frame count.
    // Ported from omnishot/timeline.py (max_zoom)
    #[must_use]
    pub fn max_zoom(&self) -> f64 {
        (self.duration * self.fps * 8.0 / self.plot_width()).max(1.0)
    }

    /// The zoom factor at the current zoom level.
    #[must_use]
    pub fn factor(&self) -> f64 {
        self.max_zoom().powf(f64::from(self.zoom_level) / 100.0)
    }

    /// The source seconds visible in the plot.
    #[must_use]
    pub fn visible_span(&self) -> f64 {
        (self.duration / self.factor()).max(0.001)
    }

    /// The plot's pixel width.
    #[must_use]
    pub fn plot_width(&self) -> f64 {
        (self.width - PLOT_MARGIN * 2.0).max(1.0)
    }

    /// The plot's pixels per source second.
    #[must_use]
    pub fn pixels_per_second(&self) -> f64 {
        self.plot_width() / self.visible_span()
    }

    /// The widget x for a source instant.
    #[must_use]
    pub fn x(&self, seconds: f64) -> f64 {
        PLOT_MARGIN + (seconds - self.scroll) * self.pixels_per_second()
    }

    /// The source instant at a widget x, clamped to the duration.
    #[must_use]
    pub fn time_at(&self, x: f64) -> f64 {
        let seconds = self.scroll + (x - PLOT_MARGIN) / self.pixels_per_second();
        seconds.clamp(0.0, self.duration)
    }

    /// Change the zoom level, keeping `anchor` at the same source instant.
    // Ported from omnishot/timeline.py (set_zoom)
    pub fn set_zoom(&mut self, value: i32, anchor: Option<f64>) {
        let value = value.clamp(0, 100);
        if value == self.zoom_level {
            return;
        }
        let anchor = anchor.unwrap_or_else(|| {
            let position_x = self.x(self.position);
            if self.scroll <= self.position && self.position <= self.scroll + self.visible_span() {
                position_x
            } else {
                PLOT_MARGIN + self.plot_width() / 2.0
            }
        });
        let before = self.time_at(anchor);
        self.zoom_level = value;
        self.scroll =
            (before - (anchor - PLOT_MARGIN) / self.pixels_per_second()).clamp(0.0 + 0.0, f64::MAX);
    }

    /// The thumbnail tiles visible at the current zoom.
    ///
    /// Groups shorter than 48 px are merged, and the tile count is bounded by
    /// the visible span rather than the split count.
    // Ported from omnishot/timeline.py (thumbnail_tiles)
    #[must_use]
    pub fn thumbnail_tiles(&self) -> Vec<(u64, f64, f64)> {
        if self.duration <= 0.0 {
            return Vec::new();
        }
        let aspect = if self.source_aspect > 0.0 {
            self.source_aspect
        } else {
            16.0 / 9.0
        };
        let width = (48.0_f64).max((150.0_f64).min(48.0 * aspect));
        let frames = (width / self.pixels_per_second() * self.fps)
            .ceil()
            .max(1.0);
        let step = frames / self.fps;
        let mut edges = vec![0.0];
        edges.extend(
            self.splits
                .iter()
                .copied()
                .filter(|s| *s > 0.0 && *s < self.duration),
        );
        edges.push(self.duration);
        let left = self.scroll;
        let right = left + self.visible_span();
        let mut groups: Vec<(f64, f64, bool)> = Vec::new();
        for pair in edges.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let small = (b - a) * self.pixels_per_second() < 48.0;
            if small
                && let Some(last) = groups.last_mut()
                && last.2
            {
                last.1 = b;
                continue;
            }
            groups.push((a, b, small));
        }
        let mut tiles = Vec::new();
        for (a, b, _) in groups {
            if b < left || a > right {
                continue;
            }
            let first = ((left - a) / step).floor().max(0.0) as u64;
            let last = (((b - a) / step).ceil()).min(((right - a) / step).ceil() + 1.0);
            let last = last.max(0.0) as u64;
            for index in first..last {
                let start = a + index as f64 * step;
                tiles.push((
                    (start * 1000.0).round() as u64,
                    start,
                    b.min(a + (index as f64 + 1.0) * step),
                ));
            }
        }
        tiles
    }

    /// Persist the model's editable state into an options object.
    // Ported from omnishot/recording.py (VideoEditor.edit_options)
    pub fn write_options(&self, options: &mut std::collections::BTreeMap<String, Value>) {
        options.insert("start".to_owned(), Value::Number(self.start));
        options.insert("end".to_owned(), Value::Number(self.end));
        options.insert(
            "splits".to_owned(),
            Value::Array(self.splits.iter().map(|s| Value::Number(*s)).collect()),
        );
        options.insert(
            "cuts".to_owned(),
            Value::Array(
                self.cuts
                    .iter()
                    .map(|(a, b)| Value::Array(vec![Value::Number(*a), Value::Number(*b)]))
                    .collect(),
            ),
        );
        options.insert(
            "zooms".to_owned(),
            Value::Array(self.zooms.iter().map(zoom_json).collect()),
        );
    }

    /// Restore the model's editable state from an options object.
    // Ported from omnishot/video_history.py (restore_options)
    pub fn read_options(&mut self, options: &std::collections::BTreeMap<String, Value>) {
        if let Some(value) = options.get("start").and_then(Value::as_f64) {
            self.start = value;
        }
        if let Some(value) = options.get("end").and_then(Value::as_f64) {
            self.end = value;
        }
        self.splits = options
            .get("splits")
            .and_then(Value::as_array)
            .map(crate::video_splits::clean_splits)
            .unwrap_or_default();
        self.cuts = options
            .get("cuts")
            .and_then(Value::as_array)
            .map(|rows| {
                rows.iter()
                    .filter_map(|row| {
                        let values = row.as_array()?;
                        Some((values.first()?.as_f64()?, values.get(1)?.as_f64()?))
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.zooms = Zoom::list_from_json(options.get("zooms"));
    }
}

fn zoom_json(zoom: &Zoom) -> Value {
    Value::Object(std::collections::BTreeMap::from([
        ("start".to_owned(), Value::Number(zoom.start)),
        ("end".to_owned(), Value::Number(zoom.end)),
        ("scale".to_owned(), Value::Number(zoom.scale)),
        ("x".to_owned(), Value::Number(zoom.x)),
        ("y".to_owned(), Value::Number(zoom.y)),
        ("follow".to_owned(), Value::Bool(zoom.follow)),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timeline(duration_ms: f64, fps: f64) -> Timeline {
        let mut timeline = Timeline::default();
        timeline.set_duration(duration_ms);
        timeline.set_frame_rate(fps);
        timeline
    }

    #[test]
    fn split_frame_boundaries_and_clip_delete_restore() {
        // Ported from omnishot/test_video_splits.py
        //   (test_split_frame_boundaries_and_clip_delete_restore)
        let mut t = timeline(12000.0, 15.0);
        assert!(!t.split_at(Some(0.0)));
        assert!(!t.split_at(Some(12.0)));
        assert!(t.split_at(Some(4.019)));
        assert_eq!(t.splits, vec![4.0]);
        assert!(!t.split_at(Some(4.01)));

        t.set_position(8000.0);
        assert!(t.split_at(None));
        assert_eq!(t.splits, vec![4.0, 8.0]);
        assert_eq!(t.clips(), vec![(0.0, 4.0), (4.0, 8.0), (8.0, 12.0)]);

        t.select(Some(Selection::Clip(1)));
        assert!(t.delete_selected());
        assert_eq!(t.cuts, vec![(4.0, 8.0)]);
        assert_eq!(t.clips(), vec![(0.0, 4.0), (8.0, 12.0)]);
        assert!(!t.split_at(Some(6.0)));

        t.select(Some(Selection::Cut(0)));
        assert!(t.delete_selected());
        assert!(t.cuts.is_empty());

        t.set_cut_mode(true);
        assert!(t.cut_mode);
        assert!(t.split_at(Some(10.0)));
        assert!((t.splits.last().copied().unwrap() - 10.0).abs() < 1.0 / 15.0);
        t.set_cut_mode(false);

        t.start = 1.0;
        t.end = 11.0;
        t.cuts = vec![(5.0, 6.0)];
        assert!(!t.split_at(Some(0.5)));
        assert!(!t.split_at(Some(11.5)));
        assert!(t.split_at(Some(2.0)));
        assert!(t.clips().iter().all(|(a, b)| *a >= 1.0 && *b <= 11.0));

        t.set_duration(30000.0);
        t.splits = (1..450).map(|i| f64::from(i) / 15.0).collect();
        assert!(t.thumbnail_tiles().len() < 30);
    }

    #[test]
    fn zoom_actions_add_split_duplicate_and_level_all() {
        let mut t = timeline(18000.0, 30.0);
        assert_eq!(t.add_zoom_at(2.0), Some(0));
        assert_eq!(t.zooms.len(), 1);
        assert_eq!(t.selected, Some(Selection::Zoom(0)));
        assert_eq!(t.zooms[0].scale, 1.5);
        assert!(t.zooms[0].follow);

        assert_eq!(t.add_zoom_at(8.0), Some(1));
        assert!((t.zooms[1].end - 11.0).abs() < 1e-9);
        assert!(t.zooms.iter().all(
            |z| (z.start * t.fps).fract().abs() < 1e-6 && (z.end * t.fps).fract().abs() < 1e-6
        ));

        t.zooms[1].scale = 2.3;
        assert!(t.apply_zoom_level_to_all());
        assert!(t.zooms.iter().all(|z| (z.scale - 2.3).abs() < 1e-9));

        t.set_position((t.zooms[1].start + 1.0) * 1000.0);
        assert!(t.split_zoom_at_playhead());
        assert_eq!(t.zooms.len(), 3);
        assert!((t.zooms[1].end - t.zooms[2].start).abs() < 1e-9);
        assert!(!t.split_zoom_at_playhead());

        assert!(t.duplicate_zoom());
        assert_eq!(t.zooms.len(), 4);
        assert!(t.delete_selected());
        assert_eq!(t.zooms.len(), 3);
    }

    #[test]
    fn zoom_gap_and_limits_stop_a_zoom_from_overlapping_its_neighbor() {
        let mut t = timeline(18000.0, 30.0);
        t.zooms = vec![
            Zoom {
                start: 2.0,
                end: 4.0,
                scale: 2.0,
                x: 0.5,
                y: 0.5,
                follow: false,
            },
            Zoom {
                start: 6.0,
                end: 8.0,
                scale: 2.0,
                x: 0.5,
                y: 0.5,
                follow: false,
            },
        ];
        assert_eq!(t.zoom_gap(3.0), None);
        assert_eq!(t.zoom_gap(5.0), Some((4.0, 6.0)));
        assert_eq!(t.zoom_limits(0), (0.0, 6.0));
        assert_eq!(t.zoom_limits(1), (4.0, 18.0));
    }

    #[test]
    fn clip_trim_restores_frames_across_a_drag_reversal() {
        let mut t = timeline(12000.0, 30.0);
        t.splits = vec![4.0, 8.0];
        let baseline = t.cuts.clone();
        // Exclude the first second of the middle clip.
        let (a, b) = t.trim_clip((4.0, 8.0), "start", 5.0, &baseline);
        assert_eq!(t.cuts, vec![(4.0, 5.0)]);
        assert!((a - 5.0).abs() < 1e-9 && (b - 8.0).abs() < 1e-9);
        assert_eq!(t.clips(), vec![(0.0, 4.0), (5.0, 8.0), (8.0, 12.0)]);
        // Reversing the drag restores the excluded interval.
        let baseline = t.cuts.clone();
        let (a, b) = t.trim_clip((5.0, 8.0), "start", 4.0, &baseline);
        assert!(t.cuts.is_empty());
        assert!((a - 4.0).abs() < 1e-9 && (b - 8.0).abs() < 1e-9);
    }

    #[test]
    fn options_round_trip_splits_cuts_and_zooms() {
        let mut t = timeline(12000.0, 15.0);
        t.splits = vec![4.0, 8.0];
        t.cuts = vec![(4.0, 5.0)];
        t.start = 1.0;
        t.end = 11.0;
        assert_eq!(t.add_zoom_at(2.0), Some(0));
        let mut options = std::collections::BTreeMap::new();
        t.write_options(&mut options);

        let mut restored = timeline(12000.0, 15.0);
        restored.read_options(&options);
        assert_eq!(restored.splits, t.splits);
        assert_eq!(restored.cuts, t.cuts);
        assert_eq!(restored.start, 1.0);
        assert_eq!(restored.end, 11.0);
        assert_eq!(restored.zooms.len(), 1);
        assert!((restored.zooms[0].start - t.zooms[0].start).abs() < 1e-9);
    }

    #[test]
    fn geometry_maps_source_time_to_pixels_and_back() {
        let mut t = timeline(120000.0, 30.0);
        t.width = 800.0;
        t.set_zoom(60, None);
        t.scroll = 20.0;
        let anchor = 400.0;
        let time = t.time_at(anchor);
        assert!((t.time_at(t.x(time)) - time).abs() < 1e-6);
    }
}
