//! Test-only hold points for blocking-pool work.
//!
//! The responsiveness regression tests need to prove a notification handler's
//! synchronous work runs off the connection's serve future. Requests can pin
//! that with a pure ordering check (see `assert_stays_responsive` in the test
//! harness), but a notification has no response to order against — earlier
//! versions fell back to wall-clock budgets, which flaked on loaded CI
//! runners. A gate makes the check deterministic instead: the test arms a
//! named section, the handler's blocking closure parks in `pass` until
//! released, and the test asserts the server still answers a probe while the
//! work is provably in flight. On a correct build that holds under any
//! scheduling; on a regressed (inline) build the serve future itself parks,
//! no later message is processed, and the probe times out — a hard failure,
//! never a flake.
//!
//! Production builds compile the whole mechanism out: without the
//! `test-hooks` feature (enabled only for test targets, via the self
//! dev-dependency in Cargo.toml) `DebugGate` is a zero-sized no-op and the
//! hold/release custom methods are never registered, so a real client cannot
//! arm anything.

/// Gate section around `did_open`'s parse-diagnostics closure.
pub const GATE_DID_OPEN_PARSE: &str = "didOpen.parse";
/// Gate section around `did_save`'s diagnostics-recompute closure.
pub const GATE_DID_SAVE_DIAGNOSTICS: &str = "didSave.diagnostics";
/// Gate section around `did_change_watched_files`' batched parse closure.
pub const GATE_DID_CHANGE_WATCHED_FILES: &str = "didChangeWatchedFiles.parse";
/// Gate section around `will_rename_files`' `use`-edit batch closure.
pub const GATE_WILL_RENAME_FILES: &str = "willRenameFiles.useEdits";
/// Gate section around `will_delete_files`' `use`-edit batch closure.
pub const GATE_WILL_DELETE_FILES: &str = "willDeleteFiles.useEdits";
/// Gate section around `prepare_call_hierarchy`'s indexed-lookup closure.
pub const GATE_PREPARE_CALL_HIERARCHY: &str = "prepareCallHierarchy.lookup";
/// Gate section around `selection_range`'s AST-walk closure.
pub const GATE_SELECTION_RANGE: &str = "selectionRange.walk";
/// Gate section around `goto_implementation`'s method-decl cursor check.
pub const GATE_GOTO_IMPLEMENTATION: &str = "gotoImplementation.methodDeclCheck";
/// Gate section around `rename`'s property-decl cursor check + variable rename.
pub const GATE_RENAME_VARIABLE: &str = "rename.variable";
/// Gate section around `completion_resolve`'s all-indexes signature/doc lookup.
pub const GATE_COMPLETION_RESOLVE: &str = "completionResolve.lookup";
/// Gate section around `inlay_hint_resolve`'s all-indexes doc lookup.
pub const GATE_INLAY_HINT_RESOLVE: &str = "inlayHintResolve.lookup";

#[cfg(not(feature = "test-hooks"))]
#[derive(Default)]
pub struct DebugGate;

#[cfg(not(feature = "test-hooks"))]
impl DebugGate {
    #[inline(always)]
    pub fn pass(&self, _section: &str) {}

    #[inline(always)]
    pub fn held_section(&self) -> Option<String> {
        None
    }
}

#[cfg(feature = "test-hooks")]
pub use real::DebugGate;

#[cfg(feature = "test-hooks")]
mod real {
    use std::sync::{Condvar, Mutex};

    #[derive(Default)]
    struct GateState {
        /// Section name the next matching `pass` call should hold at.
        armed: Option<String>,
        /// Section currently parked at the gate (surfaced via `debugStats`).
        held: Option<String>,
        released: bool,
    }

    #[derive(Default)]
    pub struct DebugGate {
        state: Mutex<GateState>,
        cv: Condvar,
    }

    impl DebugGate {
        /// Arm the gate: the next `pass(section)` parks until [`release`](Self::release).
        /// One passage per arm; re-arming replaces any previous armed section.
        pub fn arm(&self, section: &str) {
            let mut s = self.state.lock().unwrap();
            s.armed = Some(section.to_owned());
            s.released = false;
        }

        pub fn release(&self) {
            let mut s = self.state.lock().unwrap();
            s.armed = None;
            s.released = true;
            self.cv.notify_all();
        }

        /// Section currently parked at the gate, if any.
        pub fn held_section(&self) -> Option<String> {
            self.state.lock().unwrap().held.clone()
        }

        /// Hold point. No-op unless the gate is armed for exactly `section`;
        /// when it is, claims the arm (so concurrent passages can't stack) and
        /// parks until released. The park is capped at 60s: a test that fails
        /// without releasing must not leave this task parked forever — tokio's
        /// runtime shutdown joins in-flight blocking tasks, so an uncapped park
        /// would turn that test's failure report into an eternal hang.
        pub fn pass(&self, section: &str) {
            let s = self.state.lock().unwrap();
            if s.armed.as_deref() != Some(section) {
                return;
            }
            let mut s = s;
            s.armed = None;
            s.held = Some(section.to_owned());
            let (mut s, _timed_out) = self
                .cv
                .wait_timeout_while(s, std::time::Duration::from_secs(60), |s| !s.released)
                .unwrap();
            s.held = None;
            s.released = false;
        }
    }
}

#[cfg(all(test, feature = "test-hooks"))]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn unarmed_pass_is_a_no_op() {
        let gate = DebugGate::default();
        gate.pass("anything");
        assert_eq!(gate.held_section(), None);
    }

    #[test]
    fn armed_pass_parks_until_release() {
        let gate = Arc::new(DebugGate::default());
        gate.arm("sec");
        let g = Arc::clone(&gate);
        let t = std::thread::spawn(move || g.pass("sec"));
        while gate.held_section().as_deref() != Some("sec") {
            std::thread::yield_now();
        }
        assert!(!t.is_finished());
        gate.release();
        t.join().unwrap();
        assert_eq!(gate.held_section(), None);
    }

    #[test]
    fn arm_holds_only_the_named_section_and_only_once() {
        let gate = Arc::new(DebugGate::default());
        gate.arm("sec");
        gate.pass("other"); // wrong section: must not park
        let g = Arc::clone(&gate);
        let t = std::thread::spawn(move || g.pass("sec"));
        while gate.held_section().is_none() {
            std::thread::yield_now();
        }
        gate.release();
        t.join().unwrap();
        gate.pass("sec"); // arm consumed: must not park again
    }
}
