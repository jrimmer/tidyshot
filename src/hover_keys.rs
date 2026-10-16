//! Hover shortcuts for an overlay, without stealing focus from the active app.
// Ported from omnishot/overlay_keys.py
//!
//! Hyprland keys are installed only while the pointer is over the preview and
//! removed again. The install is a lease in compositor state; the decision of
//! which keys are safe to install and whether a lost lease should be rearmed
//! is pure and is asserted here.

use std::collections::BTreeMap;

use crate::json::Value;

/// The hover actions, keyed by the Hyprland binding spelling.
pub const ACTIONS: &[(&str, &str)] = &[
    ("CTRL + C", "copy"),
    ("CTRL + S", "save"),
    ("CTRL + W", "close"),
    ("CTRL + E", "annotate"),
    ("CTRL + P", "pin"),
    ("SPACE", "preview"),
    ("ESCAPE", "close"),
];

/// The capture-scoped scrolling actions.
pub const SCROLL_ACTIONS: &[(&str, &str)] = &[("ESCAPE", "cancel"), ("RETURN", "accept")];

/// The bits of a preview that decide whether a lost hover lease is rearmed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OverlayState {
    pub visible: bool,
    pub hovered: bool,
    pub collapsed: bool,
    pub dragging: bool,
    pub copying: bool,
    pub large_open: bool,
}

/// Filter the hover actions to the keys no other binding already owns.
///
/// Hyprland 0.56's `handle.remove()` removes every binding with the same
/// modifiers and key, including bindings in other submaps, so a key already
/// bound must never be installed over.
// Ported from omnishot/overlay_keys.py (available_actions)
#[must_use]
pub fn available_actions(bindings: &[Value]) -> BTreeMap<&'static str, &'static str> {
    available_from(bindings, ACTIONS)
}

/// The same filter for an arbitrary action table, so the rule is testable
/// without the fixed set.
#[must_use]
pub fn available_from(
    bindings: &[Value],
    actions: &'static [(&'static str, &'static str)],
) -> BTreeMap<&'static str, &'static str> {
    let mut occupied: Vec<(i64, String)> = Vec::new();
    for binding in bindings {
        let modmask = binding
            .get("modmask")
            .and_then(Value::as_f64)
            .unwrap_or(0.0) as i64;
        let key = binding
            .get("key")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_lowercase();
        occupied.push((modmask, key));
    }
    actions
        .iter()
        .filter(|(name, _)| {
            let key = name.rsplit(" + ").next().unwrap_or(name).to_lowercase();
            let modmask = if name.starts_with("CTRL + ") { 4 } else { 0 };
            !occupied
                .iter()
                .any(|(mask, occupied_key)| *mask == modmask && *occupied_key == key)
        })
        .copied()
        .collect()
}

/// Whether a hover lease should be rearmed after the compositor dropped it.
///
/// A preview that is hidden, not hovered, collapsed, mid-drag, mid-copy or
/// showing its large view never reinstalls the keys. A scrolling capture
/// rearms whenever any guide is still visible.
// Ported from omnishot/overlay_keys.py (OverlayKeys.should_rearm)
#[must_use]
pub fn should_rearm(state: OverlayState, require_hover: bool) -> bool {
    state.visible
        && (!require_hover || state.hovered)
        && !state.collapsed
        && !state.dragging
        && !state.copying
        && !state.large_open
}

/// The scrolling-capture rearm rule: any visible guide keeps the lease.
// Ported from omnishot/overlay_keys.py (ScrollKeys.should_rearm)
#[must_use]
pub fn scroll_should_rearm(closed: bool, guides_visible: bool) -> bool {
    !closed && guides_visible
}

/// What a poll of the compositor lease found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaseReply {
    /// The lease is gone.
    Inactive,
    /// Another window owns the lease now.
    Replaced,
    /// The lease is held by this overlay.
    Active,
}

impl LeaseReply {
    /// Parse the compositor's reply, defaulting unknown text to [`Active`].
    ///
    /// [`Active`]: LeaseReply::Active
    #[must_use]
    pub fn parse(reply: &str) -> Self {
        match reply.trim() {
            "inactive" => Self::Inactive,
            "replaced" => Self::Replaced,
            _ => Self::Active,
        }
    }

    /// The action a poll result implies: whether to rearm, stop, or do nothing.
    #[must_use]
    pub const fn decision(self) -> LeaseDecision {
        match self {
            Self::Inactive => LeaseDecision::MaybeRearm,
            Self::Replaced => LeaseDecision::Stop,
            Self::Active => LeaseDecision::Keep,
        }
    }
}

/// What to do after a lease poll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaseDecision {
    /// Reinstall only if the overlay is eligible.
    MaybeRearm,
    /// Another window owns the lease; stop without rearming.
    Stop,
    /// Keep the lease.
    Keep,
}

#[cfg(test)]
mod tests {
    use super::*;

    // Ported from omnishot/tests/test_overlay_keys.py
    // (test_hover_keys_preserve_user_bindings_in_other_submaps)
    #[test]
    fn hover_keys_preserve_user_bindings_in_other_submaps() {
        let bindings = crate::json::parse(
            r#"[
                {"modmask":4,"key":"c","submap":"editor","enabled":false},
                {"modmask":0,"key":"space","submap":""},
                {"modmask":64,"key":"S"}
            ]"#,
        )
        .unwrap();
        let actions = available_actions(bindings.as_array().unwrap());
        assert!(!actions.contains_key("CTRL + C"));
        assert!(!actions.contains_key("SPACE"));
        assert_eq!(actions.get("CTRL + S").copied(), Some("save"));
        assert_eq!(actions.get("CTRL + E").copied(), Some("annotate"));
        // The unrelated user binding is still untouched.
        let rows = bindings.as_array().unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].get("enabled").and_then(Value::as_bool), Some(false));
    }

    #[test]
    fn a_modifier_key_does_not_block_a_bare_key() {
        let bindings = crate::json::parse(r#"[{"modmask":64,"key":"s"}]"#).unwrap();
        let actions = available_actions(bindings.as_array().unwrap());
        assert!(actions.contains_key("CTRL + S"));
    }

    #[test]
    fn should_rearm_requires_visibility_and_hover() {
        let visible = OverlayState {
            visible: true,
            hovered: true,
            ..OverlayState::default()
        };
        assert!(should_rearm(visible, true));
        // Collapsed, dragging, copying or a large view suppresses rearm.
        for state in [
            OverlayState {
                collapsed: true,
                ..visible
            },
            OverlayState {
                dragging: true,
                ..visible
            },
            OverlayState {
                copying: true,
                ..visible
            },
            OverlayState {
                large_open: true,
                ..visible
            },
            OverlayState {
                visible: false,
                ..visible
            },
        ] {
            assert!(!should_rearm(state, true), "{state:?}");
        }
        // A non-hover lease (scrolling) does not require the pointer.
        let not_hovered = OverlayState {
            visible: true,
            hovered: false,
            ..OverlayState::default()
        };
        assert!(should_rearm(not_hovered, false));
        assert!(!should_rearm(not_hovered, true));
    }

    #[test]
    fn scrolling_rearms_while_a_guide_is_visible() {
        assert!(scroll_should_rearm(false, true));
        assert!(!scroll_should_rearm(true, true));
        assert!(!scroll_should_rearm(false, false));
    }

    #[test]
    fn poll_replies_map_to_decisions() {
        assert_eq!(LeaseReply::parse("inactive"), LeaseReply::Inactive);
        assert_eq!(LeaseReply::parse("replaced"), LeaseReply::Replaced);
        assert_eq!(LeaseReply::parse("copy\n"), LeaseReply::Active);
        assert_eq!(LeaseReply::Inactive.decision(), LeaseDecision::MaybeRearm);
        assert_eq!(LeaseReply::Replaced.decision(), LeaseDecision::Stop);
        assert_eq!(LeaseReply::Active.decision(), LeaseDecision::Keep);
    }

    #[test]
    fn scroll_actions_are_a_separate_lease() {
        assert_eq!(SCROLL_ACTIONS.len(), 2);
        let bindings = crate::json::parse(r#"[{"modmask":0,"key":"escape"}]"#).unwrap();
        let actions = available_from(bindings.as_array().unwrap(), SCROLL_ACTIONS);
        assert!(!actions.contains_key("ESCAPE"));
        assert!(actions.contains_key("RETURN"));
    }
}
