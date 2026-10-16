//! Offline OCR with local language packs and script detection.
// Ported from omnishot/ocr.py
//!
//! Recognition shells out to `tesseract` through the shared command runner so
//! the pipeline is testable without the binary. The language table and script
//! map match the model set OmniShot installs; an explicit language list is
//! validated before it reaches the command line.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::commands::{CommandError, CommandRunner, CommandSpec};

/// The language packs offered by name.
pub const LANGUAGES: &[(&str, &str)] = &[
    ("eng", "English"),
    ("spa", "Spanish"),
    ("fra", "French"),
    ("deu", "German"),
    ("ita", "Italian"),
    ("por", "Portuguese"),
    ("nld", "Dutch"),
    ("dan", "Danish"),
    ("swe", "Swedish"),
    ("nor", "Norwegian"),
    ("fin", "Finnish"),
    ("isl", "Icelandic"),
    ("pol", "Polish"),
    ("ces", "Czech"),
    ("slk", "Slovak"),
    ("hun", "Hungarian"),
    ("ron", "Romanian"),
    ("tur", "Turkish"),
    ("vie", "Vietnamese"),
    ("ind", "Indonesian"),
    ("msa", "Malay"),
    ("cat", "Catalan"),
    ("hrv", "Croatian"),
    ("slv", "Slovenian"),
    ("ell", "Greek"),
    ("rus", "Russian"),
    ("ukr", "Ukrainian"),
    ("bul", "Bulgarian"),
    ("ara", "Arabic"),
    ("heb", "Hebrew"),
    ("hin", "Hindi"),
    ("chi_sim", "Chinese (simplified)"),
    ("chi_tra", "Chinese (traditional)"),
    ("jpn", "Japanese"),
    ("kor", "Korean"),
    ("tha", "Thai"),
];

/// OSD script names mapped to the script model tesseract expects.
pub const SCRIPTS: &[(&str, &str)] = &[
    ("Latin", "Latin"),
    ("Cyrillic", "Cyrillic"),
    ("Greek", "Greek"),
    ("Arabic", "Arabic"),
    ("Hebrew", "Hebrew"),
    ("Devanagari", "Devanagari"),
    ("Han", "HanS"),
    ("Japanese", "Japanese"),
    ("Hangul", "Hangul"),
    ("Thai", "Thai"),
];

/// The tessdata_fast release the language packs are installed from.
pub const MODEL_VERSION: &str = "4.1.0";

/// The language shown in the UI when no explicit list is chosen.
pub const AUTO: &str = "auto";

/// Why OCR could not return text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcrError {
    message: String,
}

impl OcrError {
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for OcrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for OcrError {}

fn failure(detail: impl std::fmt::Display) -> OcrError {
    OcrError {
        message: detail.to_string(),
    }
}

impl From<CommandError> for OcrError {
    fn from(error: CommandError) -> Self {
        failure(characterise(&error))
    }
}

/// The tessdata directory under the user's data home.
///
/// `XDG_DATA_HOME` wins, then `$HOME/.local/share`; the directory name keeps
/// parity with the Python app so an existing model install is reused.
#[must_use]
pub fn model_dir() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from(".local/share"));
    base.join("tidyshot").join("tessdata")
}

/// The ordered language codes, without the display names.
#[must_use]
pub fn language_codes() -> Vec<&'static str> {
    LANGUAGES.iter().map(|(code, _)| *code).collect()
}

/// Whether a language list follows `eng`, `eng+spa`, or `chi_sim` syntax.
#[must_use]
pub fn valid_languages(languages: &str) -> bool {
    if languages.is_empty() {
        return false;
    }
    languages
        .split('+')
        .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_alphabetic() || c == '_'))
}

/// Read an image and return its recognized text.
///
/// `languages` is either [`AUTO`] or a `+`-joined list of codes. With auto and
/// a local model install the orientation/script detector picks the script
/// model; otherwise recognition falls back to English. `linebreaks` off
/// collapses all whitespace to single spaces.
// Ported from omnishot/ocr.py (recognize)
pub fn recognize<R: CommandRunner>(
    runner: &R,
    path: &Path,
    languages: &str,
    linebreaks: bool,
) -> Result<String, OcrError> {
    let root = model_dir();
    let local = root.join("eng.traineddata").is_file();
    let mut extra: Vec<String> = Vec::new();
    if local {
        extra.push("--tessdata-dir".to_owned());
        extra.push(root.to_string_lossy().into_owned());
    }

    let chosen = if languages == AUTO {
        let script = detect_script(runner, path, &extra).unwrap_or_else(|| "Latin".to_owned());
        let model = format!(
            "script/{}",
            SCRIPTS
                .iter()
                .find(|(name, _)| *name == script)
                .map_or("Latin", |(_, model)| *model)
        );
        if local && root.join(format!("{model}.traineddata")).is_file() {
            model
        } else {
            "eng".to_owned()
        }
    } else {
        if !valid_languages(languages) {
            return Err(failure("Choose Auto or language codes such as eng+spa"));
        }
        languages.to_owned()
    };

    let value = run_psm(runner, path, &chosen, &extra, "3")?;
    let value = if value.is_empty() {
        run_psm(runner, path, &chosen, &extra, "11")?
    } else {
        value
    };
    if linebreaks {
        Ok(value)
    } else {
        Ok(collapse_whitespace(&value))
    }
}

/// Ask tesseract's OSD mode which script the image uses.
///
/// A short selection often has too few characters for OSD; that is not an
/// error, it just means auto mode falls back to Latin.
fn detect_script<R: CommandRunner>(runner: &R, path: &Path, extra: &[String]) -> Option<String> {
    let spec = ocr_spec(path, extra, "osd")
        .args(["--psm", "0"])
        .timeout(Duration::from_secs(20));
    let output = runner.run(&spec).ok()?;
    let text = output.stdout_string();
    parse_script(&text)
}

/// The `Script: <name>` line of an OSD report.
fn parse_script(report: &str) -> Option<String> {
    report.lines().find_map(|line| {
        line.strip_prefix("Script:")
            .map(|rest| rest.trim().to_owned())
            .filter(|name| !name.is_empty())
    })
}

/// Run tesseract in page-segmentation mode `psm`, returning trimmed text.
fn run_psm<R: CommandRunner>(
    runner: &R,
    path: &Path,
    languages: &str,
    extra: &[String],
    psm: &str,
) -> Result<String, OcrError> {
    let spec = ocr_spec(path, extra, languages)
        .args(["--psm", psm])
        .timeout(Duration::from_secs(90));
    let output = runner.run(&spec)?;
    Ok(output.stdout_string().trim().to_owned())
}

/// The common `tesseract <path> stdout [-l lang]` prefix.
fn ocr_spec(path: &Path, extra: &[String], languages: &str) -> CommandSpec {
    let mut spec = CommandSpec::new("tesseract");
    spec.args.push(path.to_string_lossy().into_owned());
    spec.args.push("stdout".to_owned());
    spec.args.extend(extra.iter().cloned());
    spec.args.push("-l".to_owned());
    spec.args.push(languages.to_owned());
    spec
}

/// Collapse every whitespace run to a single space, as `" ".join(text.split())`.
fn collapse_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The stderr or stdout text of a failed command, trimmed.
fn characterise(error: &CommandError) -> String {
    match error {
        CommandError::Failure { output, .. } => {
            let stderr = output.stderr_string();
            let stdout = output.stdout_string();
            let detail = if stderr.trim().is_empty() {
                stdout
            } else {
                stderr
            };
            let detail = detail.trim();
            if detail.is_empty() {
                "tesseract failed".to_owned()
            } else {
                detail.to_owned()
            }
        }
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::CommandOutput;
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
    }

    impl CommandRunner for FakeRunner {
        fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, CommandError> {
            self.seen.lock().unwrap().push(spec.clone());
            self.results.lock().unwrap().remove(0)
        }
    }

    #[test]
    fn explicit_languages_run_one_recognition_pass() {
        let runner = FakeRunner::default();
        runner.push("Silver meadow\nAmber sunrise\n");
        let text = recognize(&runner, Path::new("shot.png"), "eng+spa", true).unwrap();
        assert_eq!(text, "Silver meadow\nAmber sunrise");
        let seen = runner.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].program, "tesseract");
        assert!(seen[0].args.contains(&"eng+spa".to_owned()));
        assert!(seen[0].args.windows(2).any(|pair| pair == ["--psm", "3"]));
    }

    #[test]
    fn linebreaks_off_collapses_whitespace() {
        let runner = FakeRunner::default();
        runner.push("Silver meadow\n   Amber   sunrise");
        let text = recognize(&runner, Path::new("shot.png"), "eng", false).unwrap();
        assert_eq!(text, "Silver meadow Amber sunrise");
    }

    #[test]
    fn invalid_language_codes_are_rejected_before_running() {
        let runner = FakeRunner::default();
        let error = recognize(&runner, Path::new("shot.png"), "eng+bad code", true).unwrap_err();
        assert!(error.message().contains("eng+spa"), "{error}");
        assert!(runner.seen.lock().unwrap().is_empty());
    }

    #[test]
    fn auto_mode_detects_a_script_model_then_recognizes() {
        // The fixture data home has no local models, so auto mode still runs
        // OSD first and then recognizes with the English fallback.
        let runner = FakeRunner::default();
        runner.push("Script: HanS\nOrientation in degrees: 0\n");
        runner.push("recognized");
        let text = recognize(&runner, Path::new("shot.png"), AUTO, true).unwrap();
        assert_eq!(text, "recognized");
        let seen = runner.seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert!(seen[0].args.contains(&"osd".to_owned()));
        assert!(seen[1].args.contains(&"eng".to_owned()));
    }

    #[test]
    fn a_short_selection_falls_back_when_osd_reports_nothing() {
        let runner = FakeRunner::default();
        runner.push("");
        runner.push("hi");
        assert_eq!(
            recognize(&runner, Path::new("tiny.png"), AUTO, true).unwrap(),
            "hi"
        );
    }

    #[test]
    fn empty_first_pass_retries_with_sparse_text() {
        let runner = FakeRunner::default();
        runner.push("   ");
        runner.push("found on retry");
        let text = recognize(&runner, Path::new("sparse.png"), "eng", true).unwrap();
        assert_eq!(text, "found on retry");
        let seen = runner.seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert!(seen[1].args.windows(2).any(|pair| pair == ["--psm", "11"]));
    }

    #[test]
    fn a_failing_command_is_reported_with_its_stderr() {
        let runner = FakeRunner::default();
        runner
            .results
            .lock()
            .unwrap()
            .push(Err(CommandError::Failure {
                program: "tesseract".into(),
                output: CommandOutput {
                    status: 1,
                    stdout: Vec::new(),
                    stderr: b"Error opening data file".to_vec(),
                },
            }));
        let error = recognize(&runner, Path::new("shot.png"), "eng", true).unwrap_err();
        assert!(error.message().contains("Error opening data file"));
    }

    #[test]
    fn language_table_and_validation_match_the_source() {
        assert_eq!(language_codes().len(), 36);
        assert!(valid_languages("chi_sim"));
        assert!(valid_languages("eng+spa+chi_tra"));
        assert!(!valid_languages(""));
        assert!(!valid_languages("eng+"));
        assert!(!valid_languages("eng spa"));
    }
}
