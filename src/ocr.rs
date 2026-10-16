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
/// `XDG_DATA_HOME` wins, then `$HOME/.local/share`.
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

/// Every model file the installer manages, relative to the tessdata root.
///
/// Order matches the source: languages, the OSD pack, then each script model.
// Ported from omnishot/ocr.py (install_models)
#[must_use]
pub fn model_names() -> Vec<String> {
    let mut names: Vec<String> = LANGUAGES
        .iter()
        .map(|(code, _)| (*code).to_owned())
        .collect();
    names.push("osd".to_owned());
    for (_, model) in SCRIPTS {
        names.push(format!("script/{model}"));
    }
    names
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

/// Download one model file by URL.
///
/// A [`CommandFetcher`] wraps the shared runner (`curl -fsSL <url>`); tests
/// substitute a fake so install logic is asserted without a network.
pub trait ModelFetcher {
    fn fetch(&self, url: &str) -> Result<Vec<u8>, String>;
}

/// A [`ModelFetcher`] that shells out to `curl` through the command runner.
pub struct CommandFetcher<R> {
    runner: R,
}

impl<R> CommandFetcher<R> {
    #[must_use]
    pub fn new(runner: R) -> Self {
        Self { runner }
    }
}

impl<R: CommandRunner> ModelFetcher for CommandFetcher<R> {
    fn fetch(&self, url: &str) -> Result<Vec<u8>, String> {
        let spec = CommandSpec::new("curl")
            .args(["-fsSL"])
            .arg(url)
            .timeout(Duration::from_secs(60));
        self.runner
            .run(&spec)
            .map(|output| output.stdout)
            .map_err(|error| error.to_string())
    }
}

/// Install every language, OSD, and script model into the tessdata root.
///
/// A file already present with a plausible size is kept; a truncated download
/// and a suspiciously small model both fail rather than replacing a good file.
/// `progress` receives `(completed, total)` after each file. Returns the number
/// of language packs, matching upstream's return value.
// Ported from omnishot/ocr.py (install_models)
pub fn install_models<F: ModelFetcher>(
    fetcher: &F,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<usize, OcrError> {
    install_models_at(&model_dir(), fetcher, progress)
}

/// The install body over an explicit root, so tests stay off the home dir.
fn install_models_at<F: ModelFetcher>(
    root: &Path,
    fetcher: &F,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<usize, OcrError> {
    std::fs::create_dir_all(root).map_err(|error| failure(error.to_string()))?;
    let names = model_names();
    let total = names.len();
    let mut hashes: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for (completed, name) in names.iter().enumerate() {
        let path = root.join(format!("{name}.traineddata"));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| failure(error.to_string()))?;
        }
        let reusable = path
            .metadata()
            .map(|meta| meta.len() != 32_000_000)
            .unwrap_or(false);
        if !reusable {
            let url = format!(
                "https://raw.githubusercontent.com/tesseract-ocr/tessdata_fast/{MODEL_VERSION}/{name}.traineddata"
            );
            let data = fetcher.fetch(&url).map_err(failure)?;
            if data.len() > 200_000_000 {
                return Err(failure(format!(
                    "Incomplete language model download: {name}"
                )));
            }
            if data.len() < 1000 {
                return Err(failure(format!("Invalid language model: {name}")));
            }
            let temporary = path.with_extension("download");
            std::fs::write(&temporary, &data).map_err(|error| failure(error.to_string()))?;
            std::fs::rename(&temporary, &path).map_err(|error| failure(error.to_string()))?;
        }
        let bytes = std::fs::read(&path).map_err(|error| failure(error.to_string()))?;
        hashes.insert(name.clone(), sha256_hex(&bytes));
        progress(completed + 1, total);
    }
    let manifest = crate::json::Value::Object(
        [
            (
                "source".to_owned(),
                crate::json::Value::String("tesseract-ocr/tessdata_fast".to_owned()),
            ),
            (
                "version".to_owned(),
                crate::json::Value::String(crate::ocr::MODEL_VERSION.to_owned()),
            ),
            (
                "sha256".to_owned(),
                crate::json::Value::Object(
                    hashes
                        .into_iter()
                        .map(|(name, digest)| (name, crate::json::Value::String(digest)))
                        .collect(),
                ),
            ),
        ]
        .into_iter()
        .collect(),
    );
    let manifest_path = root.join("manifest.json");
    crate::storage::atomic_write_file(&manifest_path, manifest.to_string_pretty().as_bytes())
        .map_err(|error| failure(error.to_string()))?;
    Ok(LANGUAGES.len())
}

/// The lowercase hex SHA-256 of `data`.
///
/// Kept inline because hashing model files is the only use and the project
/// vendors no digest crate.
#[must_use]
pub fn sha256_hex(data: &[u8]) -> String {
    let digest = sha256(data);
    let mut out = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// SHA-256, from FIPS 180-4, used only to record model checksums.
fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut message = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in message.as_chunks::<64>().0 {
        let mut w = [0u32; 64];
        for (index, word) in chunk.as_chunks::<4>().0.iter().enumerate() {
            w[index] = u32::from_be_bytes(*word);
        }
        for index in 16..64 {
            let s0 = w[index - 15].rotate_right(7)
                ^ w[index - 15].rotate_right(18)
                ^ (w[index - 15] >> 3);
            let s1 = w[index - 2].rotate_right(17)
                ^ w[index - 2].rotate_right(19)
                ^ (w[index - 2] >> 10);
            w[index] = w[index - 16]
                .wrapping_add(s0)
                .wrapping_add(w[index - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
        for index in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[index])
                .wrapping_add(w[index]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        state[0] = state[0].wrapping_add(a);
        state[1] = state[1].wrapping_add(b);
        state[2] = state[2].wrapping_add(c);
        state[3] = state[3].wrapping_add(d);
        state[4] = state[4].wrapping_add(e);
        state[5] = state[5].wrapping_add(f);
        state[6] = state[6].wrapping_add(g);
        state[7] = state[7].wrapping_add(h);
    }
    let mut digest = [0u8; 32];
    for (index, word) in state.iter().enumerate() {
        digest[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    digest
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

    #[test]
    fn model_names_cover_languages_osd_and_scripts() {
        let names = model_names();
        assert!(names.contains(&"eng".to_owned()));
        assert!(names.contains(&"chi_sim".to_owned()));
        assert!(names.contains(&"osd".to_owned()));
        assert!(names.contains(&"script/HanS".to_owned()));
        assert_eq!(names.len(), 36 + 1 + SCRIPTS.len());
    }

    #[test]
    fn sha256_matches_known_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn install_writes_models_skips_existing_and_records_a_manifest() {
        let root = std::env::temp_dir().join(format!("tidyshot-ocr-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // Pretend one model already exists at a plausible size.
        std::fs::write(root.join("eng.traineddata"), vec![7u8; 2048]).unwrap();
        let fetcher = FakeFetcher::default();
        let mut progress = Vec::new();
        let languages = install_models_at(&root, &fetcher, &mut |done, total| {
            progress.push((done, total))
        })
        .unwrap();
        assert_eq!(languages, 36);
        assert_eq!(progress.len(), model_names().len());
        assert_eq!(
            progress.last(),
            Some(&(model_names().len(), model_names().len()))
        );
        // The existing file was not fetched again.
        assert!(
            !fetcher
                .fetched()
                .iter()
                .any(|url| url.ends_with("/eng.traineddata"))
        );
        assert!(
            fetcher
                .fetched()
                .iter()
                .any(|url| url.ends_with("/spa.traineddata"))
        );
        let manifest = std::fs::read_to_string(root.join("manifest.json")).unwrap();
        let value = crate::json::parse(&manifest).unwrap();
        assert_eq!(
            value.get("version").and_then(crate::json::Value::as_str),
            Some(MODEL_VERSION)
        );
        assert_eq!(
            value.get("source").and_then(crate::json::Value::as_str),
            Some("tesseract-ocr/tessdata_fast")
        );
        assert_eq!(
            value
                .get("sha256")
                .and_then(|hashes| hashes.get("eng"))
                .and_then(crate::json::Value::as_str),
            Some(sha256_hex(&vec![7u8; 2048]).as_str())
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn install_rejects_a_too_small_download() {
        let root = std::env::temp_dir().join(format!("tidyshot-ocr-small-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let fetcher = FakeFetcher {
            body: vec![0u8; 10],
            ..FakeFetcher::default()
        };
        let error = install_models_at(&root, &fetcher, &mut |_, _| {}).unwrap_err();
        assert!(
            error.message().contains("Invalid language model"),
            "{error}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[derive(Default)]
    struct FakeFetcher {
        body: Vec<u8>,
        fetched: Mutex<Vec<String>>,
    }

    impl FakeFetcher {
        fn fetched(&self) -> Vec<String> {
            self.fetched.lock().unwrap().clone()
        }
    }

    impl ModelFetcher for FakeFetcher {
        fn fetch(&self, url: &str) -> Result<Vec<u8>, String> {
            self.fetched.lock().unwrap().push(url.to_owned());
            if self.body.is_empty() {
                Ok(vec![1u8; 2048])
            } else {
                Ok(self.body.clone())
            }
        }
    }
}
