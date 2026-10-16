//! File drag completion when Qt omits the Wayland drag action.
// Ported from omnishot/drag.py
//!
//! A Wayland drop can move keyboard focus, and Qt reports a cancelled drag
//! even when the receiving application accepted the file. The `drag-status`
//! preload helper brackets every drag so a drop is definitive. When the
//! helper is absent the Qt result is the only signal, exactly as upstream.

/// A drag bracket reported by the preload helper.
#[derive(Debug, Default)]
pub struct DragToken {
    generation: Option<libc::c_ulong>,
}

impl DragToken {
    /// Begin a drag, arming the helper and returning its generation token.
    #[must_use]
    pub fn begin() -> Self {
        match begin_hook() {
            Some(begin) => Self {
                generation: Some(unsafe { begin() }),
            },
            None => Self { generation: None },
        }
    }

    /// End the drag; true when the helper saw a completed drop.
    ///
    /// A missing helper leaves the token unarmed, so the caller falls back to
    /// Qt's own action.
    #[must_use]
    pub fn finish(self) -> bool {
        match (self.generation, end_hook()) {
            (Some(generation), Some(end)) => unsafe { end(generation) != 0 },
            _ => false,
        }
    }
}

/// Whether `result` counts as an accepted drag.
///
/// A definitive drop from the helper wins; otherwise Qt's action is accepted
/// when it is not `IgnoreAction`.
#[must_use]
pub fn drag_accepted(completion: bool, qt_ignored: bool) -> bool {
    completion || !qt_ignored
}

type BeginHook = unsafe extern "C" fn() -> libc::c_ulong;
type EndHook = unsafe extern "C" fn(libc::c_ulong) -> libc::c_int;

/// Resolve the helper symbols once, like upstream's import-time ctypes cache.
static BEGIN_HOOK: std::sync::OnceLock<Option<BeginHook>> = std::sync::OnceLock::new();
static END_HOOK: std::sync::OnceLock<Option<EndHook>> = std::sync::OnceLock::new();

fn begin_hook() -> Option<BeginHook> {
    *BEGIN_HOOK.get_or_init(|| {
        symbol("omnishot_drag_begin").map(|pointer| unsafe { std::mem::transmute(pointer) })
    })
}

fn end_hook() -> Option<EndHook> {
    *END_HOOK.get_or_init(|| {
        symbol("omnishot_drag_end").map(|pointer| unsafe { std::mem::transmute(pointer) })
    })
}

/// Look up a preloaded symbol in the global scope.
#[cfg(unix)]
fn symbol(name: &str) -> Option<*mut libc::c_void> {
    let Ok(name) = std::ffi::CString::new(name) else {
        return None;
    };
    // `RTLD_DEFAULT` is the null handle on glibc: search the global scope, so
    // the LD_PRELOAD helper's symbols are found but nothing is loaded.
    let pointer = unsafe { libc::dlsym(std::ptr::null_mut(), name.as_ptr()) };
    if pointer.is_null() {
        None
    } else {
        Some(pointer)
    }
}

#[cfg(not(unix))]
fn symbol(_name: &str) -> Option<*mut libc::c_void> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // Ported from omnishot/drag.py (execute)
    #[test]
    fn a_definitive_drop_wins_over_a_qt_ignore() {
        assert!(drag_accepted(true, true));
        assert!(drag_accepted(true, false));
    }

    #[test]
    fn without_the_helper_the_qt_action_decides() {
        assert!(drag_accepted(false, false));
        assert!(!drag_accepted(false, true));
    }

    #[test]
    fn an_unloaded_helper_leaves_the_token_unarmed() {
        // The test binary does not preload drag-status, so both lookups miss
        // and the token reports no completion.
        let token = DragToken::begin();
        assert!(token.generation.is_none());
        assert!(!token.finish());
    }
}
