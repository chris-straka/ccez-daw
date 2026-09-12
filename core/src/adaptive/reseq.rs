//! Horizontal resequencing: transition rules, beat/bar-aware switching,
//! stingers (GA-1, horizontal agent).
//!
//! A "cue section" in v1 is a named game state rendered through the cue's
//! layers: the `explore` section *is* `layers_for_state("explore")` at the
//! cue tempo. Resequencing is therefore the act of moving from one
//! state-section to another under a `TransitionRule`. This module answers two
//! questions for the engine:
//!
//! 1. `resolve_transition` — *what kind* of switch does this state change
//!    need, and *at which beat* does the new section take over?
//! 2. `Resequencer` — *has the switch fired yet?* Holds an armed deferred
//!    switch (`BarWait`, `Stinger`) across transport callbacks until its
//!    beat arrives; immediate switches (`Cut`, `Fade`) apply on request.
//!
//! The engine keeps owning the transport clock and the actual gains; this
//! module only computes beats and gains so it stays unit-testable without
//! audio hardware.

use crate::game_audio::{AdaptiveCue, TransitionKind};

/// Metre used when the engine has no time-signature track: 4/4.
pub const DEFAULT_BEATS_PER_BAR: f64 = 4.0;

/// Small epsilon for beat comparisons (float bar-line hits).
const BEAT_EPS: f64 = 1e-9;

/// What the engine should do for one state change.
///
/// Beats are absolute transport beats at the cue tempo.
#[derive(Debug, Clone, PartialEq)]
pub enum SwitchPlan {
    /// Cut now: swap to the target state's layers immediately.
    Immediate { to_state: String },
    /// Crossfade now over `fade_beats`; per-sample gains come from
    /// [`fade_gains`].
    Crossfade { to_state: String, fade_beats: f64 },
    /// Hold the old layers until `switch_beat` (a bar line), then cut.
    AtBar { to_state: String, switch_beat: f64 },
    /// Play `stinger_id` once now, then cut on its downbeat at
    /// `switch_beat` (`position_beats + stinger_beats`).
    StingerThenCut {
        to_state: String,
        stinger_id: String,
        switch_beat: f64,
    },
}

impl SwitchPlan {
    /// State that will be audible once this plan completes.
    pub fn to_state(&self) -> &str {
        match self {
            SwitchPlan::Immediate { to_state }
            | SwitchPlan::Crossfade { to_state, .. }
            | SwitchPlan::AtBar { to_state, .. }
            | SwitchPlan::StingerThenCut { to_state, .. } => to_state,
        }
    }

    /// Beat at which the new section takes over. Immediate/crossfade
    /// plans take over at the request beat (returned as `None` here —
    /// the engine already knows "now"); deferred plans return their
    /// armed beat.
    pub fn switch_beat(&self) -> Option<f64> {
        match self {
            SwitchPlan::Immediate { .. } | SwitchPlan::Crossfade { .. } => None,
            SwitchPlan::AtBar { switch_beat, .. }
            | SwitchPlan::StingerThenCut { switch_beat, .. } => Some(*switch_beat),
        }
    }

    /// True once a deferred plan's beat has arrived.
    pub fn is_due(&self, position_beats: f64) -> bool {
        match self.switch_beat() {
            None => true,
            Some(beat) => position_beats + BEAT_EPS >= beat,
        }
    }
}

/// Next bar line strictly after `position_beats`, in absolute transport
/// beats. A position sitting exactly on a bar line returns that same beat
/// (zero wait — the downbeat is now).
///
/// Non-positive `beats_per_bar` is a caller bug; it degrades to "now"
/// instead of dividing by zero.
pub fn next_bar_line(position_beats: f64, beats_per_bar: f64) -> f64 {
    if beats_per_bar <= 0.0 {
        return position_beats.max(0.0);
    }
    let pos = position_beats.max(0.0);
    let bar_index = (pos / beats_per_bar).floor();
    let line = bar_index * beats_per_bar;
    if pos - line < BEAT_EPS {
        line
    } else {
        (bar_index + 1.0) * beats_per_bar
    }
}

/// Linear crossfade gains `(out_gain, in_gain)` for a fade `elapsed_beats`
/// into a `fade_beats`-long ramp. Clamped: before the ramp the old section
/// is full, after it the new section is full.
pub fn fade_gains(elapsed_beats: f64, fade_beats: f64) -> (f64, f64) {
    if fade_beats <= 0.0 {
        return (0.0, 1.0);
    }
    let t = (elapsed_beats / fade_beats).clamp(0.0, 1.0);
    (1.0 - t, t)
}

/// Resolve *what kind* of switch a state change needs and *at which beat*
/// the new section takes over.
///
/// - Rule lookup is an exact `(from_state, to_state)` match via
///   [`AdaptiveCue::transition_for`]; **no rule = `Cut`** (sparse graph,
///   per contract).
/// - `Cut` (and a `Fade` with non-positive `fade_beats`) switch now.
/// - `BarWait` arms the next bar line after `position_beats`.
/// - `Stinger` arms `position_beats + stinger_beats`: the one-shot plays
///   now, the cut lands on its downbeat. `stinger_beats` is the stinger
///   length in beats at the cue tempo (known to the engine which owns the
///   stinger clip); it must be positive, else the cut is immediate.
/// - Negative positions clamp to `0.0`.
pub fn resolve_transition(
    cue: &AdaptiveCue,
    from_state: &str,
    to_state: &str,
    position_beats: f64,
    beats_per_bar: f64,
    stinger_beats: f64,
) -> SwitchPlan {
    let pos = position_beats.max(0.0);
    let to = to_state.to_string();
    match cue.transition_for(from_state, to_state) {
        None => SwitchPlan::Immediate { to_state: to },
        Some(rule) => match rule.kind {
            TransitionKind::Cut => SwitchPlan::Immediate { to_state: to },
            TransitionKind::Fade if rule.fade_beats <= 0.0 => {
                SwitchPlan::Immediate { to_state: to }
            }
            TransitionKind::Fade => SwitchPlan::Crossfade {
                to_state: to,
                fade_beats: rule.fade_beats,
            },
            TransitionKind::BarWait => SwitchPlan::AtBar {
                to_state: to,
                switch_beat: next_bar_line(pos, beats_per_bar),
            },
            TransitionKind::Stinger if stinger_beats <= 0.0 => {
                SwitchPlan::Immediate { to_state: to }
            }
            TransitionKind::Stinger => SwitchPlan::StingerThenCut {
                to_state: to,
                stinger_id: rule.stinger_cue_id.clone(),
                switch_beat: pos + stinger_beats,
            },
        },
    }
}

/// Armed deferred switch held across transport callbacks.
#[derive(Debug, Clone, PartialEq)]
struct PendingSwitch {
    to_state: String,
    switch_beat: f64,
}

/// Beat/bar-aware resequencing state machine.
///
/// The engine calls [`Resequencer::request`] from its snapshot handler and
/// [`Resequencer::poll`] from its per-buffer transport callback. Immediate
/// switches apply inside `request`; deferred ones (`BarWait`, `Stinger`)
/// arm a pending switch that `poll` fires when the transport beat arrives.
/// A new request always supersedes an armed-but-unfired switch (the player
/// changed their mind mid-bar — the latest game state wins).
#[derive(Debug, Clone)]
pub struct Resequencer {
    current_state: String,
    pending: Option<PendingSwitch>,
}

impl Resequencer {
    /// Start in the cue's `default_state` (what sounds before the first
    /// posted snapshot, per contract).
    pub fn new(cue: &AdaptiveCue) -> Self {
        Self {
            current_state: cue.default_state.clone(),
            pending: None,
        }
    }

    /// Currently audible section's state (target of the last fired switch).
    pub fn current_state(&self) -> &str {
        &self.current_state
    }

    /// Beat of the armed deferred switch, if one is waiting.
    pub fn pending_switch_beat(&self) -> Option<f64> {
        self.pending.as_ref().map(|p| p.switch_beat)
    }

    /// Handle a game-state change at `position_beats`. Returns the plan the
    /// engine must execute (start crossfade / hold-until-bar / fire stinger
    /// one-shot). Same-state requests and requests for the already-armed
    /// target are no-ops returning `None`.
    pub fn request(
        &mut self,
        cue: &AdaptiveCue,
        to_state: &str,
        position_beats: f64,
        beats_per_bar: f64,
        stinger_beats: f64,
    ) -> Option<SwitchPlan> {
        // Already there (or already heading there): keep the armed beat,
        // don't re-fire stingers. Requesting the *current* state while a
        // switch away is armed falls through: it cancels the pending
        // switch (latest game state wins) and resolves to an Immediate
        // hold-the-current-section plan.
        if let Some(pending) = &self.pending {
            if pending.to_state == to_state {
                return None;
            }
        } else if to_state == self.current_state {
            return None;
        }
        let plan = resolve_transition(
            cue,
            &self.current_state,
            to_state,
            position_beats,
            beats_per_bar,
            stinger_beats,
        );
        match &plan {
            SwitchPlan::Immediate { to_state } | SwitchPlan::Crossfade { to_state, .. } => {
                self.current_state = to_state.clone();
                self.pending = None;
            }
            SwitchPlan::AtBar { to_state, switch_beat }
            | SwitchPlan::StingerThenCut {
                to_state, switch_beat, ..
            } => {
                // Zero-wait deferred plan (request landed on the bar line,
                // or zero-length stinger resolved above): fire at once.
                if *switch_beat <= position_beats.max(0.0) + BEAT_EPS {
                    self.current_state = to_state.clone();
                    self.pending = None;
                } else {
                    self.pending = Some(PendingSwitch {
                        to_state: to_state.clone(),
                        switch_beat: *switch_beat,
                    });
                }
            }
        }
        Some(plan)
    }

    /// Transport callback: fire the armed switch once its beat arrives.
    /// Returns the newly audible state, if the section changed.
    pub fn poll(&mut self, position_beats: f64) -> Option<String> {
        let due = match &self.pending {
            Some(p) if position_beats + BEAT_EPS >= p.switch_beat => true,
            _ => false,
        };
        if due {
            let pending = self.pending.take().expect("checked");
            self.current_state = pending.to_state.clone();
            Some(pending.to_state)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_audio::{CueLayer, TransitionRule};

    fn demo_cue() -> AdaptiveCue {
        let mut cue = AdaptiveCue::new("cue_fight", "Fight");
        cue.default_state = "explore".to_string();
        cue.layers.push(CueLayer {
            id: "bed".to_string(),
            name: "Bed".to_string(),
            clip_ids: vec!["clip_a".to_string()],
            states: vec![],
            volume: 0.8,
        });
        cue.layers.push(CueLayer {
            id: "drums".to_string(),
            name: "Drums".to_string(),
            clip_ids: vec!["clip_b".to_string()],
            states: vec!["combat".to_string()],
            volume: 0.9,
        });
        cue.transitions.push(TransitionRule {
            id: "t_fade".to_string(),
            from_state: "explore".to_string(),
            to_state: "combat".to_string(),
            kind: TransitionKind::Fade,
            fade_beats: 4.0,
            stinger_cue_id: String::new(),
        });
        cue.transitions.push(TransitionRule {
            id: "t_bar".to_string(),
            from_state: "combat".to_string(),
            to_state: "explore".to_string(),
            kind: TransitionKind::BarWait,
            fade_beats: 0.0,
            stinger_cue_id: String::new(),
        });
        cue.transitions.push(TransitionRule {
            id: "t_sting".to_string(),
            from_state: "explore".to_string(),
            to_state: "boss".to_string(),
            kind: TransitionKind::Stinger,
            fade_beats: 0.0,
            stinger_cue_id: "sting_boss".to_string(),
        });
        cue
    }

    #[test]
    fn no_rule_means_immediate_cut() {
        let cue = demo_cue();
        // combat -> boss has no rule: sparse graph falls back to Cut.
        let plan = resolve_transition(&cue, "combat", "boss", 5.0, 4.0, 2.0);
        assert_eq!(
            plan,
            SwitchPlan::Immediate {
                to_state: "boss".to_string()
            }
        );
        assert!(plan.is_due(5.0));
    }

    #[test]
    fn transition_matches_exact_pair_only() {
        let cue = demo_cue();
        // Rule is explore->combat; same states reversed hit the BarWait rule,
        // an unrelated pair cuts.
        assert!(matches!(
            resolve_transition(&cue, "explore", "combat", 0.0, 4.0, 2.0),
            SwitchPlan::Crossfade { fade_beats, .. } if fade_beats == 4.0
        ));
        assert!(matches!(
            resolve_transition(&cue, "combat", "explore", 5.0, 4.0, 2.0),
            SwitchPlan::AtBar { .. }
        ));
        assert!(matches!(
            resolve_transition(&cue, "combat", "menu", 5.0, 4.0, 2.0),
            SwitchPlan::Immediate { .. }
        ));
    }

    #[test]
    fn transition_on_bar_waits_for_next_bar_line() {
        let cue = demo_cue();
        let mut seq = Resequencer::new(&cue);
        // Move to combat first via the Fade rule (immediate take-over).
        let plan = seq
            .request(&cue, "combat", 0.0, DEFAULT_BEATS_PER_BAR, 2.0)
            .expect("state change");
        assert!(matches!(plan, SwitchPlan::Crossfade { .. }));
        assert_eq!(seq.current_state(), "combat");

        // combat -> explore is BarWait. Mid-bar (beat 5 of 4/4) arms beat 8.
        let plan = seq
            .request(&cue, "explore", 5.0, DEFAULT_BEATS_PER_BAR, 2.0)
            .expect("state change");
        assert_eq!(
            plan,
            SwitchPlan::AtBar {
                to_state: "explore".to_string(),
                switch_beat: 8.0,
            }
        );
        // Old section still audible before the bar line...
        assert_eq!(seq.poll(7.9), None);
        assert_eq!(seq.current_state(), "combat");
        // ...new section takes over exactly on the downbeat.
        assert_eq!(seq.poll(8.0), Some("explore".to_string()));
        assert_eq!(seq.current_state(), "explore");
        assert_eq!(seq.pending_switch_beat(), None);
    }

    #[test]
    fn bar_wait_on_the_downbeat_fires_at_once() {
        let cue = demo_cue();
        let mut seq = Resequencer::new(&cue);
        seq.request(&cue, "combat", 0.0, 4.0, 2.0);
        // Request lands exactly on bar line 8: zero wait.
        let plan = seq.request(&cue, "explore", 8.0, 4.0, 2.0).expect("change");
        assert_eq!(
            plan,
            SwitchPlan::AtBar {
                to_state: "explore".to_string(),
                switch_beat: 8.0,
            }
        );
        assert_eq!(seq.current_state(), "explore");
        assert_eq!(seq.pending_switch_beat(), None);
    }

    #[test]
    fn next_bar_line_math() {
        assert_eq!(next_bar_line(5.0, 4.0), 8.0);
        assert_eq!(next_bar_line(8.0, 4.0), 8.0);
        assert_eq!(next_bar_line(0.0, 4.0), 0.0);
        assert_eq!(next_bar_line(2.5, 3.0), 3.0);
        assert_eq!(next_bar_line(-1.0, 4.0), 0.0);
        // Degenerate metre degrades to "now", never NaN.
        assert_eq!(next_bar_line(5.0, 0.0), 5.0);
    }

    #[test]
    fn fade_gains_ramp_linearly_and_clamp() {
        assert_eq!(fade_gains(0.0, 4.0), (1.0, 0.0));
        assert_eq!(fade_gains(2.0, 4.0), (0.5, 0.5));
        assert_eq!(fade_gains(4.0, 4.0), (0.0, 1.0));
        assert_eq!(fade_gains(9.0, 4.0), (0.0, 1.0));
        assert_eq!(fade_gains(-1.0, 4.0), (1.0, 0.0));
        assert_eq!(fade_gains(0.0, 0.0), (0.0, 1.0));
    }

    #[test]
    fn stinger_defers_cut_until_its_downbeat() {
        let cue = demo_cue();
        let mut seq = Resequencer::new(&cue);
        // 2-beat stinger fired at beat 1: cut lands on beat 3.
        let plan = seq.request(&cue, "boss", 1.0, 4.0, 2.0).expect("change");
        assert_eq!(
            plan,
            SwitchPlan::StingerThenCut {
                to_state: "boss".to_string(),
                stinger_id: "sting_boss".to_string(),
                switch_beat: 3.0,
            }
        );
        assert_eq!(seq.current_state(), "explore");
        assert_eq!(seq.poll(2.9), None);
        assert_eq!(seq.poll(3.0), Some("boss".to_string()));
    }

    #[test]
    fn new_request_supersedes_armed_bar_wait() {
        let cue = demo_cue();
        let mut seq = Resequencer::new(&cue);
        seq.request(&cue, "combat", 0.0, 4.0, 2.0);
        seq.request(&cue, "explore", 5.0, 4.0, 2.0)
            .expect("bar wait armed");
        assert_eq!(seq.pending_switch_beat(), Some(8.0));
        // Player re-engages before the bar line: requesting the still-
        // current state cancels the armed switch away (hold combat).
        let plan = seq.request(&cue, "combat", 6.0, 4.0, 2.0).expect("change");
        assert_eq!(
            plan,
            SwitchPlan::Immediate {
                to_state: "combat".to_string()
            }
        );
        assert_eq!(seq.current_state(), "combat");
        assert_eq!(seq.pending_switch_beat(), None);
        // The stale bar-line firing must not resurrect explore.
        assert_eq!(seq.poll(8.0), None);
        assert_eq!(seq.current_state(), "combat");
    }

    #[test]
    fn same_state_request_is_a_no_op() {
        let cue = demo_cue();
        let mut seq = Resequencer::new(&cue);
        assert_eq!(seq.request(&cue, "explore", 0.0, 4.0, 2.0), None);
    }
}
