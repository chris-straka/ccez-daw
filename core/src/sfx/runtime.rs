//! SFX trigger runtime: pool picks, humanization, throttles, RTPC mapping.
//!
//! All randomness flows through [`SeededRng`] (xorshift64*, seeded per
//! runtime), so two runtimes with the same seed trigger identical voices —
//! the reproducibility the export path depends on.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::game_audio::{GameStateParam, GameStateSnapshot, SfxBank};

/// Minimal deterministic RNG (xorshift64*). Kept local so the SFX runtime
/// adds no new crate dependencies; good enough for pool picks and
/// humanization spreads, not for cryptography.
#[derive(Debug, Clone)]
pub struct SeededRng(u64);

impl SeededRng {
    pub fn new(seed: u64) -> Self {
        // Zero is a degenerate xorshift state; mix in a nonzero constant.
        Self(if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        })
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform draw from `[0, 1)`.
    pub fn next_f64(&mut self) -> f64 {
        const DIV: f64 = (u64::MAX as f64) + 1.0;
        (self.next_u64() as f64) / DIV
    }

    /// Uniform draw from `[lo, hi]`.
    pub fn range_f64(&mut self, lo: f64, hi: f64) -> f64 {
        lo + self.next_f64() * (hi - lo)
    }

    /// Uniform index into a nonempty pool of length `len`.
    pub fn pick_index(&mut self, len: usize) -> usize {
        debug_assert!(len > 0);
        ((self.next_f64() * (len as f64)) as usize) % len
    }
}

/// One RTPC binding resolved for a single trigger: the game value was
/// clamped to the declared param range, normalized to 0–1, then mapped
/// linearly onto `[min, max]`. `min`/`max` are kept so audition rendering
/// can re-normalize filter-style targets without re-reading the bank.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedRtpc {
    pub target_node: String,
    pub target_param: String,
    pub value: f64,
    pub min: f64,
    pub max: f64,
}

impl ResolvedRtpc {
    /// The resolved value re-normalized to 0–1 within its own output range
    /// (degenerate range reads as 0).
    pub fn normalized(&self) -> f64 {
        let span = self.max - self.min;
        if span.abs() < f64::EPSILON {
            0.0
        } else {
            ((self.value - self.min) / span).clamp(0.0, 1.0)
        }
    }
}

/// One sounding voice: the full per-trigger decision, ready for audition
/// rendering or engine playback. `clip_id` binds the event to a v0 clip by
/// string id (no v0 types move); `delay_ms` is the timing-spread onset.
#[derive(Debug, Clone, PartialEq, Serialize,Deserialize)]
pub struct Voice {
    pub event_id: String,
    pub clip_id: String,
    pub gain: f64,
    pub pitch_semitones: f64,
    pub delay_ms: f64,
    pub started_ms: u64,
    pub rtpc: Vec<ResolvedRtpc>,
}

/// Per-trigger runtime parameters. These are *not* stored on the bank:
/// the frozen v1 `SfxEvent` has no timing field, so timing humanization
/// travels alongside the trigger instead of inside the schema.
#[derive(Debug, Clone, PartialEq)]
pub struct TriggerOptions {
    /// Upper bound of the uniform `[0, timing_spread_ms]` onset delay.
    /// `0.0` (the default) disables timing humanization.
    pub timing_spread_ms: f64,
}

impl Default for TriggerOptions {
    fn default() -> Self {
        Self {
            timing_spread_ms: 0.0,
        }
    }
}

/// Why a trigger produced no voice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TriggerRejectKind {
    /// No event with that id in the bank.
    UnknownEvent,
    /// The event names no clips — an authoring bug, never silence.
    EmptyPool,
    /// Inside `cooldown_ms` of the last accepted trigger.
    Cooldown,
    /// `max_polyphony == 0` mutes the event (no voice may live).
    PolyphonyBlocked,
}

/// A rejected trigger: which event, and why it made no sound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriggerReject {
    pub event_id: String,
    pub kind: TriggerRejectKind,
}

/// The live SFX runtime: seeded RNG plus per-event throttle state
/// (`last_accepted_ms`) and the currently tracked voices for polyphony.
#[derive(Debug)]
pub struct SfxRuntime {
    rng: SeededRng,
    last_accepted_ms: HashMap<String, u64>,
    active: Vec<Voice>,
}

impl SfxRuntime {
    pub fn new(seed: u64) -> Self {
        Self {
            rng: SeededRng::new(seed),
            last_accepted_ms: HashMap::new(),
            active: Vec::new(),
        }
    }

    /// Currently tracked (un-stolen, un-expired) voices.
    pub fn active_voices(&self) -> &[Voice] {
        &self.active
    }

    /// Tracked voices for one event.
    pub fn voices_for(&self, event_id: &str) -> Vec<&Voice> {
        self.active
            .iter()
            .filter(|v| v.event_id == event_id)
            .collect()
    }

    /// Drop voices older than `ttl_ms` relative to `now_ms`.
    pub fn expire_older_than(&mut self, now_ms: u64, ttl_ms: u64) {
        self.active.retain(|v| {
            now_ms.saturating_sub(v.started_ms) <= ttl_ms
        });
    }

    pub fn clear(&mut self) {
        self.active.clear();
        self.last_accepted_ms.clear();
    }

    /// Trigger `event_id` from `bank` under one game snapshot.
    ///
    /// `declared` is the bank author's param surface; snapshot values for
    /// undeclared params are ignored, and declared-but-missing values read
    /// as the param `default` (additive-params rule from `game-state.md`).
    pub fn trigger(
        &mut self,
        bank: &SfxBank,
        event_id: &str,
        snapshot: &GameStateSnapshot,
        declared: &[GameStateParam],
        now_ms: u64,
        opts: &TriggerOptions,
    ) -> Result<Voice, TriggerReject> {
        let reject = |kind: TriggerRejectKind| TriggerReject {
            event_id: event_id.to_string(),
            kind,
        };
        let event = bank
            .events
            .iter()
            .find(|e| e.id == event_id)
            .ok_or_else(|| reject(TriggerRejectKind::UnknownEvent))?;
        if event.clip_ids.is_empty() {
            return Err(reject(TriggerRejectKind::EmptyPool));
        }
        if let Some(&last) = self.last_accepted_ms.get(event_id) {
            if now_ms.saturating_sub(last) < event.cooldown_ms {
                return Err(reject(TriggerRejectKind::Cooldown));
            }
        }
        if event.max_polyphony == 0 {
            return Err(reject(TriggerRejectKind::PolyphonyBlocked));
        }

        let clip_id = event.clip_ids[self.rng.pick_index(event.clip_ids.len())].clone();
        let gain = event.volume + self.rng.range_f64(-event.volume_random, event.volume_random);
        let pitch_semitones =
            self.rng.range_f64(-event.pitch_random, event.pitch_random);
        let spread = opts.timing_spread_ms.max(0.0);
        let delay_ms = if spread > 0.0 {
            self.rng.range_f64(0.0, spread)
        } else {
            0.0
        };
        let rtpc = resolve_rtpc(event.rtpc.iter(), snapshot, declared);

        // Polyphony: steal oldest voices of this event until the new voice
        // fits. Oldest = smallest `started_ms`; ties break by position.
        let live = self.voices_for(event_id).len();
        if live >= event.max_polyphony as usize {
            let mut idx: Vec<usize> = self
                .active
                .iter()
                .enumerate()
                .filter(|(_, v)| v.event_id == event_id)
                .map(|(i, _)| i)
                .collect();
            idx.sort_by_key(|&i| self.active[i].started_ms);
            let to_drop = live - event.max_polyphony as usize + 1;
            for &i in idx.iter().take(to_drop) {
                self.active[i].event_id.push('\0'); // tombstone, swept below
            }
            self.active.retain(|v| !v.event_id.ends_with('\0'));
        }

        let voice = Voice {
            event_id: event_id.to_string(),
            clip_id,
            gain,
            pitch_semitones,
            delay_ms,
            started_ms: now_ms,
            rtpc,
        };
        self.active.push(voice.clone());
        self.last_accepted_ms.insert(event_id.to_string(), now_ms);
        Ok(voice)
    }
}

/// Map one trigger's RTPC bindings onto output values.
///
/// For each binding: look up the declared param — unknown names are skipped
/// (quiet tolerance); read the snapshot value or the declared `default`;
/// clamp into the declared `[min, max]`; normalize to 0–1; map linearly
/// onto the binding's `[min, max]`.
pub fn resolve_rtpc<'a, I>(
    bindings: I,
    snapshot: &GameStateSnapshot,
    declared: &[GameStateParam],
) -> Vec<ResolvedRtpc>
where
    I: Iterator<Item = &'a crate::game_audio::RtpcBinding>,
{
    bindings
        .filter_map(|b| {
            let param = declared.iter().find(|p| p.id == b.param)?;
            let raw = snapshot
                .values
                .iter()
                .find(|v| v.param == b.param)
                .map(|v| v.value)
                .unwrap_or(param.default);
            let lo = param.min.min(param.max);
            let hi = param.min.max(param.max);
            let clamped = raw.clamp(lo, hi);
            let t = if (hi - lo).abs() < f64::EPSILON {
                0.0
            } else {
                (clamped - lo) / (hi - lo)
            };
            Some(ResolvedRtpc {
                target_node: b.target_node.clone(),
                target_param: b.target_param.clone(),
                value: b.min + t * (b.max - b.min),
                min: b.min,
                max: b.max,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_audio::{GameStateValue, RtpcBinding, SfxEvent};

    fn event(id: &str) -> SfxEvent {
        SfxEvent {
            id: id.to_string(),
            name: id.to_string(),
            clip_ids: vec!["clip_a".to_string(), "clip_b".to_string(), "clip_c".to_string()],
            volume: 0.8,
            volume_random: 0.1,
            pitch_random: 2.0,
            cooldown_ms: 100,
            max_polyphony: 2,
            rtpc: vec![],
        }
    }

    fn bank_with(event: SfxEvent) -> SfxBank {
        SfxBank {
            schema_version: crate::game_audio::GAME_AUDIO_SCHEMA_VERSION,
            id: "bank_t".to_string(),
            name: "T".to_string(),
            events: vec![event],
        }
    }

    fn snap(values: Vec<(&str, f64)>) -> GameStateSnapshot {
        GameStateSnapshot {
            state: "combat".to_string(),
            values: values
                .into_iter()
                .map(|(param, value)| GameStateValue {
                    param: param.to_string(),
                    value,
                })
                .collect(),
        }
    }

    fn threat_param() -> GameStateParam {
        GameStateParam {
            id: "threat".to_string(),
            label: "Threat".to_string(),
            min: 0.0,
            max: 1.0,
            default: 0.0,
            unit: String::new(),
        }
    }

    #[test]
    fn pool_pick_is_uniform_over_time_and_seeded_reproducible() {
        let opts = TriggerOptions::default();
        let snap = GameStateSnapshot {
            state: String::new(),
            values: vec![],
        };
        // Same seed -> identical voice sequence (export reproducibility).
        let (mut a, mut b) = (SfxRuntime::new(7), SfxRuntime::new(7));
        let (bank_a, bank_b) = (bank_with(event("ev")), bank_with(event("ev")));
        let mut t = 0u64;
        for _ in 0..20 {
            // Cooldown is 100ms: step past it so every trigger is accepted.
            let va = a
                .trigger(&bank_a, "ev", &snap, &[], t, &opts)
                .expect("accepted");
            let vb = b
                .trigger(&bank_b, "ev", &snap, &[], t, &opts)
                .expect("accepted");
            assert_eq!(va, vb);
            t += 200;
        }
        // Uniform coverage: 60 triggers over 3 clips must touch every clip
        // (p = 3*(2/3)^60 ~= 0, so a miss means the pick is skewed).
        let mut c = SfxRuntime::new(99);
        let bank = bank_with(event("ev"));
        let mut seen = std::collections::HashSet::new();
        let mut t = 0u64;
        for _ in 0..60 {
            seen.insert(
                c.trigger(&bank, "ev", &snap, &[], t, &opts)
                    .expect("accepted")
                    .clip_id,
            );
            t += 200;
        }
        assert_eq!(seen.len(), 3, "pool pick must cover all clips");
    }

    #[test]
    fn humanization_stays_inside_declared_spreads() {
        let mut e = event("ev");
        e.cooldown_ms = 0;
        e.max_polyphony = u32::MAX;
        let bank = bank_with(e);
        let mut rt = SfxRuntime::new(3);
        let snap = GameStateSnapshot {
            state: String::new(),
            values: vec![],
        };
        let opts = TriggerOptions {
            timing_spread_ms: 15.0,
        };
        for t in 0..50u64 {
            let v = rt
                .trigger(&bank, "ev", &snap, &[], t, &opts)
                .expect("accepted");
            assert!((v.gain - 0.8).abs() <= 0.1 + 1e-9, "gain {}", v.gain);
            assert!(v.pitch_semitones.abs() <= 2.0 + 1e-9, "pitch {}", v.pitch_semitones);
            assert!((0.0..=15.0).contains(&v.delay_ms), "delay {}", v.delay_ms);
        }
    }

    #[test]
    fn empty_pool_and_unknown_event_reject_loudly() {
        let mut e = event("ev");
        e.clip_ids.clear();
        let bank = bank_with(e);
        let mut rt = SfxRuntime::new(1);
        let snap = GameStateSnapshot {
            state: String::new(),
            values: vec![],
        };
        let opts = TriggerOptions::default();
        assert_eq!(
            rt.trigger(&bank, "ev", &snap, &[], 0, &opts)
                .expect_err("empty pool must reject")
                .kind,
            TriggerRejectKind::EmptyPool
        );
        assert_eq!(
            rt.trigger(&bank, "nope", &snap, &[], 0, &opts)
                .expect_err("unknown event must reject")
                .kind,
            TriggerRejectKind::UnknownEvent
        );
    }

    #[test]
    fn cooldown_drops_bursts_and_polyphony_steals_oldest() {
        let bank = bank_with(event("ev")); // cooldown 100ms, polyphony 2
        let mut rt = SfxRuntime::new(5);
        let snap = GameStateSnapshot {
            state: String::new(),
            values: vec![],
        };
        let opts = TriggerOptions::default();
        rt.trigger(&bank, "ev", &snap, &[], 0, &opts)
            .expect("t=0 accepted");
        assert_eq!(
            rt.trigger(&bank, "ev", &snap, &[], 50, &opts)
                .expect_err("inside cooldown")
                .kind,
            TriggerRejectKind::Cooldown
        );
        rt.trigger(&bank, "ev", &snap, &[], 100, &opts)
            .expect("cooldown edge accepts");
        rt.trigger(&bank, "ev", &snap, &[], 200, &opts)
            .expect("third voice steals oldest");
        let voices = rt.voices_for("ev");
        assert_eq!(voices.len(), 2, "polyphony cap holds");
        assert!(
            voices.iter().all(|v| v.started_ms >= 100),
            "oldest (t=0) must be stolen"
        );
    }

    #[test]
    fn rtpc_clamps_normalizes_and_maps_linearly() {
        let mut e = event("ev");
        e.rtpc.push(RtpcBinding {
            param: "threat".to_string(),
            target_node: "bus_sfx".to_string(),
            target_param: "volume".to_string(),
            min: 0.5,
            max: 1.0,
        });
        e.rtpc.push(RtpcBinding {
            param: "future_param".to_string(), // undeclared: ignored quietly
            target_node: "bus_sfx".to_string(),
            target_param: "volume".to_string(),
            min: 0.0,
            max: 1.0,
        });
        let bank = bank_with(e);
        let declared = vec![threat_param()];
        let mut rt = SfxRuntime::new(11);
        let opts = TriggerOptions::default();
        let mut t = 0u64;
        let mut trig = |value: f64| {
            let v = rt
                .trigger(&bank, "ev", &snap(vec![("threat", value)]), &declared, t, &opts)
                .expect("accepted");
            t += 200;
            v
        };
        assert_eq!(trig(0.0).rtpc.len(), 1, "unknown param ignored");
        assert!((trig(0.0).rtpc[0].value - 0.5).abs() < 1e-9);
        assert!((trig(1.0).rtpc[0].value - 1.0).abs() < 1e-9);
        assert!((trig(0.5).rtpc[0].value - 0.75).abs() < 1e-9);
        // Clamping: out-of-range game values pin to the output ends.
        assert!((trig(-3.0).rtpc[0].value - 0.5).abs() < 1e-9);
        assert!((trig(42.0).rtpc[0].value - 1.0).abs() < 1e-9);
        // Missing value reads as the declared default (0.0 -> 0.5 here).
        let v = rt
            .trigger(
                &bank,
                "ev",
                &GameStateSnapshot {
                    state: "combat".to_string(),
                    values: vec![],
                },
                &declared,
                t,
                &opts,
            )
            .expect("accepted");
        assert!((v.rtpc[0].value - 0.5).abs() < 1e-9);
    }
}
