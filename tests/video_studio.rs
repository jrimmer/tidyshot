//! End-to-end behavior for the video studio port.
//!
//! These drive the pure decoder policy, timeline, split/trim and export
//! modules the way the Python `test_video_decoder.py`, `test_video_splits.py`,
//! `test_studio.py` and `test_motion_blur.py` drove the Qt widgets, so a
//! reviewer can diff observable results against the source tests.

use std::collections::BTreeMap;
use std::path::Path;

use tidyshot::json::Value;
use tidyshot::studio::{
    ExportRequest, Options, compose_frame, compose_motion, encoder_arguments, export_plan,
    kept_segments,
};
use tidyshot::timeline::{Selection, Timeline};
use tidyshot::video_camera::{FramingEvent, options_at};
use tidyshot::video_decoder::{
    DEFAULT_HARDWARE_VIDEO_DECODING, DecoderPolicy, VARIABLE, decoder_policy, policy_environment,
    setting_enabled, setting_tooltip,
};
use tidyshot::video_motion::{Zoom, zoom_at};
use tidyshot::video_position::camera_rect;
use tidyshot::video_project::{build_archive, read_archive, validate_manifest};
use tidyshot::video_splits::{clean_splits, clips};

fn settings(enabled: bool) -> BTreeMap<String, Value> {
    BTreeMap::from([("hardware_video_decoding".to_owned(), Value::Bool(enabled))])
}

#[test]
fn software_decoding_is_the_default_on_every_gpu() {
    // Ported from tests/test_video_decoder.py
    //   (test_software_decoding_is_the_default_on_every_gpu)
    const { assert!(!DEFAULT_HARDWARE_VIDEO_DECODING) };
    for intel in [true, false] {
        assert_eq!(
            decoder_policy(&BTreeMap::new(), None, intel),
            DecoderPolicy::Software
        );
        assert_eq!(
            decoder_policy(&settings(false), None, intel),
            DecoderPolicy::Software
        );
    }
    assert_eq!(
        policy_environment(DecoderPolicy::Software),
        Some((VARIABLE, Some(",")))
    );
}

#[test]
fn setting_enables_hardware_decoding_except_on_intel() {
    // Ported from tests/test_video_decoder.py
    //   (test_setting_enables_hardware_decoding_except_on_intel)
    assert_eq!(
        decoder_policy(&settings(true), None, false),
        DecoderPolicy::Hardware
    );
    assert_eq!(
        decoder_policy(&settings(true), None, true),
        DecoderPolicy::Software
    );
    assert_eq!(
        policy_environment(DecoderPolicy::Hardware),
        Some((VARIABLE, None))
    );
    // The checkbox is usable off Intel and its tooltip names the restart.
    assert!(setting_enabled(false));
    assert!(setting_tooltip(false).contains("restarts"));
    assert!(!setting_enabled(true));
    assert!(setting_tooltip(true).contains("Intel"));
}

#[test]
fn value_set_before_startup_stays_authoritative() {
    // Ported from tests/test_video_decoder.py
    //   (test_value_set_before_startup_stays_authoritative)
    assert_eq!(
        decoder_policy(&settings(true), Some("vaapi"), false),
        DecoderPolicy::Explicit
    );
    assert_eq!(
        decoder_policy(&settings(false), Some(","), true),
        DecoderPolicy::Explicit
    );
    assert_eq!(policy_environment(DecoderPolicy::Explicit), None);
}

#[test]
fn split_frame_boundaries_and_clip_delete_restore() {
    // Ported from tests/test_video_splits.py
    //   (test_split_frame_boundaries_and_clip_delete_restore)
    let mut t = Timeline::default();
    t.set_duration(12000.0);
    t.set_frame_rate(15.0);
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
fn split_normalization_and_cut_intersections() {
    // Ported from tests/test_video_splits.py
    //   (test_split_normalization_and_cut_intersections)
    let values = vec![
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
    assert_eq!(clean_splits(&values), vec![2.0, 3.0]);
    assert!(clean_splits(&[]).is_empty());
    assert_eq!(
        clips(
            1.0,
            9.0,
            &[(2.0, 4.0), (3.0, 5.0), (8.0, 10.0)],
            &[3.0, 6.0, 7.0]
        ),
        vec![(1.0, 2.0), (5.0, 6.0), (6.0, 7.0), (7.0, 8.0)]
    );

    // The project manifest rejects the same invalid boundaries.
    let manifest = Value::Object(BTreeMap::from([
        (
            "format".to_owned(),
            Value::String("omnishot-video".to_owned()),
        ),
        ("version".to_owned(), Value::Number(1.0)),
        ("source".to_owned(), Value::String("source.mp4".to_owned())),
        ("metadata".to_owned(), Value::Object(BTreeMap::new())),
    ]));
    for invalid in [
        Value::Bool(true),
        Value::Number(f64::NAN),
        Value::Number(-1.0),
        Value::String("3".to_owned()),
    ] {
        let mut candidate = manifest.clone();
        let options = BTreeMap::from([("splits".to_owned(), Value::Array(vec![invalid.clone()]))]);
        candidate
            .as_object_mut()
            .unwrap()
            .insert("options".to_owned(), Value::Object(options));
        assert!(
            validate_manifest(&candidate).is_err(),
            "accepted {invalid:?}"
        );
    }
}

#[test]
fn timeline_round_trips_splits_cuts_and_zooms_through_a_project() {
    // Timeline persistence through the project manifest, matching
    // omnishot/test_video_project.py.
    let mut t = Timeline::default();
    t.set_duration(12000.0);
    t.set_frame_rate(15.0);
    t.splits = vec![4.0, 8.0];
    t.cuts = vec![(4.0, 5.0)];
    t.start = 1.0;
    t.end = 11.0;
    assert_eq!(t.add_zoom_at(2.0), Some(0));
    let mut options = Options::new();
    t.write_options(&mut options);

    let metadata = Value::Object(BTreeMap::new());
    let bytes = build_archive("source.mp4", b"media", None, &metadata, &options).unwrap();
    let project = read_archive(&bytes).unwrap();
    assert_eq!(project.options, options);

    let mut restored = Timeline::default();
    restored.set_duration(12000.0);
    restored.read_options(&project.options);
    assert_eq!(restored.splits, t.splits);
    assert_eq!(restored.cuts, t.cuts);
    assert_eq!(restored.start, 1.0);
    assert_eq!(restored.end, 11.0);
    assert_eq!(restored.zooms.len(), 1);
    assert!((restored.zooms[0].start - t.zooms[0].start).abs() < 1e-9);
}

#[test]
fn clips_can_be_trimmed_and_restored_across_a_drag_reversal() {
    let mut t = Timeline::default();
    t.set_duration(12000.0);
    t.set_frame_rate(30.0);
    t.splits = vec![4.0, 8.0];
    let baseline = t.cuts.clone();
    let bounds = t.trim_clip((4.0, 8.0), "start", 5.0, &baseline);
    assert_eq!(bounds, (5.0, 8.0));
    assert_eq!(t.cuts, vec![(4.0, 5.0)]);
    assert_eq!(t.clips(), vec![(0.0, 4.0), (5.0, 8.0), (8.0, 12.0)]);
    // Reversing the drag restores the excluded interval.
    let baseline = t.cuts.clone();
    let bounds = t.trim_clip((5.0, 8.0), "start", 4.0, &baseline);
    assert_eq!(bounds, (4.0, 8.0));
    assert!(t.cuts.is_empty());
}

#[test]
fn a_manifest_rejects_a_path_that_escapes_the_project() {
    // Ported from tests/test_video_project.py
    //   (test_project_rejects_paths_and_cancellation_preserves_destination)
    let manifest = Value::Object(BTreeMap::from([
        (
            "format".to_owned(),
            Value::String("omnishot-video".to_owned()),
        ),
        ("version".to_owned(), Value::Number(1.0)),
        (
            "source".to_owned(),
            Value::String("../outside.mp4".to_owned()),
        ),
        ("metadata".to_owned(), Value::Object(BTreeMap::new())),
    ]));
    assert!(validate_manifest(&manifest).is_err());
}

#[test]
fn export_defaults_to_software_x264_and_plans_retained_frames() {
    // Ported from tests/test_studio.py (test_studio_export_real_media) and
    // tests/test_motion_blur.py legacy blur migration.
    let software = encoder_arguments(&Options::new(), 320, 180, 15.0);
    assert!(software.contains(&"libx264".to_owned()));
    assert!(!software.contains(&"h264_vaapi".to_owned()));
    assert!(software.windows(2).any(|w| w == ["-video_size", "320x180"]));

    let hardware = encoder_arguments(
        &Options::from([("hardware".to_owned(), Value::Bool(true))]),
        320,
        180,
        15.0,
    );
    assert!(hardware.contains(&"h264_vaapi".to_owned()));
    assert!(hardware.contains(&"/dev/dri/renderD128".to_owned()));

    let request = ExportRequest {
        source: Path::new("/tmp/source.mp4"),
        destination: Path::new("/tmp/result.mp4"),
        metadata: &Value::Null,
        options: &Options::new(),
        fps: 15.0,
        speed: 1.0,
        start: 0.0,
        end: 1.2,
        cuts: &[],
    };
    let plan = export_plan(&request).unwrap();
    assert_eq!(plan.segments, vec![(0.0, 1.2)]);
    assert_eq!(plan.total, 18);
    assert_eq!(plan.times.len(), 18);
    assert_eq!(
        kept_segments(0.0, 10.0, &[(2.0, 4.0), (3.0, 5.0), (8.0, 10.0)]),
        vec![(0.0, 2.0), (5.0, 8.0)]
    );
}

#[test]
fn joined_zoom_transitions_never_zoom_out() {
    // Ported from tests/test_zoom_actions.py (test_joined_zoom_transitions)
    let a = Zoom {
        start: 1.0,
        end: 3.0,
        scale: 2.0,
        x: 0.25,
        y: 0.5,
        follow: false,
    };
    let b = Zoom {
        start: 3.0,
        end: 5.0,
        scale: 3.0,
        x: 0.75,
        y: 0.5,
        follow: false,
    };
    assert_eq!(
        zoom_at(&[a.clone(), b.clone()], 3.0, None, "Smooth"),
        (2.0, 0.25, 0.5)
    );
    for mode in ["Smooth", "Dynamic"] {
        let values: Vec<(f64, f64, f64)> = (2950..=3500)
            .step_by(10)
            .map(|t| zoom_at(&[a.clone(), b.clone()], f64::from(t) / 1000.0, None, mode))
            .collect();
        assert!(values.iter().all(|v| v.0 >= 2.0 - 1e-9));
        assert!(
            values
                .windows(2)
                .all(|w| w[0].0 <= w[1].0 && w[0].1 <= w[1].1)
        );
        assert_eq!(values.last().copied().unwrap(), (3.0, 0.75, 0.5));
    }
    assert_eq!(
        zoom_at(std::slice::from_ref(&a), 3.0, None, "Smooth").0,
        1.0
    );
    assert!(
        zoom_at(&[a.clone(), b.clone()], 1.1, None, "Dynamic").0
            > zoom_at(&[a, b], 1.1, None, "Smooth").0
    );
}

#[test]
fn recorded_camera_framing_follows_the_source_clock() {
    // Ported from the camera framing behavior in omnishot/camera_framing.py.
    let metadata = Value::Object(BTreeMap::from([(
        "camera_framing".to_owned(),
        Value::Array(vec![
            FramingEvent {
                t: 1.0,
                fullscreen: false,
                shape: Some("Square".to_owned()),
                placement: None,
            }
            .to_json(),
            FramingEvent {
                t: 3.0,
                fullscreen: true,
                shape: None,
                placement: None,
            }
            .to_json(),
        ]),
    )]));
    let options = Options::new();
    assert_eq!(
        options_at(&metadata, &options, 0.5)
            .get("camera_fullscreen")
            .and_then(Value::as_bool),
        Some(false)
    );
    assert_eq!(
        options_at(&metadata, &options, 2.0)
            .get("camera_shape")
            .and_then(Value::as_str),
        Some("Square")
    );
    assert_eq!(
        options_at(&metadata, &options, 3.5)
            .get("camera_fullscreen")
            .and_then(Value::as_bool),
        Some(true)
    );
}

#[test]
fn camera_rect_stays_inside_the_canvas_and_centres_the_zoom_focus() {
    let rect = camera_rect(1000.0, 500.0, 1.5, &Options::new(), 1.0);
    assert!(rect.x >= 0.0 && rect.y >= 0.0);
    assert!(rect.right() <= 1000.0 + 1e-9 && rect.bottom() <= 500.0 + 1e-9);

    // A frame with a solid quadrants image: the zoom focus must land on the
    // lower-right quadrant, matching the source's visual/export check.
    let mut data = vec![0u8; 400 * 200 * 4];
    for y in 0..200 {
        for x in 0..400 {
            let offset = (y * 400 + x) * 4;
            let colour = if y < 100 && x < 200 {
                [255, 0, 0, 255]
            } else if y >= 100 && x >= 200 {
                [0, 255, 0, 255]
            } else {
                [0, 0, 0, 255]
            };
            data[offset..offset + 4].copy_from_slice(&colour);
        }
    }
    let frame = tidyshot::pngzip::Image::new(400, 200, data);
    let options = Options::from([
        ("cursor".to_owned(), Value::Bool(false)),
        ("clicks".to_owned(), Value::Bool(false)),
        ("keys".to_owned(), Value::Bool(false)),
        (
            "zooms".to_owned(),
            Value::Array(vec![Value::Object(BTreeMap::from([
                ("start".to_owned(), Value::Number(0.0)),
                ("end".to_owned(), Value::Number(3.0)),
                ("scale".to_owned(), Value::Number(2.0)),
                ("x".to_owned(), Value::Number(0.75)),
                ("y".to_owned(), Value::Number(0.75)),
                ("follow".to_owned(), Value::Bool(false)),
            ]))]),
        ),
    ]);
    let result = compose_frame(&frame, 1.5, &Value::Null, &options, None);
    assert_eq!(result.pixel(200, 100), [0, 255, 0, 255]);
}

#[test]
fn motion_blur_changes_edges_and_stays_time_local() {
    // Ported from tests/test_motion_blur.py
    //   (test_blur_is_time_local_increases_with_intensity_and_respects_cuts)
    let width = 320;
    let height = 180;
    let mut data = vec![0u8; width * height * 4];
    for pixel in data.as_chunks_mut::<4>().0 {
        pixel.copy_from_slice(&[255, 255, 255, 255]);
    }
    let frame = tidyshot::pngzip::Image::new(width, height, data);
    let metadata = Value::Object(BTreeMap::from([(
        "cursor".to_owned(),
        Value::Array(vec![cursor(0.0, 0.1), cursor(1.0, 0.9)]),
    )]));
    let options = Options::from([
        ("fps".to_owned(), Value::Number(10.0)),
        ("cursor".to_owned(), Value::Bool(true)),
        ("cursor_style".to_owned(), Value::String("Dot".to_owned())),
        ("cursor_size".to_owned(), Value::Number(20.0)),
        (
            "cursor_color".to_owned(),
            Value::String("#000000".to_owned()),
        ),
        (
            "cursor_outline".to_owned(),
            Value::String("#000000".to_owned()),
        ),
        ("padding".to_owned(), Value::Number(0.0)),
        ("clicks".to_owned(), Value::Bool(false)),
        ("keys".to_owned(), Value::Bool(false)),
    ]);
    let starts: Vec<usize> = (0..4)
        .map(|level| {
            let level_options = with_blur(&options, level);
            let image = compose_motion(&frame, 0.5, &metadata, &level_options);
            (0..width)
                .find(|x| image.pixel(*x, height / 2)[0] < 220)
                .unwrap_or(width)
        })
        .collect();
    assert!(
        starts[0] > starts[1] && starts[1] > starts[2] && starts[2] > starts[3],
        "{starts:?}"
    );

    // The default level is None; a cut containing the frame disables blur.
    let none = with_blur(&options, 0);
    let reference = compose_motion(&frame, 0.5, &metadata, &none);
    let mut cut = with_blur(&options, 3);
    cut.insert(
        "cuts".to_owned(),
        Value::Array(vec![Value::Array(vec![
            Value::Number(0.2),
            Value::Number(0.5),
        ])]),
    );
    assert_eq!(
        compose_motion(&frame, 0.5, &metadata, &cut).data,
        reference.data
    );
}

#[test]
fn a_legacy_blur_boolean_migrates_to_medium() {
    // Ported from tests/test_motion_blur.py
    //   (test_legacy_blur_migration_level_persistence_and_manifest_validation)
    assert_eq!(
        tidyshot::motion_blur::blur_level(&BTreeMap::from([(
            "blur".to_owned(),
            Value::Bool(true)
        )])),
        2
    );
    assert_eq!(
        tidyshot::motion_blur::blur_level(&BTreeMap::from([(
            "blur".to_owned(),
            Value::Bool(false)
        )])),
        0
    );
}

fn cursor(t: f64, x: f64) -> Value {
    Value::Object(BTreeMap::from([
        ("t".to_owned(), Value::Number(t)),
        ("x".to_owned(), Value::Number(x)),
        ("y".to_owned(), Value::Number(0.5)),
    ]))
}

fn with_blur(options: &Options, level: i32) -> Options {
    let mut out = options.clone();
    out.insert("motion_blur".to_owned(), Value::Number(f64::from(level)));
    out
}
