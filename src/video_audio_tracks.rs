//! Source-specific audio edits and the shared preview/export mixing graph.
// Ported from omnishot/video_audio_tracks.py and omnishot/video_audio.py
//!
//! Playback and every exported stream use the same linear gain, so a quiet
//! source is never normalized up to hide a level the user chose. The graph is
//! a plain ffmpeg `-filter_complex` string built here and handed to the
//! exporter.

use std::collections::BTreeMap;

use crate::json::Value;

/// One track's persisted edit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackEdit {
    pub enabled: bool,
    pub volume: f64,
}

impl TrackEdit {
    /// Read a track edit, defaulting enabled and volume to the source values.
    #[must_use]
    pub fn from_json(value: &Value) -> Self {
        Self {
            enabled: value
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            volume: value
                .get("volume")
                .and_then(Value::as_f64)
                .filter(|v| v.is_finite())
                .unwrap_or(100.0),
        }
    }

    /// Read a persisted track list.
    #[must_use]
    pub fn list_from_json(value: Option<&Value>) -> Vec<Self> {
        value
            .and_then(Value::as_array)
            .map(|rows| rows.iter().map(Self::from_json).collect())
            .unwrap_or_default()
    }
}

/// The audio sources a capture recorded, for labeling the track controls.
///
/// A single-source MP4 without studio separation was captured mixed, matching
/// the source's `capture_audio_sources`.
// Ported from omnishot/video_audio_tracks.py (capture_audio_sources)
#[must_use]
pub fn capture_audio_sources(options: &BTreeMap<String, Value>) -> Vec<String> {
    let mut sources = Vec::new();
    if options.get("system").is_some_and(Value::is_truthy) {
        sources.push("system".to_owned());
    }
    if options.get("mic").is_some_and(Value::is_truthy) {
        sources.push("microphone".to_owned());
    }
    let separate = options.get("studio").is_some_and(Value::is_truthy)
        || options.get("separate_audio").is_some_and(Value::is_truthy);
    if separate || sources.len() >= 2 {
        sources
    } else {
        vec!["mixed".to_owned()]
    }
}

/// The default per-track edits for a set of sources.
// Ported from omnishot/video_audio_tracks.py (default_track_edits)
#[must_use]
pub fn default_track_edits(sources: &[String]) -> Vec<TrackEdit> {
    sources
        .iter()
        .map(|_| TrackEdit {
            enabled: true,
            volume: 100.0,
        })
        .collect()
}

/// The linear `volume=` filter for playback and export.
///
/// A muted or fully-silent result yields no filter, matching the source.
// Ported from omnishot/video_audio.py (audio_filters)
#[must_use]
pub fn audio_filters(options: &BTreeMap<String, Value>) -> Vec<String> {
    let mut volume = options
        .get("audio_volume")
        .and_then(Value::as_f64)
        .unwrap_or(100.0)
        / 100.0;
    volume = volume.clamp(0.0, 1.0);
    if options.get("audio_muted").is_some_and(Value::is_truthy) {
        volume = 0.0;
    }
    if (volume - 1.0).abs() < 1e-9 {
        Vec::new()
    } else {
        vec![format!("volume={volume}")]
    }
}

/// Build the mixing graph for the persisted track edits.
///
/// Returns `None` when no track is enabled, so the caller drops audio rather
/// than mixing silence.
// Ported from omnishot/video_audio_tracks.py (mix_graph)
#[must_use]
pub fn mix_graph(
    options: &BTreeMap<String, Value>,
    input_index: usize,
    after: &[String],
) -> Option<String> {
    let rows = TrackEdit::list_from_json(options.get("audio_tracks"));
    let mut filters = Vec::new();
    let mut labels = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        if !row.enabled {
            continue;
        }
        let gain = row.volume.clamp(0.0, 200.0) / 100.0;
        let label = format!("os_track_{index}");
        labels.push(format!("[{label}]"));
        filters.push(format!(
            "[{input_index}:a:{index}]aresample=48000:async=1:first_pts=0,volume={gain}[{label}]"
        ));
    }
    if labels.is_empty() {
        return None;
    }
    let mut chain = vec![format!(
        "amix=inputs={}:duration=longest:normalize=0",
        labels.len()
    )];
    chain.extend(after.iter().cloned());
    chain.extend(audio_filters(options));
    filters.push(format!("{}{}[os_audio]", labels.join(""), chain.join(",")));
    Some(filters.join(";"))
}

/// The ffmpeg output arguments that carry the selected audio edit.
///
/// A tracked mix maps `[os_audio]`; otherwise the source's own tracks pass
/// through with the volume filter applied.
// Ported from omnishot/video_audio_tracks.py (export_audio_args)
#[must_use]
pub fn export_audio_args(
    options: &BTreeMap<String, Value>,
    input_index: usize,
    after: &[String],
) -> Vec<String> {
    let mut args = if options
        .get("audio_tracks")
        .and_then(Value::as_array)
        .is_some()
    {
        let with_apad: Vec<String> = after
            .iter()
            .cloned()
            .chain(std::iter::once("apad".to_owned()))
            .collect();
        match mix_graph(options, input_index, &with_apad) {
            Some(graph) => vec![
                "-filter_complex".to_owned(),
                graph,
                "-map".to_owned(),
                "[os_audio]".to_owned(),
                "-shortest".to_owned(),
            ],
            None => vec!["-an".to_owned()],
        }
    } else {
        let mut filters = after.to_vec();
        filters.extend(audio_filters(options));
        let mut result = vec!["-map".to_owned(), format!("{input_index}:a?")];
        if !filters.is_empty() {
            result.push("-af".to_owned());
            result.push(filters.join(","));
        }
        result
    };
    if options.get("audio_mono").is_some_and(Value::is_truthy) {
        args.push("-ac".to_owned());
        args.push("1".to_owned());
    }
    args
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

    fn tracks(rows: &[(bool, f64)]) -> Value {
        Value::Array(
            rows.iter()
                .map(|(enabled, volume)| {
                    let mut object = BTreeMap::new();
                    object.insert("enabled".to_owned(), Value::Bool(*enabled));
                    object.insert("volume".to_owned(), Value::Number(*volume));
                    Value::Object(object)
                })
                .collect(),
        )
    }

    #[test]
    fn source_labels_match_the_capture_shape() {
        assert_eq!(
            capture_audio_sources(&options(&[("system", Value::Bool(true))])),
            vec!["mixed"]
        );
        assert_eq!(
            capture_audio_sources(&options(&[
                ("system", Value::Bool(true)),
                ("mic", Value::Bool(true)),
            ])),
            vec!["system", "microphone"]
        );
        assert_eq!(
            capture_audio_sources(&options(&[
                ("system", Value::Bool(true)),
                ("studio", Value::Bool(true)),
            ])),
            vec!["system"]
        );
        assert_eq!(
            default_track_edits(&["system".to_owned()])[0],
            TrackEdit {
                enabled: true,
                volume: 100.0
            }
        );
    }

    #[test]
    fn volume_filter_uses_linear_gain_and_mute() {
        assert!(audio_filters(&options(&[])).is_empty());
        assert_eq!(
            audio_filters(&options(&[("audio_volume", Value::Number(50.0))])),
            vec!["volume=0.5"]
        );
        assert_eq!(
            audio_filters(&options(&[
                ("audio_volume", Value::Number(200.0)),
                ("audio_muted", Value::Bool(true)),
            ])),
            vec!["volume=0"]
        );
    }

    #[test]
    fn mixing_graph_resamples_each_track_and_normalizes_off() {
        let opts = options(&[("audio_tracks", tracks(&[(true, 75.0), (false, 100.0)]))]);
        let graph = mix_graph(&opts, 1, &["apad".to_owned()]).unwrap();
        assert!(
            graph.contains("[1:a:0]aresample=48000:async=1:first_pts=0,volume=0.75[os_track_0]")
        );
        assert!(!graph.contains("os_track_1"));
        assert!(graph.contains("amix=inputs=1:duration=longest:normalize=0,apad[os_audio]"));
        // No enabled track means no graph and a dropped audio stream.
        let silent = options(&[("audio_tracks", tracks(&[(false, 100.0)]))]);
        assert!(mix_graph(&silent, 0, &[]).is_none());
    }

    #[test]
    fn export_arguments_differ_for_tracked_and_plain_audio() {
        let tracked = options(&[("audio_tracks", tracks(&[(true, 100.0)]))]);
        let args = export_audio_args(&tracked, 0, &[]);
        assert_eq!(args[0], "-filter_complex");
        assert!(args.contains(&"[os_audio]".to_owned()));
        let plain = options(&[("audio_volume", Value::Number(50.0))]);
        assert_eq!(
            export_audio_args(&plain, 0, &[]),
            vec!["-map", "0:a?", "-af", "volume=0.5"]
        );
        let mono = options(&[
            ("audio_volume", Value::Number(50.0)),
            ("audio_mono", Value::Bool(true)),
        ]);
        assert_eq!(export_audio_args(&mono, 0, &[]).last().unwrap(), "1");
    }
}
