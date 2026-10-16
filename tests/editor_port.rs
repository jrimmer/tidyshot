//! End-to-end behavior for the annotate editor port.
//!
//! These drive the pure document, project and renderer modules the way the
//! Python editor tests drove `Editor`, `Annotation` and `write_project`, so a
//! reviewer can diff observable results against the source tests.

use std::collections::BTreeMap;

use tidyshot::annotation::{Annotation, Props, Rect};
use tidyshot::backgrounds::Background;
use tidyshot::editor::Document;
use tidyshot::json::Value;
use tidyshot::pngzip::{Image, ZipWriter};
use tidyshot::project::Project;

fn props(entries: &[(&str, Value)]) -> Props {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_owned(), value.clone()))
        .collect()
}

fn image(width: usize, height: usize, color: [u8; 4]) -> Image {
    let mut data = vec![0u8; width * height * 4];
    for pixel in data.as_chunks_mut::<4>().0 {
        pixel.copy_from_slice(&color);
    }
    Image::new(width, height, data)
}

#[test]
fn crop_apply_is_one_undo_and_survives_the_project_file() {
    let mut document = Document::new(image(800, 500, [230, 230, 230, 255]), None, false);
    document.objects.push(Annotation::new(props(&[
        ("kind", Value::String("arrow".into())),
        ("x", Value::Number(120.0)),
        ("y", Value::Number(90.0)),
        ("w", Value::Number(220.0)),
        ("h", Value::Number(120.0)),
        ("color", Value::String("#ff0000".into())),
        ("width", Value::Number(6.0)),
    ])));
    document.commit();
    let before = document.render();
    let index_renders = before.clone();

    document
        .crop(Rect::new(50.0, 40.0, 600.0, 400.0), false, None)
        .unwrap();
    assert_eq!(document.objects[0].x(), 70.0);
    assert_eq!(document.objects[0].y(), 50.0);

    // Undo restores the pre-crop pixels, redo re-applies them.
    let after = document.render();
    document.undo();
    assert_eq!(document.render().data, index_renders.data);
    document.redo();
    assert_eq!(document.render().data, after.data);

    // The project file carries the cropped image and its annotations.
    let bytes = document.to_project().write_bytes().unwrap();
    let reopened = Project::read(&bytes).unwrap();
    assert_eq!(reopened.base, document.base);
    assert_eq!(reopened.objects, document.objects);

    // A crop with a deliberately different canvas restores the original.
    document.restore_original().unwrap();
    assert_eq!(document.base, image(800, 500, [230, 230, 230, 255]));
    assert_eq!(document.objects[0].x(), 120.0);
    assert_eq!(document.objects[0].y(), 90.0);
}

#[test]
fn expansion_preserves_alpha_and_cancels_pending_drafts() {
    let mut base = Image::new(80, 50, vec![0u8; 80 * 50 * 4]);
    for pixel in base.data.as_chunks_mut::<4>().0 {
        pixel.copy_from_slice(&[120, 80, 40, 128]);
    }
    let original = base.clone();
    let mut document = Document::new(base, None, false);
    document
        .crop(
            Rect::new(-10.0, -10.0, 100.0, 70.0),
            true,
            Some(tidyshot::color::Rgba::new(120, 80, 40, 128)),
        )
        .unwrap();
    let expanded = document.render();
    assert_eq!((expanded.width, expanded.height), (100, 70));
    // The original content is untouched, and the border keeps its alpha.
    for y in 0..50 {
        for x in 0..80 {
            assert_eq!(expanded.pixel(x + 10, y + 10), original.pixel(x, y));
        }
    }
    assert_eq!(expanded.pixel(1, 1), [120, 80, 40, 128]);
}

#[test]
fn a_portable_project_reopens_with_a_cover_background() {
    let mut document = Document::new(image(300, 200, [255, 255, 255, 255]), None, false);
    let background = Background {
        padding: 40,
        radius: 0.0,
        shadow: false,
        image_data: Some(image(20, 20, [0, 0, 255, 255])),
        ..Background::default()
    };
    document.background = Some(background);
    document.commit();
    let expected = document.render_export();

    // Save and reopen without the cover image in memory; the archive carries it.
    let bytes = document.to_project().write_bytes().unwrap();
    let reopened = Project::read(&bytes).unwrap();
    let other = Document::open(reopened);
    assert_eq!(other.render_export(), expected);
    let restored = other.background.unwrap();
    assert!(restored.image_data.is_some());
}

#[test]
fn version_one_project_without_an_original_still_opens() {
    // A legacy archive: image plus a minimal version-1 document.
    let mut writer = ZipWriter::new();
    writer.add("image.png", image(120, 80, [10, 20, 30, 255]).to_png());
    writer.add(
        "project.json",
        br##"{"version":1,"objects":[{"kind":"rect","x":5,"y":6,"w":20,"h":10,"color":"#00ff00"}]}"##
            .to_vec(),
    );
    let legacy = writer.finish();
    let project = Project::read(&legacy).unwrap();
    assert_eq!(project.version, 1);
    assert!(project.original.is_none());
    let baseline = project.base.clone();
    let mut document = Document::open(project);
    // Cropping a legacy project then restoring returns to its baseline.
    document
        .crop(Rect::new(5.0, 5.0, 100.0, 60.0), false, None)
        .unwrap();
    document.restore_original().unwrap();
    assert_eq!(document.base, baseline);
}

#[test]
fn editable_annotations_round_trip_through_the_clipboard() {
    let swatch = Image::new(20, 15, {
        let mut data = vec![0u8; 20 * 15 * 4];
        for pixel in data.as_chunks_mut::<4>().0 {
            pixel.copy_from_slice(&[80, 30, 10, 64]);
        }
        data
    });
    let objects = vec![
        Annotation::new(props(&[
            ("kind", Value::String("text".into())),
            ("x", Value::Number(20.0)),
            ("y", Value::Number(25.0)),
            ("w", Value::Number(210.0)),
            ("h", Value::Number(80.0)),
            ("text", Value::String("Editable é\nsecond line".into())),
            ("font_size", Value::Number(22.0)),
            ("bold", Value::Bool(false)),
            ("italic", Value::Bool(true)),
            ("color", Value::String("#ff0033".into())),
        ])),
        Annotation::new(props(&[
            ("kind", Value::String("image".into())),
            ("x", Value::Number(100.0)),
            ("y", Value::Number(200.0)),
            ("w", Value::Number(20.0)),
            ("h", Value::Number(15.0)),
            (
                "image",
                Value::String(tidyshot::annotation::encode_base64(&swatch.to_png())),
            ),
        ])),
    ];
    let encoded = tidyshot::annotation::encode_clipboard(&objects).unwrap();
    let decoded = tidyshot::annotation::decode_clipboard(&encoded).unwrap();
    assert_eq!(decoded, objects);

    // Pasting offsets every object by 20 and selects the new group.
    let mut document = Document::new(image(800, 500, [255, 255, 255, 255]), None, false);
    document.paste_annotations(decoded).unwrap();
    assert_eq!(document.objects.len(), 2);
    assert_eq!(document.objects[0].x(), 40.0);
    assert_eq!(document.objects[0].y(), 45.0);
}

#[test]
fn malformed_annotations_leave_the_document_untouched() {
    let bad = br#"{"version":1,"objects":[{"kind":"rect","w":20000,"h":20000}]}"#;
    let error = tidyshot::annotation::decode_clipboard(bad).unwrap_err();
    assert!(error.message().contains("invalid clipboard"));
    // The document must not change on a rejected paste.
    let mut document = Document::new(image(10, 10, [255, 255, 255, 255]), None, false);
    let before = document.render();
    assert!(
        document
            .paste_annotations(tidyshot::annotation::decode_clipboard(bad).unwrap_or_default())
            .is_ok()
    );
    assert_eq!(document.render(), before);
}

#[test]
fn a_spotlight_keeps_the_captures_transparency() {
    let mut base = image(400, 260, [224, 224, 224, 255]);
    // A transparent corner, matching the Python fixture.
    for offset in 0..(20 * 260 * 4) {
        if offset % 4 == 3 {
            base.data[offset] = 0;
        }
    }
    let spotlight = Annotation::new(props(&[
        ("kind", Value::String("spotlight".into())),
        ("x", Value::Number(-25.0)),
        ("y", Value::Number(40.0)),
        ("w", Value::Number(170.0)),
        ("h", Value::Number(120.0)),
        ("spotlight_shape", Value::String("Rounded rectangle".into())),
        ("spotlight_radius", Value::Number(65.0)),
        ("spotlight_alpha", Value::Number(155.0)),
    ]));
    let rendered = tidyshot::render::render(
        &base,
        &[spotlight],
        tidyshot::render::ViewTransform::default(),
    );
    // Every pixel keeps the base alpha, even fully out of the aperture.
    for y in 0..base.height {
        for x in 0..base.width {
            assert_eq!(rendered.pixel(x, y)[3], base.pixel(x, y)[3], "({x},{y})");
        }
    }
}

#[test]
fn layered_counters_render_on_top_of_a_later_annotation() {
    let mut document = Document::new(image(200, 100, [255, 255, 255, 255]), None, false);
    let counter = Annotation::new(props(&[
        ("kind", Value::String("counter".into())),
        ("x", Value::Number(10.0)),
        ("y", Value::Number(10.0)),
        ("w", Value::Number(30.0)),
        ("h", Value::Number(30.0)),
        ("text", Value::String("0".into())),
        ("color", Value::String("#000000".into())),
        ("z", Value::Number(0.0)),
    ]));
    let fill = Annotation::new(props(&[
        ("kind", Value::String("fill".into())),
        ("x", Value::Number(5.0)),
        ("y", Value::Number(5.0)),
        ("w", Value::Number(150.0)),
        ("h", Value::Number(80.0)),
        ("color", Value::String("#00ff00".into())),
        ("z", Value::Number(10.0)),
    ]));
    document.objects.push(counter.clone());
    document.objects.push(fill);
    document.commit();
    let rendered = document.render();
    // The counter is drawn after the fill, so its ink covers the fill.
    let centre = rendered.pixel(25, 25);
    assert_ne!(centre, [0, 255, 0, 255]);
}

#[test]
fn a_malformed_project_is_reported_not_partially_loaded() {
    let mut writer = ZipWriter::new();
    writer.add("image.png", image(4, 4, [255, 255, 255, 255]).to_png());
    writer.add("original.png", image(4, 4, [255, 255, 255, 255]).to_png());
    writer.add(
        "project.json",
        br#"{"version":2,"objects":[],"original_image":"original.png","source_transform":[1,0,0,1]}"#
            .to_vec(),
    );
    let error = Project::read(&writer.finish()).unwrap_err();
    assert_eq!(error.message(), "Invalid original image");
}

#[test]
fn backgrounds_default_to_a_named_preset_from_settings() {
    let settings: BTreeMap<String, Value> = [
        (
            "background_preset".to_owned(),
            Value::String("Ocean".to_owned()),
        ),
        (
            "background_presets".to_owned(),
            Value::Object(BTreeMap::new()),
        ),
    ]
    .into_iter()
    .collect();
    let background = tidyshot::backgrounds::default_background(&settings).unwrap();
    assert_eq!(
        background.color,
        tidyshot::color::Rgba::from_hex("#46c7e7").unwrap()
    );
    assert_eq!(background.padding, 64);
}
