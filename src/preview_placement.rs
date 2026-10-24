//! Place the Quick Access preview with a Hyprland window rule.
//!
//! Wayland ignores an application-set toplevel position, and this compositor
//! runs the Lua configuration, where `hyprctl keyword windowrule` is rejected
//! ("can't work with non-legacy parsers"). The rule therefore goes through
//! `hyprctl eval`, which is the documented path for a Lua configuration:
//!
//!   o.window({ title = "..." }, { name = "tidyshot-preview", move = "X Y" })
//!
//! The rule is named so a later position replaces it and uninstall can disable
//! it by name.

use std::time::Duration;

use crate::commands::{CommandError, CommandRunner, CommandSpec};

/// The title matcher that identifies the Quick Access preview window.
///
/// QuickOverlay titles a still "Capture ready" and a recording
/// "Recording ready".
pub const PREVIEW_MATCHER: &str = "^(Capture|Recording) ready$";

/// The name the dynamic rule is registered under.
pub const RULE_NAME: &str = "tidyshot-preview";

/// The Lua call that pins the preview at `x`, `y` in logical pixels.
#[must_use]
pub fn placement_rule(x: f64, y: f64) -> String {
    format!(
        "o.window({{ title = \"{PREVIEW_MATCHER}\" }}, {{ name = \"{RULE_NAME}\", move = \"{} {}\" }})",
        x.round() as i64,
        y.round() as i64
    )
}

/// The Lua call that disables the dynamic rule.
#[must_use]
pub fn removal_rule() -> String {
    format!("hl.window_rule({{ name = \"{RULE_NAME}\", enabled = false }})")
}

fn eval_spec(expression: &str) -> CommandSpec {
    CommandSpec::new("hyprctl")
        .args(["eval", expression])
        .timeout(Duration::from_secs(5))
}

/// Pin the preview rule for one open, replacing any rule from a previous one.
///
/// A named rule replaces its earlier definition, so a changed corner, margin
/// or focused monitor is honoured instead of leaving two moves to fight.
pub fn apply<R: CommandRunner>(runner: &R, x: f64, y: f64) -> Result<(), CommandError> {
    runner.run(&eval_spec(&placement_rule(x, y)))?;
    Ok(())
}

/// Remove the dynamic preview rule.
///
/// Uninstall calls this so a rule left by a running preview never survives the
/// removal of its application.
pub fn remove<R: CommandRunner>(runner: &R) -> Result<(), CommandError> {
    runner.run(&eval_spec(&removal_rule()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::CommandOutput;
    use parking_lot::Mutex;

    fn output() -> CommandOutput {
        CommandOutput {
            status: 0,
            stdout: Vec::new(),
            stderr: Vec::new(),
        }
    }

    #[derive(Default)]
    struct Fake {
        seen: Mutex<Vec<CommandSpec>>,
    }

    impl CommandRunner for Fake {
        fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, CommandError> {
            self.seen.lock().push(spec.clone());
            Ok(output())
        }
    }

    fn expressions(runner: &Fake) -> Vec<String> {
        runner
            .seen
            .lock()
            .iter()
            .map(|spec| spec.args[1].clone())
            .collect()
    }

    #[test]
    fn the_placement_rule_carries_the_computed_corner() {
        assert_eq!(
            placement_rule(1547.0, 762.0),
            "o.window({ title = \"^(Capture|Recording) ready$\" }, \
             { name = \"tidyshot-preview\", move = \"1547 762\" })"
        );
        // A fractional anchor is rounded to the logical pixel the compositor
        // moves to.
        assert_eq!(
            placement_rule(960.4, 540.6),
            "o.window({ title = \"^(Capture|Recording) ready$\" }, \
             { name = \"tidyshot-preview\", move = \"960 541\" })"
        );
        assert_eq!(
            removal_rule(),
            "hl.window_rule({ name = \"tidyshot-preview\", enabled = false })"
        );
    }

    #[test]
    fn the_rule_matches_every_corner_and_margin_shape() {
        for (x, y) in [(18.0, 18.0), (1547.0, 762.0), (-120.0, 40.0)] {
            let rule = placement_rule(x, y);
            assert!(rule.starts_with("o.window({ title = "), "{rule}");
            assert!(rule.contains("name = \"tidyshot-preview\""), "{rule}");
            assert!(rule.ends_with("})"), "{rule}");
        }
    }

    #[test]
    fn applying_sets_the_named_rule_in_one_eval() {
        let runner = Fake::default();
        apply(&runner, 1547.0, 762.0).unwrap();
        assert_eq!(
            expressions(&runner),
            [placement_rule(1547.0, 762.0).to_owned()]
        );
        assert!(
            runner.seen.lock()[0]
                .args
                .first()
                .is_some_and(|arg| arg == "eval")
        );
    }

    #[test]
    fn removing_disables_the_named_rule() {
        let runner = Fake::default();
        remove(&runner).unwrap();
        assert_eq!(expressions(&runner), [removal_rule().to_owned()]);
    }
}
