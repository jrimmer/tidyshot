//! Place the Quick Access preview with a Hyprland window rule.
//!
//! Wayland ignores an application-set toplevel position, and this compositor
//! rejects `movewindowpixel`, so the preview computes its corner and pins it
//! through a dynamic window rule before the window maps. The rule is named
//! after its matcher (`title:^(TidyShot Preview)`) so `scripts/uninstall_user.py`
//! can remove the same rule.

use std::time::Duration;

use crate::commands::{CommandError, CommandRunner, CommandSpec};

/// The matcher that identifies the Quick Access preview window.
///
/// The title is `TidyShot Preview — <file>`, so a prefix match covers every
/// capture and recording.
pub const PREVIEW_MATCHER: &str = "title:^(TidyShot Preview)";

/// The rule syntax the running compositor accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleSyntax {
    /// Hyprland 0.56's `windowrule` keyword.
    WindowRule,
    /// The older `windowrulev2` spelling, kept as a fallback.
    WindowRuleV2,
}

impl RuleSyntax {
    /// The `hyprctl keyword` name for this syntax.
    #[must_use]
    pub const fn keyword(self) -> &'static str {
        match self {
            Self::WindowRule => "windowrule",
            Self::WindowRuleV2 => "windowrulev2",
        }
    }

    const fn all() -> [Self; 2] {
        [Self::WindowRule, Self::WindowRuleV2]
    }
}

/// The dynamic rule that pins the preview at `x`, `y` in logical pixels.
#[must_use]
pub fn placement_rule(x: f64, y: f64) -> String {
    format!(
        "move {} {},{}",
        x.round() as i64,
        y.round() as i64,
        PREVIEW_MATCHER
    )
}

/// The rule that removes every dynamic rule matching the preview.
#[must_use]
pub fn removal_rule() -> String {
    format!("unset,{PREVIEW_MATCHER}")
}

fn keyword_spec(keyword: &str, rule: &str) -> CommandSpec {
    CommandSpec::new("hyprctl")
        .args(["keyword", keyword, rule])
        .timeout(Duration::from_secs(5))
}

/// Pin the preview rule for one open, replacing any rule from a previous one.
///
/// The previous rule is removed first so a changed corner, margin or focused
/// monitor is honoured instead of leaving two moves to fight. A removal the
/// compositor does not understand is not fatal: the new rule still lands last
/// and wins for the next map.
pub fn apply<R: CommandRunner>(runner: &R, x: f64, y: f64) -> Result<RuleSyntax, CommandError> {
    let removal = removal_rule();
    let mut last_error = None;
    for syntax in RuleSyntax::all() {
        match runner.run(&keyword_spec(syntax.keyword(), &removal)) {
            Ok(_) => break,
            Err(error) => last_error = Some(error),
        }
    }
    let rule = placement_rule(x, y);
    for syntax in RuleSyntax::all() {
        match runner.run(&keyword_spec(syntax.keyword(), &rule)) {
            Ok(_) => return Ok(syntax),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.expect("every syntax was attempted"))
}

/// Remove the dynamic preview rule.
///
/// Uninstall calls this through the same matcher, so a rule left by a running
/// preview never survives the removal of its application.
pub fn remove<R: CommandRunner>(runner: &R) -> Result<RuleSyntax, CommandError> {
    let removal = removal_rule();
    let mut last_error = None;
    for syntax in RuleSyntax::all() {
        match runner.run(&keyword_spec(syntax.keyword(), &removal)) {
            Ok(_) => return Ok(syntax),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.expect("every syntax was attempted"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{CommandOutput, CommandSpec};
    use std::sync::Mutex;

    fn output() -> CommandOutput {
        CommandOutput {
            status: 0,
            stdout: Vec::new(),
            stderr: Vec::new(),
        }
    }

    /// A runner that records specs and rejects chosen keyword syntaxes.
    struct Fake {
        seen: Mutex<Vec<CommandSpec>>,
        rejects: Mutex<Vec<&'static str>>,
    }

    impl Fake {
        fn rejecting(keywords: &[&'static str]) -> Self {
            Self {
                seen: Mutex::new(Vec::new()),
                rejects: Mutex::new(keywords.to_vec()),
            }
        }
    }

    impl Default for Fake {
        fn default() -> Self {
            Self {
                seen: Mutex::new(Vec::new()),
                rejects: Mutex::new(Vec::new()),
            }
        }
    }

    impl CommandRunner for Fake {
        fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, CommandError> {
            self.seen.lock().unwrap().push(spec.clone());
            if self
                .rejects
                .lock()
                .unwrap()
                .contains(&spec.args[1].as_str())
            {
                return Err(CommandError::Failure {
                    program: spec.program.clone(),
                    output: CommandOutput {
                        status: 1,
                        stdout: Vec::new(),
                        stderr: b"unknown keyword".to_vec(),
                    },
                });
            }
            Ok(output())
        }
    }

    fn rules(runner: &Fake) -> Vec<(String, String)> {
        runner
            .seen
            .lock()
            .unwrap()
            .iter()
            .map(|spec| (spec.args[1].clone(), spec.args[2].clone()))
            .collect()
    }

    #[test]
    fn the_placement_rule_carries_the_computed_corner() {
        assert_eq!(
            placement_rule(1547.0, 762.0),
            "move 1547 762,title:^(TidyShot Preview)"
        );
        // A fractional scale anchor is rounded to the logical pixel the
        // compositor moves to.
        assert_eq!(
            placement_rule(960.4, 540.6),
            "move 960 541,title:^(TidyShot Preview)"
        );
        assert_eq!(removal_rule(), "unset,title:^(TidyShot Preview)");
    }

    #[test]
    fn the_rule_matches_every_corner_and_margin_shape() {
        for (x, y) in [(18.0, 18.0), (1547.0, 762.0), (-120.0, 40.0)] {
            let rule = placement_rule(x, y);
            assert!(rule.starts_with("move "), "{rule}");
            assert!(rule.ends_with(",title:^(TidyShot Preview)"), "{rule}");
        }
    }

    #[test]
    fn applying_replaces_the_previous_rule_before_pinning_the_new_one() {
        let runner = Fake::default();
        assert_eq!(
            apply(&runner, 1547.0, 762.0).unwrap(),
            RuleSyntax::WindowRule
        );
        assert_eq!(
            rules(&runner),
            [
                (
                    "windowrule".to_owned(),
                    "unset,title:^(TidyShot Preview)".to_owned()
                ),
                (
                    "windowrule".to_owned(),
                    "move 1547 762,title:^(TidyShot Preview)".to_owned()
                ),
            ]
        );
        let seen = runner.seen.lock().unwrap();
        assert_eq!(seen[0].program, "hyprctl");
        assert_eq!(seen[0].args[0], "keyword");
    }

    #[test]
    fn an_older_compositor_falls_back_to_windowrulev2() {
        let runner = Fake::rejecting(&["windowrule"]);
        assert_eq!(
            apply(&runner, 100.0, 200.0).unwrap(),
            RuleSyntax::WindowRuleV2
        );
        let applied = rules(&runner);
        assert_eq!(
            applied.last(),
            Some(&(
                "windowrulev2".to_owned(),
                "move 100 200,title:^(TidyShot Preview)".to_owned()
            ))
        );
    }

    #[test]
    fn removal_tries_both_spellings_and_reports_the_last_failure() {
        let runner = Fake::rejecting(&["windowrule"]);
        assert_eq!(remove(&runner).unwrap(), RuleSyntax::WindowRuleV2);
        let failing = Fake::rejecting(&["windowrule", "windowrulev2"]);
        let error = remove(&failing).unwrap_err();
        assert_eq!(error.to_string(), "unknown keyword");
    }
}
