//! Track Q (Agent 8): ARA (Audio Random Access) host seam — phase-2 hooks only.
//!
//! New to ARA? Most plugin formats (VST3, CLAP, AU) stream audio *through*
//! the plugin buffer-by-buffer: the plugin never sees the whole file. ARA
//! flips that around for editing-style plugins (pitch/time tools like
//! Melodyne): the **host owns a document model** (audio sources, regions
//! placed on a musical timeline, tempo/key context) and the plugin gets
//! **random access** to the actual audio, so it can analyze a whole phrase
//! at once and follow later timeline edits. The two halves are:
//!
//! - **Document model** (host side): audio sources + regions + musical
//!   context. The host builds it from the timeline; the plugin reads it.
//! - **Playback rendering** (plugin side): the plugin renders its (possibly
//!   edited) audio back into the host's mix for a region range.
//!
//! This module is the v1 seam for that future, mirroring the sibling
//! phase-2 seams ([`HostError::ClapUnimplemented`](crate::plugins::host)
//! in `host.rs`, the `create_instance` follow-up in `vst3.rs`, the
//! render-callback follow-up in `au.rs`): it names the host-side concepts,
//! marks every entry point, refuses, and never branches. No new cargo
//! dependencies, no frozen-contract surface (these types are addressing
//! scaffolding, never written to project files — same rule as
//! [`Vst3Descriptor`](crate::plugins::vst3::Vst3Descriptor)).
//!
//! The evaluation that picked this shape — why ARA is the one format of
//! the three to scope now — lives in `docs/notes/q-formats.md`.

use std::fmt;

/// Why ARA calls below refuse: the document model + analysis renderer are
/// the phase-2 follow-up (see `docs/notes/q-formats.md` § ARA).
pub const ARA_SEAM_NOTE: &str =
    "ARA document hosting is a phase-2 seam (see docs/notes/q-formats.md); no document is hosted yet";

/// Opaque address of an audio source the host would share with an ARA
/// plugin (e.g. a timeline clip's decoded audio). Uninterpreted bytes-by-id
/// for now — the phase-2 document model fills in sample access.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AraAudioSourceId(pub String);

/// One placed slice of an [`AraAudioSourceId`] on the musical timeline, in
/// seconds. Start/end only: tempo/key following and edit-granularity arrive
/// with the phase-2 musical-context model.
#[derive(Debug, Clone, PartialEq)]
pub struct AraRegion {
    pub source: AraAudioSourceId,
    pub start_sec: f64,
    pub end_sec: f64,
}

impl AraRegion {
    /// `None` when the range is empty or inverted — the host must never hand
    /// a plugin a backwards region, so the seam checks now, not later.
    pub fn new(source: &str, start_sec: f64, end_sec: f64) -> Option<Self> {
        if !start_sec.is_finite() || !end_sec.is_finite() || end_sec <= start_sec {
            return None;
        }
        Some(Self {
            source: AraAudioSourceId(source.to_string()),
            start_sec,
            end_sec,
        })
    }

    /// Length in seconds. Always positive by construction.
    pub fn len_sec(&self) -> f64 {
        self.end_sec - self.start_sec
    }
}

/// What can go wrong at the ARA seam. Only one arm can fire in v1.
#[derive(Debug, Clone, PartialEq)]
pub enum AraError {
    /// Every host operation lands here until phase 2 wires the document
    /// model. Mirrors `HostError::ClapUnimplemented`: mark, refuse, never
    /// branch.
    AraUnimplemented(&'static str),
    /// A region failed validation (kept distinct so callers can tell "bad
    /// input" from "not built yet").
    BadRegion(String),
}

impl fmt::Display for AraError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AraUnimplemented(op) => {
                write!(f, "ARA `{op}` is not hosted yet ({ARA_SEAM_NOTE})")
            }
            Self::BadRegion(m) => write!(f, "bad ARA region: {m}"),
        }
    }
}

impl std::error::Error for AraError {}

pub type Result<T> = std::result::Result<T, AraError>;

/// Phase-2 host handle. v1 constructs it fine (zero state) so call sites
/// can name the owner of future document-model work; every operation that
/// would touch a plugin refuses at the seam.
#[derive(Debug, Default)]
pub struct AraHost;

impl AraHost {
    /// New (empty) host handle. Infallible by design: there is nothing to
    /// connect to yet.
    pub fn new() -> Self {
        Self
    }

    /// `true` once an ARA plugin can actually be hosted. `false` for all of
    /// v1 — feature probes must see the deferred state, not guess it.
    pub fn is_available(&self) -> bool {
        false
    }

    /// Share one timeline clip's audio with an ARA plugin for analysis.
    /// Phase-2 work: register the source in the document model and return
    /// its id. v1 refuses.
    pub fn share_source(&self, _clip_id: &str) -> Result<AraAudioSourceId> {
        Err(AraError::AraUnimplemented("share_source"))
    }

    /// Render an ARA plugin's (possibly edited) audio for `region` into the
    /// host mix. Phase-2 work: drive the plugin's playback renderer over
    /// the range. v1 refuses.
    pub fn render_region(&self, _region: &AraRegion) -> Result<()> {
        Err(AraError::AraUnimplemented("render_region"))
    }

    /// Drop every document-model object for `clip_id`. Idempotent in phase
    /// 2; v1 refuses like every other operation so callers cannot assume a
    /// document exists.
    pub fn detach_clip(&self, _clip_id: &str) -> Result<()> {
        Err(AraError::AraUnimplemented("detach_clip"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_rejects_empty_and_inverted_ranges() {
        assert!(AraRegion::new("clip-a", 1.0, 1.0).is_none());
        assert!(AraRegion::new("clip-a", 2.0, 1.0).is_none());
        assert!(AraRegion::new("clip-a", f64::NAN, 2.0).is_none());
        let ok = AraRegion::new("clip-a", 1.0, 2.5).expect("valid range");
        assert_eq!(ok.len_sec(), 1.5);
    }

    #[test]
    fn host_is_unavailable_and_refuses_at_the_seam() {
        let host = AraHost::new();
        assert!(!host.is_available());
        assert!(matches!(
            host.share_source("clip-a"),
            Err(AraError::AraUnimplemented("share_source"))
        ));
        let region = AraRegion::new("clip-a", 0.0, 1.0).expect("valid range");
        assert!(matches!(
            host.render_region(&region),
            Err(AraError::AraUnimplemented("render_region"))
        ));
        assert!(matches!(
            host.detach_clip("clip-a"),
            Err(AraError::AraUnimplemented("detach_clip"))
        ));
    }
}
