//! Recognized text and explicit actions for links found locally in it.
// Ported from omnishot/text_result.py
//!
//! OCR and QR reading are independent: a noisy OCR pass must never hide a QR
//! link, so both run and their results are merged. Link detection preserves
//! the recognized spelling and only prepends `https://` to a scheme-less web
//! address.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::OnceLock;

use crate::commands::{CommandRunner, CommandSpec};
use crate::ocr::{self, OcrError};

/// The IANA root-zone snapshot shipped beside the module.
const TLDS: &str = include_str!("data/tlds-alpha-by-domain.txt");

/// Why a text extraction could not return a result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextError {
    message: String,
}

impl TextError {
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for TextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for TextError {}

impl From<OcrError> for TextError {
    fn from(error: OcrError) -> Self {
        Self {
            message: error.message().to_owned(),
        }
    }
}

/// OCR preferences that shape an extraction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextSettings {
    pub ocr_languages: String,
    pub ocr_linebreaks: bool,
    pub ocr_detect_links: bool,
}

impl Default for TextSettings {
    fn default() -> Self {
        Self {
            ocr_languages: ocr::AUTO.to_owned(),
            ocr_linebreaks: true,
            ocr_detect_links: true,
        }
    }
}

impl TextSettings {
    /// Read the OCR keys out of a JSON settings object, keeping defaults for
    /// anything missing or the wrong type.
    // Ported from omnishot/backend.py (DEFAULTS)
    #[must_use]
    pub fn from_value(settings: &crate::json::Value) -> Self {
        let get = |key: &str| settings.get(key);
        Self {
            ocr_languages: get("ocr_languages")
                .and_then(crate::json::Value::as_str)
                .filter(|value| !value.is_empty())
                .unwrap_or(ocr::AUTO)
                .to_owned(),
            ocr_linebreaks: get("ocr_linebreaks")
                .and_then(crate::json::Value::as_bool)
                .unwrap_or(true),
            ocr_detect_links: get("ocr_detect_links")
                .and_then(crate::json::Value::as_bool)
                .unwrap_or(true),
        }
    }
}

/// Reads QR codes out of an image.
///
/// No pure-Rust decoder is vendored yet; production wires a decoder through
/// this trait once it is, and tests substitute a fake so the extraction merge
/// is asserted without a native library.
pub trait QrReader {
    fn read(&self, path: &Path) -> Result<String, String>;
}

/// A [`QrReader`] backed by an external decoder command and a runner.
///
/// `zbarimg --raw --quiet <path>` prints one value per line, matching the
/// multi-code newline joining upstream. If a vendored decoder replaces this,
/// the trait boundary stays the same.
pub struct CommandQrReader<R> {
    runner: R,
    program: String,
}

impl<R> CommandQrReader<R> {
    #[must_use]
    pub fn new(runner: R, program: impl Into<String>) -> Self {
        Self {
            runner,
            program: program.into(),
        }
    }
}

impl<R: CommandRunner> QrReader for CommandQrReader<R> {
    fn read(&self, path: &Path) -> Result<String, String> {
        let spec = CommandSpec::new(&self.program)
            .args(["--raw", "--quiet"])
            .arg(path.to_string_lossy().into_owned());
        let output = self.runner.run(&spec).map_err(|error| error.to_string())?;
        Ok(output.stdout_string().trim().to_owned())
    }
}

/// One detected link: the label as recognized and the URL to open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub label: String,
    pub url: String,
}

/// The IANA root zone, upper-cased, from the bundled snapshot.
fn tlds() -> &'static BTreeSet<String> {
    static ZONES: OnceLock<BTreeSet<String>> = OnceLock::new();
    ZONES.get_or_init(|| {
        TLDS.lines()
            .skip(1)
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_ascii_uppercase)
            .collect()
    })
}

/// Every web address in `text`, in order, with duplicates collapsed.
///
/// Recognized spelling is kept; a scheme-less address gets `https://` only
/// when its final label is a real root zone. Malformed authorities, non-web
/// schemes, and addresses over the 8192-byte ceiling are dropped.
// Ported from omnishot/text_result.py (detected_links)
pub fn detected_links(text: &str) -> Vec<Link> {
    let zones = tlds();
    let mut links: Vec<Link> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut cursor = 0;
    while cursor < text.len() {
        if cursor > 0 {
            let previous = text[..cursor].chars().next_back().unwrap();
            if is_glue(previous) {
                cursor += next_char_len(text, cursor);
                continue;
            }
        }
        let rest = &text[cursor..];
        let Some(length) = match_candidate(rest) else {
            cursor += next_char_len(text, cursor);
            continue;
        };
        let matched = &rest[..length];
        cursor += length;
        if let Some(link) = build_link(matched, zones)
            && seen.insert(link.url.clone())
        {
            links.push(link);
        }
        if links.len() >= 128 {
            break;
        }
    }
    links
}

/// Turn a matched token into a link, or `None` when it is not a web address.
fn build_link(matched: &str, zones: &BTreeSet<String>) -> Option<Link> {
    let mut value = matched.trim_end_matches(['.', ',', ';', ':', '!', '\'', '\u{2019}']);
    for (left, right) in [('(', ')'), ('[', ']'), ('{', '}')] {
        while value.ends_with(right) && value.matches(right).count() > value.matches(left).count() {
            value = &value[..value.len() - right.len_utf8()];
        }
    }
    let explicit = starts_with_http(value);
    let url = if explicit {
        value.to_owned()
    } else {
        format!("https://{value}")
    };
    if url.len() > 8192 || url.chars().any(|c| (c as u32) < 32) {
        return None;
    }
    let parsed = url::Url::parse(&url).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    if parsed.port() == Some(0) {
        return None;
    }
    let host = parsed.host_str()?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.is_empty() {
        return None;
    }
    let is_ip_literal = host.contains(':');
    if !is_ip_literal && host != "localhost" && !host.contains('.') {
        return None;
    }
    if !explicit {
        let ascii = if is_ip_literal {
            host.to_owned()
        } else {
            idna::domain_to_ascii(host).ok()?
        };
        let label = ascii.rsplit('.').next().unwrap_or("");
        if !zones.contains(&label.to_ascii_uppercase()) {
            return None;
        }
    }
    Some(Link {
        label: value.to_owned(),
        url,
    })
}

fn starts_with_http(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

fn is_glue(character: char) -> bool {
    character == '@'
        || character == '.'
        || character == '/'
        || character == '-'
        || character == '_'
        || character.is_alphanumeric()
}

fn next_char_len(text: &str, at: usize) -> usize {
    text[at..].chars().next().map_or(1, char::len_utf8)
}

/// A character that terminates any candidate token.
fn is_terminator(character: char) -> bool {
    character.is_whitespace() || matches!(character, '<' | '>' | '"' | '\u{201c}' | '\u{201d}')
}

/// The byte length of the candidate web address starting at `rest`, if any.
///
/// Mirrors the source regex's alternation: an explicit scheme or `www.` first,
/// then the dotted scheme-less form, whose final label must start with a
/// letter.
fn match_candidate(rest: &str) -> Option<usize> {
    for prefix in ["https://", "http://"] {
        if let Some(length) = match_prefixed(rest, prefix) {
            return Some(length);
        }
    }
    if let Some(length) = match_prefixed(rest, "www.") {
        return Some(length);
    }
    match_dotted(rest)
}

/// Match `prefix` (case-insensitive ASCII) plus at least one non-terminator.
fn match_prefixed(rest: &str, prefix: &str) -> Option<usize> {
    let bytes = rest.as_bytes();
    if bytes.len() < prefix.len() || !bytes[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
    {
        return None;
    }
    let mut end = prefix.len();
    for (offset, character) in rest[end..].char_indices() {
        if is_terminator(character) {
            break;
        }
        end = prefix.len() + offset + character.len_utf8();
    }
    (end > prefix.len()).then_some(end)
}

/// Match the scheme-less `label.label.final[:port][/path]` form.
///
/// The intermediate labels are the greedy `(label\.)+` run; the final label
/// is the last label whose first character is a letter, so a trailing numeric
/// label is left out exactly as the regex backtracking does.
fn match_dotted(rest: &str) -> Option<usize> {
    let position = match_leading_label(rest, 0)?;
    if !rest[position..].starts_with('.') {
        return None;
    }
    let mut final_end = None;
    let mut cursor = position + 1;
    loop {
        let end = word_hyphen_end(rest, cursor);
        if end == cursor {
            break;
        }
        if rest[cursor..]
            .chars()
            .next()
            .is_some_and(char::is_alphabetic)
        {
            final_end = Some(end);
        }
        if rest[end..].starts_with('.') {
            cursor = end + 1;
        } else {
            break;
        }
    }
    let mut position = final_end?;
    if rest[position..].starts_with(':') {
        let mut digits = 0;
        let mut end = position + 1;
        for character in rest[end..].chars() {
            if !character.is_ascii_digit() || digits == 5 {
                break;
            }
            digits += 1;
            end += character.len_utf8();
        }
        if digits == 0 {
            return None;
        }
        position = end;
    }
    if rest[position..].starts_with(['/', '?', '#']) {
        let mut end = position + 1;
        for character in rest[end..].chars() {
            if is_terminator(character) {
                break;
            }
            end += character.len_utf8();
        }
        position = end;
    }
    Some(position)
}

/// Match one `[^\W_](?:[\w-]*[^\W_])?` label at `position`.
fn match_leading_label(rest: &str, position: usize) -> Option<usize> {
    let end = word_hyphen_end(rest, position);
    if end == position {
        return None;
    }
    let first = rest[position..].chars().next().unwrap();
    let last = rest[position..end].chars().next_back().unwrap();
    (is_word_not_underscore(first) && is_word_not_underscore(last)).then_some(end)
}

/// The end of the maximal `[\w-]` run starting at `position`.
fn word_hyphen_end(rest: &str, position: usize) -> usize {
    let mut end = position;
    for character in rest[position..].chars() {
        if character == '-' || character == '_' || character.is_alphanumeric() {
            end += character.len_utf8();
        } else {
            break;
        }
    }
    end
}

/// A word character other than underscore.
fn is_word_not_underscore(character: char) -> bool {
    character != '_' && character.is_alphanumeric()
}

/// Read text and QR codes independently and merge them.
///
/// `languages` and `linebreaks` apply to the OCR half only. When both halves
/// fail, the OCR error is surfaced. QR values already present in the text or
/// its detected links are not repeated.
// Ported from omnishot/text_result.py (extract)
pub fn extract<R: CommandRunner, Q: QrReader>(
    runner: &R,
    qr: &Q,
    path: &Path,
    languages: &str,
    linebreaks: bool,
) -> Result<String, TextError> {
    let mut errors: Vec<TextError> = Vec::new();
    let text = match ocr::recognize(runner, path, languages, linebreaks) {
        Ok(value) => value,
        Err(error) => {
            errors.push(error.into());
            String::new()
        }
    };
    let qr_text = match qr.read(path) {
        Ok(value) => value,
        Err(message) => {
            errors.push(TextError { message });
            String::new()
        }
    };
    if text.is_empty()
        && qr_text.is_empty()
        && let Some(error) = errors.into_iter().next()
    {
        return Err(error);
    }

    let mut known: BTreeSet<String> = text.lines().map(str::to_owned).collect();
    for link in detected_links(&text) {
        known.insert(link.url);
    }
    let mut parts: Vec<String> = Vec::new();
    if !text.is_empty() {
        parts.push(text);
    }
    for value in qr_text.lines() {
        if !value.is_empty() && !known.contains(value) {
            parts.push(value.to_owned());
            known.insert(value.to_owned());
        }
    }
    let separator = if linebreaks { "\n\n" } else { " " };
    Ok(parts.join(separator))
}

/// The state behind the recognized-text dialog.
///
/// The Qt view renders this; every transition that matters to the user
/// (copy, edit, link selection) is decided here and asserted in tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextResult {
    text: String,
    detect: bool,
    copied: bool,
    status: String,
}

impl TextResult {
    /// The default status shown before any edit or copy.
    pub const REVIEW: &'static str = "Review or edit the recognized text";
    /// The status after a successful clipboard write.
    pub const COPIED: &'static str = "Text copied to clipboard";
    /// The status after the user changes the text.
    pub const EDITED: &'static str = "Edited text \u{2014} copy to use your changes";

    #[must_use]
    pub fn new(text: impl Into<String>, detect: bool, copied: bool) -> Self {
        Self {
            text: text.into(),
            detect,
            copied,
            status: if copied {
                Self::COPIED.to_owned()
            } else {
                Self::REVIEW.to_owned()
            },
        }
    }

    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    #[must_use]
    pub fn copied(&self) -> bool {
        self.copied
    }

    #[must_use]
    pub fn status(&self) -> &str {
        &self.status
    }

    /// Whether link detection is enabled for this result.
    #[must_use]
    pub fn detect(&self) -> bool {
        self.detect
    }

    /// The links currently shown for the text, or none when detection is off.
    #[must_use]
    pub fn links(&self) -> Vec<Link> {
        if self.detect {
            detected_links(&self.text)
        } else {
            Vec::new()
        }
    }

    /// Replace the text as the user types; this drops the copied state.
    pub fn set_text(&mut self, text: impl Into<String>) {
        let text = text.into();
        if self.text != text {
            self.text = text;
            self.copied = false;
            self.status = Self::EDITED.to_owned();
        }
    }

    /// Copy the current text through `wl-copy`.
    pub fn copy<R: CommandRunner>(&mut self, runner: &R) -> Result<(), TextError> {
        let spec = crate::commands::clipboard_spec(
            "text/plain;charset=utf-8",
            self.text.clone().into_bytes(),
        );
        match runner.run(&spec) {
            Ok(_) => {
                self.copied = true;
                self.status = Self::COPIED.to_owned();
                Ok(())
            }
            Err(error) => {
                self.status = format!("Could not copy text: {error}");
                Err(TextError {
                    message: error.to_string(),
                })
            }
        }
    }

    /// The URL behind link `row`, for an explicit Open action.
    #[must_use]
    pub fn open(&self, row: usize) -> Option<String> {
        self.links().get(row).map(|link| link.url.clone())
    }

    /// Whether any link is available to open.
    #[must_use]
    pub fn can_open(&self) -> bool {
        !self.links().is_empty()
    }
}

/// The label and URL lists the dialog renders for `links`.
///
/// The Qt view keeps the two parallel lists as flat string models; this is
/// the one place the pair is projected so the dialog and its tests agree.
#[must_use]
pub fn link_projection(links: &[Link]) -> (Vec<String>, Vec<String>) {
    let labels = links.iter().map(|link| link.label.clone()).collect();
    let urls = links.iter().map(|link| link.url.clone()).collect();
    (labels, urls)
}

/// The state the dialog takes when a recognition completes.
///
/// This is the mapping from a pipeline outcome to what the recognized-text
/// dialog shows, so the completion path can be asserted without a Qt engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    /// The status line shown in the dialog.
    pub status: String,
    /// Whether the dialog should be visible.
    pub shown: bool,
    /// The reviewable result, when the dialog needs the full text.
    pub result: Option<TextResult>,
}

/// Convert a recognition outcome into the dialog's state.
///
/// An ordinary result that copied cleanly stays out of the way, a result with
/// links or a copy failure opens the dialog, and an error is surfaced rather
/// than swallowed. Nothing recognized produces no dialog.
pub fn complete(outcome: Result<Presentation, TextError>) -> Completion {
    match outcome {
        Ok(Presentation::Copied) => Completion {
            status: TextResult::COPIED.to_owned(),
            shown: false,
            result: None,
        },
        Ok(Presentation::Dialog(result)) => Completion {
            status: result.status().to_owned(),
            shown: true,
            result: Some(result),
        },
        Ok(Presentation::Empty) => Completion {
            status: "No text or QR code found".to_owned(),
            shown: false,
            result: None,
        },
        Err(error) => Completion {
            status: error.to_string(),
            shown: true,
            result: None,
        },
    }
}

/// The result of presenting recognized text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Presentation {
    /// No dialog is needed; the text was already copied.
    Copied,
    /// The dialog should open with this state.
    Dialog(TextResult),
    /// There was nothing to show or copy.
    Empty,
}

/// Decide whether recognized text needs a dialog.
///
/// Ordinary OCR stays a copy action: when the text copied cleanly and link
/// detection is either off or found nothing, no dialog is shown. `review`
/// forces a dialog and skips the automatic copy.
// Ported from omnishot/text_result.py (present)
pub fn present<R: CommandRunner>(
    runner: &R,
    text: &str,
    settings: &TextSettings,
    review: bool,
) -> Presentation {
    if text.is_empty() && !review {
        return Presentation::Empty;
    }
    let mut result = TextResult::new(text, settings.ocr_detect_links, false);
    if !review
        && result.copy(runner).is_ok()
        && (!settings.ocr_detect_links || result.links().is_empty())
    {
        return Presentation::Copied;
    }
    Presentation::Dialog(result)
}

/// Extract text from `path` under `settings`, then present the result.
///
/// `linebreaks` overrides the saved preference for this call only. This is the
/// whole `recognize_text` path: extract once, then copy or open the review
/// dialog. When nothing was recognized the OCR/QR error is returned so the
/// caller can surface it.
// Ported from omnishot/app.py (recognize_text)
pub fn recognize<R: CommandRunner, Q: QrReader>(
    runner: &R,
    qr: &Q,
    path: &Path,
    settings: &TextSettings,
    linebreaks: Option<bool>,
) -> Result<Presentation, TextError> {
    let value = extract(
        runner,
        qr,
        path,
        &settings.ocr_languages,
        linebreaks.unwrap_or(settings.ocr_linebreaks),
    )?;
    Ok(present(runner, &value, settings, false))
}

/// Extract an image's text and present it, capturing first when `path` is empty.
///
/// This is the worker body behind the QML completion path: a saved file goes
/// straight to [`recognize`], while the no-argument `tidyshot ocr` selects a
/// region, grabs it, and recognizes that frame, matching the source's
/// capture-then-OCR flow.
// Ported from omnishot/app.py (recognize_text)
fn run_recognize(path: &Path, linebreaks: i32) -> Result<Presentation, TextError> {
    let runner = crate::commands::SystemRunner;
    let qr = CommandQrReader::new(runner, "zbarimg");
    let settings = crate::settings::ocr_settings();
    let override_linebreaks = match linebreaks {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    };
    recognize(&runner, &qr, path, &settings, override_linebreaks)
}

/// Select a region, capture it to a temporary PNG, then recognize its text.
fn capture_then_recognize(linebreaks: i32) -> Result<Presentation, TextError> {
    let png = capture_selection()?;
    let temporary = std::env::temp_dir().join(format!(
        "tidyshot-ocr-{}-{}.png",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ));
    std::fs::write(&temporary, &png).map_err(|error| TextError {
        message: error.to_string(),
    })?;
    let outcome = run_recognize(&temporary, linebreaks);
    let _ = std::fs::remove_file(&temporary);
    outcome
}

/// Run the interactive selector and grab the chosen region.
///
/// `ocr` with no file uses the same interactive selection as the area command:
/// slurp runs the selector, then grim grabs the chosen region. A dismissed
/// selector is reported rather than swallowed.
fn capture_selection() -> Result<Vec<u8>, TextError> {
    use crate::capture_session::{CaptureSession, slurp_region};
    let runner = crate::commands::SystemRunner;
    let session = CaptureSession {
        runner: &runner,
        mirror: crate::mirror::MirrorRegistry::global(),
        native: crate::recording::native_dir(),
    };
    let rect = slurp_region(&runner, false, &[]).map_err(|error| TextError {
        message: error.to_string(),
    })?;
    session
        .grab(Some(rect), false, None, None)
        .map_err(|error| TextError {
            message: error.to_string(),
        })
}

// --- QML bridge: the recognized-text dialog ------------------------------

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qstringlist.h");
        type QStringList = cxx_qt_lib::QStringList;
    }

    extern "RustQt" {
        /// The recognized-text dialog exposed to QML.
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        #[qproperty(QString, text)]
        #[qproperty(QString, status)]
        #[qproperty(QStringList, link_labels, cxx_name = "linkLabels")]
        #[qproperty(QStringList, link_urls, cxx_name = "linkUrls")]
        #[qproperty(i32, link_count, cxx_name = "linkCount")]
        #[qproperty(bool, copied)]
        #[qproperty(bool, can_open, cxx_name = "canOpen")]
        #[qproperty(bool, detect)]
        #[qproperty(bool, shown)]
        type TextOutput = super::TextResultDialogRust;

        /// Replace the edited text.
        #[qinvokable]
        #[cxx_name = "updateText"]
        fn update_text(self: Pin<&mut Self>, text: &QString);

        /// Run the OCR/QR pipeline over `path` and present the result.
        ///
        /// `linebreaks` is the `ocr-lines` / `ocr-single-line` override: -1
        /// keeps the saved preference, 0 forces one line, 1 forces line
        /// breaks.
        #[qinvokable]
        #[cxx_name = "recognize"]
        fn recognize_path(self: Pin<&mut Self>, path: &QString, linebreaks: i32);

        /// Select a region, capture it, then recognize its text.
        ///
        /// This is the no-path `tidyshot ocr` / `capture-text` flow: the
        /// interactive selector runs first and its frame feeds the same
        /// completion path as a file.
        #[qinvokable]
        #[cxx_name = "captureAndRecognize"]
        fn capture_and_recognize(self: Pin<&mut Self>, linebreaks: i32);

        /// Close the review dialog and hide it.
        #[qinvokable]
        fn dismiss(self: Pin<&mut Self>);

        /// Copy the current text to the clipboard.
        #[qinvokable]
        fn copy(self: Pin<&mut Self>);

        /// The URL of link `row`, empty when out of range.
        #[qinvokable]
        #[cxx_name = "linkUrl"]
        fn link_url(self: Pin<&mut Self>, row: i32) -> QString;
    }

    impl cxx_qt::Threading for TextOutput {}
}

use core::pin::Pin;
use qobject::{QString, QStringList};

/// Rust state behind the QML recognized-text dialog.
///
/// Every mutable field is a QML property, because cxx-qt exposes pinned
/// objects without `DerefMut`. The dialog keeps the link projections in sync
/// whenever the text changes.
pub struct TextResultDialogRust {
    text: QString,
    status: QString,
    link_labels: QStringList,
    link_urls: QStringList,
    link_count: i32,
    copied: bool,
    can_open: bool,
    detect: bool,
    shown: bool,
}

impl Default for TextResultDialogRust {
    fn default() -> Self {
        Self {
            text: QString::from(""),
            status: QString::from(TextResult::REVIEW),
            link_labels: QStringList::default(),
            link_urls: QStringList::default(),
            link_count: 0,
            copied: false,
            can_open: false,
            detect: true,
            shown: false,
        }
    }
}

impl qobject::TextOutput {
    /// Replace the edited text and refresh the link projections.
    pub fn update_text(mut self: Pin<&mut Self>, text: &QString) {
        self.as_mut().set_text(text.clone());
        self.as_mut().set_copied(false);
        self.as_mut().set_status(QString::from(TextResult::EDITED));
        let text = text.to_string();
        let links = if *self.detect() {
            detected_links(&text)
        } else {
            Vec::new()
        };
        self.as_mut().project_links(&links);
    }

    /// Run the OCR/QR pipeline over `path` and present the result.
    ///
    /// This is the completion end of `tidyshot ocr <path>` and the
    /// `capture-text` URL: extraction copies an ordinary result and opens
    /// this dialog only when there are links to act on or an error to show.
    ///
    /// The pipeline shells out to tesseract and the QR reader, so it runs on a
    /// worker thread and delivers its outcome back to the Qt thread; running
    /// it here would freeze the window for the length of both OCR passes.
    pub fn recognize_path(self: Pin<&mut Self>, path: &QString, linebreaks: i32) {
        use cxx_qt::Threading as _;
        let path = path.to_string();
        let qt_thread = self.qt_thread();
        std::thread::spawn(move || {
            let outcome = run_recognize(Path::new(&path), linebreaks);
            let completion = complete(outcome);
            let _ = qt_thread.queue(move |mut qobject| {
                qobject.as_mut().apply_completion(completion);
            });
        });
    }

    /// Select a region, grab it, then recognize the frame's text.
    ///
    /// Both the capture and the OCR run off the Qt thread; the outcome is
    /// delivered through the same completion path as a file.
    pub fn capture_and_recognize(self: Pin<&mut Self>, linebreaks: i32) {
        use cxx_qt::Threading as _;
        let qt_thread = self.qt_thread();
        std::thread::spawn(move || {
            let outcome = capture_then_recognize(linebreaks);
            let completion = complete(outcome);
            let _ = qt_thread.queue(move |mut qobject| {
                qobject.as_mut().apply_completion(completion);
            });
        });
    }

    /// Apply a finished recognition to the dialog without freezing the UI.
    fn apply_completion(mut self: Pin<&mut Self>, completion: Completion) {
        self.as_mut()
            .set_status(QString::from(completion.status.clone()));
        match completion.result {
            Some(result) => self.as_mut().apply_result(&result),
            None => {
                // An error or an empty read must not leave the previous run's
                // text and links under the new status.
                self.as_mut().reset_state();
                self.as_mut().set_shown(completion.shown);
            }
        }
    }

    /// Clear the reviewable result while keeping the current detection mode.
    fn reset_state(mut self: Pin<&mut Self>) {
        self.as_mut().set_text(QString::default());
        self.as_mut().set_copied(false);
        self.as_mut().set_shown(false);
        self.as_mut().project_links(&[]);
    }

    /// Load a presented result into the dialog and show it.
    fn apply_result(mut self: Pin<&mut Self>, result: &TextResult) {
        self.as_mut()
            .set_text(QString::from(result.text().to_owned()));
        self.as_mut()
            .set_status(QString::from(result.status().to_owned()));
        self.as_mut().set_copied(result.copied());
        self.as_mut().set_detect(result.detect());
        let links = result.links();
        self.as_mut().project_links(&links);
        self.as_mut().set_shown(true);
    }

    /// Project `links` into the dialog's parallel label and URL lists.
    fn project_links(mut self: Pin<&mut Self>, links: &[Link]) {
        let (labels, urls) = link_projection(links);
        let mut label_list = QStringList::default();
        let mut url_list = QStringList::default();
        for label in labels {
            label_list.append(QString::from(label));
        }
        for url in urls {
            url_list.append(QString::from(url));
        }
        self.as_mut().set_link_labels(label_list);
        self.as_mut().set_link_urls(url_list);
        self.as_mut().set_link_count(links.len() as i32);
        self.as_mut().set_can_open(!links.is_empty());
    }

    /// Close the review dialog.
    pub fn dismiss(mut self: Pin<&mut Self>) {
        self.as_mut().set_shown(false);
    }

    /// Copy the current text through `wl-copy`.
    pub fn copy(mut self: Pin<&mut Self>) {
        let value = self.text().to_string();
        let spec = crate::commands::clipboard_spec("text/plain;charset=utf-8", value.into_bytes());
        match crate::commands::SystemRunner.run(&spec) {
            Ok(_) => {
                self.as_mut().set_copied(true);
                self.as_mut().set_status(QString::from(TextResult::COPIED));
            }
            Err(error) => {
                self.as_mut()
                    .set_status(QString::from(format!("Could not copy text: {error}")));
            }
        }
    }

    /// The URL of link `row`, or an empty string.
    pub fn link_url(self: Pin<&mut Self>, row: i32) -> QString {
        let urls = self.link_urls();
        if row < 0 {
            return QString::default();
        }
        urls.get(row as isize)
            .map_or_else(QString::default, |url| url.clone())
    }
}

#[cfg(test)]
mod bridge_tests {
    use super::*;

    #[test]
    fn default_dialog_starts_empty_and_detecting() {
        let dialog = TextResultDialogRust::default();
        assert_eq!(dialog.text.to_string(), "");
        assert_eq!(dialog.status.to_string(), TextResult::REVIEW);
        assert!(dialog.detect);
        assert!(!dialog.can_open);
        assert!(!dialog.shown);
        assert_eq!(dialog.link_labels.len(), 0);
        assert_eq!(dialog.link_count, 0);
    }

    #[test]
    fn link_projection_keeps_spelling_and_order_parallel() {
        let links = vec![
            Link {
                label: "www.example.org".to_owned(),
                url: "https://www.example.org".to_owned(),
            },
            Link {
                label: "https://example.com/a".to_owned(),
                url: "https://example.com/a".to_owned(),
            },
        ];
        let (labels, urls) = link_projection(&links);
        assert_eq!(labels, vec!["www.example.org", "https://example.com/a"]);
        assert_eq!(
            urls,
            vec!["https://www.example.org", "https://example.com/a"]
        );
        assert_eq!(link_projection(&[]), (Vec::new(), Vec::new()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{CommandError, CommandOutput};
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeRunner {
        results: Mutex<Vec<Result<CommandOutput, CommandError>>>,
        seen: Mutex<Vec<CommandSpec>>,
    }

    impl FakeRunner {
        fn push(&self, stdout: &str) {
            self.results.lock().unwrap().push(Ok(CommandOutput {
                status: 0,
                stdout: stdout.as_bytes().to_vec(),
                stderr: Vec::new(),
            }));
        }

        fn fail(&self, program: &str, stderr: &str) {
            self.results
                .lock()
                .unwrap()
                .push(Err(CommandError::Failure {
                    program: program.to_owned(),
                    output: CommandOutput {
                        status: 1,
                        stdout: Vec::new(),
                        stderr: stderr.as_bytes().to_vec(),
                    },
                }));
        }
    }

    impl CommandRunner for FakeRunner {
        fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, CommandError> {
            self.seen.lock().unwrap().push(spec.clone());
            self.results.lock().unwrap().remove(0)
        }
    }

    struct FakeQr(Mutex<Result<String, String>>);

    impl FakeQr {
        fn returning(value: &str) -> Self {
            Self(Mutex::new(Ok(value.to_owned())))
        }
    }

    impl QrReader for FakeQr {
        fn read(&self, _path: &Path) -> Result<String, String> {
            self.0.lock().unwrap().clone()
        }
    }

    fn link(label: &str, url: &str) -> Link {
        Link {
            label: label.to_owned(),
            url: url.to_owned(),
        }
    }

    #[test]
    fn link_detection_preserves_queries_and_balanced_parentheses() {
        let text = "Visit https://example.com/a_(b)?x=1&y=2. Then (www.example.org/path). example.net/docs#part and https://example.com/a_(b)?x=1&y=2.";
        assert_eq!(
            detected_links(text),
            vec![
                link(
                    "https://example.com/a_(b)?x=1&y=2",
                    "https://example.com/a_(b)?x=1&y=2"
                ),
                link("www.example.org/path", "https://www.example.org/path"),
                link("example.net/docs#part", "https://example.net/docs#part"),
            ]
        );
        assert_eq!(
            detected_links("http://localhost:8000/path https://[::1]:8443/a https://例子.中国/页"),
            vec![
                link("http://localhost:8000/path", "http://localhost:8000/path"),
                link("https://[::1]:8443/a", "https://[::1]:8443/a"),
                link("https://例子.中国/页", "https://例子.中国/页"),
            ]
        );
        assert_eq!(
            detected_links("例子.中国/页"),
            vec![link("例子.中国/页", "https://例子.中国/页")]
        );
    }

    #[test]
    fn link_detection_ignores_non_web_actions_and_malformed_urls() {
        let text = "file:///tmp/screenshot.png javascript:alert(1) mailto:me@example.com person@sub.example.com image.png example.notarealtld https://[broken https://example.com:99999";
        assert!(
            detected_links(text).is_empty(),
            "{:?}",
            detected_links(text)
        );
    }

    #[test]
    fn scheme_less_links_stop_at_punctuation_and_ports() {
        assert_eq!(
            detected_links("see example.com."),
            vec![link("example.com", "https://example.com")]
        );
        assert_eq!(
            detected_links("a.b.c.d.example.com"),
            vec![link("a.b.c.d.example.com", "https://a.b.c.d.example.com")]
        );
        assert_eq!(
            detected_links("example.com:8080/x?y=1#z"),
            vec![link(
                "example.com:8080/x?y=1#z",
                "https://example.com:8080/x?y=1#z"
            )]
        );
        // A zero port is malformed and a trailing numeric label is not a host.
        assert!(detected_links("example.com:0").is_empty());
    }

    #[test]
    fn qr_is_read_when_ocr_is_noisy_empty_or_unavailable() {
        for text in ["noise", "", "https://example.com/qr"] {
            let runner = FakeRunner::default();
            // An empty first pass retries with sparse-text mode.
            runner.push(text);
            runner.push(text);
            let qr = FakeQr::returning("https://example.com/qr\nhttps://example.org/other");
            let result = extract(&runner, &qr, Path::new("generated.png"), "eng", true).unwrap();
            assert_eq!(
                result.matches("https://example.com/qr").count(),
                1,
                "{result}"
            );
            assert!(result.contains("https://example.org/other"), "{result}");
        }

        let runner = FakeRunner::default();
        runner.fail("tesseract", "OCR language unavailable");
        let qr = FakeQr::returning("https://example.com/qr\nhttps://example.org/other");
        assert_eq!(
            extract(&runner, &qr, Path::new("qr.png"), "eng", true).unwrap(),
            "https://example.com/qr\n\nhttps://example.org/other"
        );

        // Both halves empty, OCR having failed: the OCR error is surfaced.
        let runner = FakeRunner::default();
        runner.fail("tesseract", "OCR language unavailable");
        let qr = FakeQr::returning("");
        let error = extract(&runner, &qr, Path::new("empty.png"), "eng", true).unwrap_err();
        assert!(error.message().contains("language"), "{error}");
    }

    #[test]
    fn qr_values_already_in_the_text_are_not_repeated() {
        let runner = FakeRunner::default();
        runner.push("https://example.com/qr?different=1");
        let qr = FakeQr::returning("https://example.com/qr");
        let result = extract(&runner, &qr, Path::new("different-links.png"), "eng", true).unwrap();
        assert_eq!(
            result.lines().collect::<Vec<_>>(),
            vec![
                "https://example.com/qr?different=1",
                "",
                "https://example.com/qr"
            ]
        );
    }

    #[test]
    fn result_preference_editing_and_clipboard_failure() {
        let runner = FakeRunner::default();
        runner.push("");
        let text = "https://example.com/capture";
        let settings = TextSettings {
            ocr_detect_links: true,
            ..TextSettings::default()
        };
        let Presentation::Dialog(mut dialog) = present(&runner, text, &settings, false) else {
            panic!("a link should open the dialog");
        };
        assert!(dialog.copied());
        assert_eq!(dialog.links().len(), 1);

        dialog.set_text("edited text");
        assert!(dialog.links().is_empty());
        assert!(!dialog.copied());

        let runner = FakeRunner::default();
        runner.push("");
        dialog.copy(&runner).unwrap();
        assert!(dialog.copied());

        // Detection off and text copied cleanly: no dialog.
        let runner = FakeRunner::default();
        runner.push("");
        let settings = TextSettings {
            ocr_detect_links: false,
            ..TextSettings::default()
        };
        assert_eq!(
            present(&runner, text, &settings, false),
            Presentation::Copied
        );

        // Nothing recognized: nothing shown or copied.
        let runner = FakeRunner::default();
        assert_eq!(present(&runner, "", &settings, false), Presentation::Empty);
        assert!(runner.seen.lock().unwrap().is_empty());

        // Review forces a dialog and skips the copy.
        let runner = FakeRunner::default();
        let Presentation::Dialog(dialog) = present(&runner, "review", &settings, true) else {
            panic!("review should open the dialog");
        };
        assert!(!dialog.copied());
        assert!(runner.seen.lock().unwrap().is_empty());

        // Clipboard failure keeps the text and reports the error.
        let runner = FakeRunner::default();
        runner.fail("wl-copy", "Clipboard unavailable");
        let Presentation::Dialog(dialog) = present(&runner, "keep this text", &settings, false)
        else {
            panic!("a copy failure should open the dialog");
        };
        assert_eq!(dialog.text(), "keep this text");
        assert!(
            dialog.status().contains("Could not copy"),
            "{}",
            dialog.status()
        );
    }

    #[test]
    fn explicit_open_only_returns_a_real_link() {
        let dialog = TextResult::new("see https://example.com/x now", true, false);
        assert_eq!(dialog.open(0).as_deref(), Some("https://example.com/x"));
        assert_eq!(dialog.open(1), None);
        let no_detect = TextResult::new("see https://example.com/x now", false, false);
        assert_eq!(no_detect.open(0), None);
        assert!(!no_detect.can_open());
    }

    #[test]
    fn recognize_extracts_then_copies_or_opens_the_dialog() {
        // Detection off and a clean copy: the whole path is a copy action.
        let runner = FakeRunner::default();
        runner.push("hello");
        runner.push("");
        let qr = FakeQr::returning("");
        let settings = TextSettings {
            ocr_languages: "eng".to_owned(),
            ocr_detect_links: false,
            ..TextSettings::default()
        };
        assert_eq!(
            recognize(&runner, &qr, Path::new("shot.png"), &settings, None).unwrap(),
            Presentation::Copied
        );
        assert!(
            runner
                .seen
                .lock()
                .unwrap()
                .iter()
                .any(|s| s.program == "wl-copy")
        );

        // A line-break override reaches the OCR command.
        let runner = FakeRunner::default();
        runner.push("Silver meadow\nAmber sunrise");
        runner.push("");
        let qr = FakeQr::returning("");
        let linebreaks_off = TextSettings {
            ocr_languages: "eng".to_owned(),
            ocr_detect_links: false,
            ..TextSettings::default()
        };
        recognize(
            &runner,
            &qr,
            Path::new("shot.png"),
            &linebreaks_off,
            Some(false),
        )
        .unwrap();
        let seen = runner.seen.lock().unwrap();
        assert!(seen.iter().any(|spec| {
            spec.program == "wl-copy"
                && String::from_utf8_lossy(spec.stdin.as_deref().unwrap())
                    .contains("Silver meadow Amber sunrise")
        }));
    }

    #[test]
    fn settings_default_and_json_override() {
        assert_eq!(TextSettings::default().ocr_languages, ocr::AUTO);
        assert!(TextSettings::default().ocr_linebreaks);
        assert!(TextSettings::default().ocr_detect_links);

        let value = crate::json::parse(
            r#"{"ocr_languages":"eng+spa","ocr_linebreaks":false,"ocr_detect_links":false}"#,
        )
        .unwrap();
        let settings = TextSettings::from_value(&value);
        assert_eq!(settings.ocr_languages, "eng+spa");
        assert!(!settings.ocr_linebreaks);
        assert!(!settings.ocr_detect_links);

        let partial =
            TextSettings::from_value(&crate::json::parse("{\"ocr_languages\":\"\"}").unwrap());
        assert_eq!(partial.ocr_languages, ocr::AUTO);
        assert!(partial.ocr_linebreaks);
    }

    #[test]
    fn completion_opens_only_on_links_or_errors() {
        // A clean copy stays out of the way.
        let copied = complete(Ok(Presentation::Copied));
        assert!(!copied.shown);
        assert!(copied.result.is_none());
        assert_eq!(copied.status, TextResult::COPIED);

        // A reviewable result opens the dialog with its own state.
        let dialog = complete(Ok(Presentation::Dialog(TextResult::new(
            "see https://example.com",
            true,
            false,
        ))));
        assert!(dialog.shown);
        let result = dialog.result.expect("the review result must carry through");
        assert_eq!(result.text(), "see https://example.com");
        assert!(result.can_open());

        // Nothing recognized produces no dialog and no stale result.
        let empty = complete(Ok(Presentation::Empty));
        assert!(!empty.shown);
        assert!(empty.result.is_none());
        assert!(empty.status.contains("No text"));

        // An error is surfaced, not swallowed.
        let failed = complete(Err(TextError {
            message: "OCR language unavailable".to_owned(),
        }));
        assert!(failed.shown);
        assert!(failed.result.is_none());
        assert!(failed.status.contains("language unavailable"));
    }
}
