//! Track Q: ARA (Audio Random Access) document hosting + mock effect path.
//!
//! New to ARA? Most plugin formats (VST3, CLAP, AU) stream audio *through*
//! the plugin buffer-by-buffer: the plugin never sees the whole file. ARA
//! flips that around for editing-style plugins (pitch/time tools like
//! Melodyne): the **host owns a document model** (audio sources, regions
//! placed on a musical timeline, tempo/key context) and the plugin gets
//! **random access** to the actual audio, so it can analyze a whole phrase
//! at once and follow later timeline edits. The two halves are:
//!
//! - **Document model** (host side): [`AraDocument`] — audio sources with
//!   sample access ([`AraAudioSource`]), region sequences mapped from
//!   arrangement clips ([`AraRegionEntry`]), and musical context
//!   ([`MusicalContext`]: tempo/key/meter). The host builds it from the
//!   timeline; the plugin reads it.
//! - **Playback rendering** (plugin side): [`MockAraEffect`] renders
//!   (possibly edited) audio back into the host's mix for a region range.
//!   It is a hand-rolled test double — region gain, per-note gains from a
//!   real zero-crossing analysis, and a length-preserving naive transpose —
//!   not a production pitch engine. It exists so the edit round-trip
//!   (analyze → edit notes → render reflects the edit) is tested without
//!   any third-party binary.
//!
//! What this module does NOT do (explicit follow-up, no code): hosting real
//! third-party ARA binaries. That needs two pieces that do not exist here:
//!
//! 1. **Factory entry hosting** — ARA plugins ship *inside* VST3/AU
//!    binaries, so the host must load the binary through the existing
//!    [`Vst3Backend`](crate::plugins::vst3::Vst3Backend) /
//!    [`AuBackend`](crate::plugins::au::AuBackend), then perform the ARA
//!    factory handshake (`ARAFactory::createDocumentController` negotiation
//!    over the plug-in extension). No ARA entry-point handshake code exists
//!    in this repo; [`AraHost::load_binary_plugin`] names the seam and
//!    refuses.
//! 2. **Document controller lifecycle** — creating/destroying the
//!    `ARADocumentController`, binding this document's sources/regions to
//!    its objects, driving the analysis + playback-renderer roles, and
//!    invalidating on timeline edits. The [`AraDocument`] shape is designed
//!    to feed that controller when it lands; nothing here binds to one.
//!
//! No new cargo dependencies: the model is plain structs, analysis is
//! zero-crossing counting plus linear-interpolation resampling (~40
//! lines). An ARA SDK crate would drag C++ bindings and license weight for
//! test-only value — hand-rolling wins until real binary hosting lands.
//! These types are addressing scaffolding, never written to project files
//! (same rule as [`Vst3Descriptor`](crate::plugins::vst3::Vst3Descriptor)),
//! so the typegen drift gate is unaffected.
//!
//! The evaluation that picked this shape lives in `docs/notes/q-formats.md`.

use std::collections::BTreeMap;
use std::fmt;

use crate::model::{Clip, Project};

/// Why real-binary ARA calls refuse: factory entry hosting + document
/// controller lifecycle are the follow-up (see module docs and
/// `docs/notes/q-formats.md` § ARA).
pub const ARA_SEAM_NOTE: &str =
    "real ARA binary hosting needs factory entry hosting + document controller lifecycle (see docs/notes/q-formats.md); the document model + mock effect path below is live";

/// Opaque address of an audio source the host shares with an ARA
/// plugin (e.g. a timeline clip's decoded audio). The phase-2 document
/// model fills in sample access via [`AraAudioSource`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AraAudioSourceId(pub String);

/// One placed slice of an [`AraAudioSourceId`] on the musical timeline, in
/// seconds. Start/end only at the region level; tempo/key following lives
/// in [`MusicalContext`], edit granularity in [`AraRegionEntry`].
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

/// Musical context of an [`AraDocument`]: the tempo/key/meter the regions
/// are placed against. A tempo change re-maps every beats↔seconds
/// conversion through here, so plugins following the timeline read the new
/// context instead of caching seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct MusicalContext {
    /// Beats per minute. Always finite and positive by construction.
    pub tempo_bpm: f64,
    /// Key tonic as pitch class (C = 0 … B = 11). `None` means no key
    /// (atonal / unknown) — the plugin must not assume C major.
    pub key_tonic: Option<u8>,
    /// `true` = minor, `false` = major. Only meaningful when `key_tonic`
    /// is `Some`.
    pub key_minor: bool,
    /// Meter numerator (beats per bar). Informational for now.
    pub time_sig_num: u8,
    /// Meter denominator (note value per beat). Informational for now.
    pub time_sig_den: u8,
}

impl MusicalContext {
    /// `None` when `tempo_bpm` is non-finite or non-positive — a document
    /// with no usable tempo must not exist.
    pub fn new(tempo_bpm: f64) -> Option<Self> {
        if !tempo_bpm.is_finite() || tempo_bpm <= 0.0 {
            return None;
        }
        Some(Self {
            tempo_bpm,
            key_tonic: None,
            key_minor: false,
            time_sig_num: 4,
            time_sig_den: 4,
        })
    }

    /// Attach a key. `None` when `tonic > 11`.
    pub fn with_key(mut self, tonic: u8, minor: bool) -> Option<Self> {
        if tonic > 11 {
            return None;
        }
        self.key_tonic = Some(tonic);
        self.key_minor = minor;
        Some(self)
    }

    /// Attach a meter. Zero values are kept as-is (unknown meter renders
    /// as 0, never as a silent default the caller did not ask for).
    pub fn with_meter(mut self, num: u8, den: u8) -> Self {
        self.time_sig_num = num;
        self.time_sig_den = den;
        self
    }

    /// Beats → seconds at this context's tempo. Mirrors
    /// [`crate::timeline::beats_to_seconds`].
    pub fn beats_to_sec(&self, beats: f64) -> f64 {
        beats * 60.0 / self.tempo_bpm
    }

    /// Seconds → beats at this context's tempo.
    pub fn sec_to_beats(&self, sec: f64) -> f64 {
        sec * self.tempo_bpm / 60.0
    }

    /// Context for a project: its tempo (120 BPM fallback when the project
    /// carries no usable tempo) and its meter. Key defaults to unknown —
    /// the project model stores no key yet, and guessing C major would lie
    /// to the plugin.
    pub fn from_project(project: &Project) -> Self {
        let tempo = if project.tempo.is_finite() && project.tempo > 0.0 {
            project.tempo
        } else {
            120.0
        };
        Self {
            tempo_bpm: tempo,
            key_tonic: None,
            key_minor: false,
            time_sig_num: project.time_sig_num,
            time_sig_den: project.time_sig_den,
        }
    }
}

impl Default for MusicalContext {
    fn default() -> Self {
        Self {
            tempo_bpm: 120.0,
            key_tonic: None,
            key_minor: false,
            time_sig_num: 4,
            time_sig_den: 4,
        }
    }
}

/// One audio source in the document: the full sample buffer the plugin may
/// randomly access, plus the rate that gives the samples time meaning.
/// Mono f32, matching the offline bounce stems that feed it.
#[derive(Debug, Clone, PartialEq)]
pub struct AraAudioSource {
    pub id: AraAudioSourceId,
    pub sample_rate: u32,
    pub samples: Vec<f32>,
}

impl AraAudioSource {
    pub fn len_frames(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Duration in seconds. 0.0 when the rate is 0 (a source that cannot
    /// carry time) or the buffer is empty.
    pub fn duration_sec(&self) -> f64 {
        if self.sample_rate == 0 || self.samples.is_empty() {
            return 0.0;
        }
        self.samples.len() as f64 / self.sample_rate as f64
    }

    /// Nearest sample at `t_sec`, clamped to the buffer. 0.0 outside a
    /// timed buffer (empty or rate 0) — silence, never a panic.
    pub fn sample_at(&self, t_sec: f64) -> f32 {
        if self.sample_rate == 0 || self.samples.is_empty() || !t_sec.is_finite() {
            return 0.0;
        }
        let idx = (t_sec * self.sample_rate as f64).round() as i64;
        let idx = idx.clamp(0, self.samples.len() as i64 - 1) as usize;
        self.samples[idx]
    }

    /// Copy of `[start_sec, end_sec)` at the source rate, clamped to the
    /// buffer. Empty when the range is inverted, non-finite, or untimed.
    pub fn read_range(&self, start_sec: f64, end_sec: f64) -> Vec<f32> {
        if self.sample_rate == 0
            || self.samples.is_empty()
            || !start_sec.is_finite()
            || !end_sec.is_finite()
            || end_sec <= start_sec
        {
            return Vec::new();
        }
        let sr = self.sample_rate as f64;
        let from = (start_sec * sr).round().max(0.0) as usize;
        let to = (end_sec * sr).round().max(0.0) as usize;
        let to = to.min(self.samples.len());
        if from >= to {
            return Vec::new();
        }
        self.samples[from..to].to_vec()
    }
}

/// One arrangement clip mapped into the document: which source slice plays
/// on which timeline span, plus where that slice starts inside the source
/// buffer. Clips and sources live on different time bases (a source buffer
/// may start at a bounce-window offset while the region is in
/// project-timeline seconds), so `source_offset_sec` pins them together:
/// region time `t` reads source time `t - region.start_sec +
/// source_offset_sec`.
#[derive(Debug, Clone, PartialEq)]
pub struct AraRegionEntry {
    /// Document-unique id (`ara-region-N`).
    pub id: String,
    /// Arrangement clip this entry was mapped from.
    pub clip_id: String,
    /// Timeline span + source identity.
    pub region: AraRegion,
    /// Source-buffer time of `region.start_sec`.
    pub source_offset_sec: f64,
}

/// What can go wrong in the ARA document model.
#[derive(Debug, Clone, PartialEq)]
pub enum AraError {
    /// Real-binary hosting (factory entry + document controller) is not
    /// built yet. The document model + mock effect path below it succeed;
    /// only binary loading lands here.
    AraUnimplemented(&'static str),
    /// A region failed validation (kept distinct so callers can tell "bad
    /// input" from "not built yet").
    BadRegion(String),
    /// No source with that id is registered.
    UnknownSource(String),
    /// A scalar argument (rate, count, id) is unusable.
    BadArgument(String),
}

impl fmt::Display for AraError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AraUnimplemented(op) => {
                write!(f, "ARA `{op}` is not hosted yet ({ARA_SEAM_NOTE})")
            }
            Self::BadRegion(m) => write!(f, "bad ARA region: {m}"),
            Self::UnknownSource(id) => write!(f, "unknown ARA source `{id}`"),
            Self::BadArgument(m) => write!(f, "bad ARA argument: {m}"),
        }
    }
}

impl std::error::Error for AraError {}

pub type Result<T> = std::result::Result<T, AraError>;

/// The host-owned ARA document: musical context, audio sources with sample
/// access, and the region sequence mapped from arrangement clips.
///
/// Two-phase build is intentional: [`AraDocument::from_project`] maps every
/// clip to a region entry first (sources empty), then the caller shares
/// each clip's audio with [`AraDocument::add_source`] (the offline bounce
/// path in [`crate::bounce`] does exactly this). Rendering checks both
/// phases — an effect with no region, or a region with no source, is
/// ignored / errors instead of guessing.
#[derive(Debug, Clone, Default)]
pub struct AraDocument {
    context: MusicalContext,
    sources: BTreeMap<String, AraAudioSource>,
    regions: Vec<AraRegionEntry>,
    next_region: u64,
}

impl AraDocument {
    pub fn new(context: MusicalContext) -> Self {
        Self {
            context,
            sources: BTreeMap::new(),
            regions: Vec::new(),
            next_region: 0,
        }
    }

    /// Map every valid arrangement clip to a region entry. Source ids are
    /// the clip ids — the caller then shares each clip's audio under its
    /// id. Clips with empty/inverted/non-finite spans are skipped (mapping
    /// is lossy by rule, and [`AraRegion::new`] owns that rule).
    /// Source offsets are 0.0 here: the source buffer is assumed to start
    /// where the region starts; callers with a different buffer base (e.g.
    /// a bounce window) fix it with [`AraDocument::set_source_offset`].
    pub fn from_project(project: &Project) -> Self {
        let mut doc = Self::new(MusicalContext::from_project(project));
        for clip in &project.clips {
            let _ = doc.add_region_for_clip(clip, &clip.id);
        }
        doc
    }

    pub fn context(&self) -> &MusicalContext {
        &self.context
    }

    pub fn set_context(&mut self, context: MusicalContext) {
        self.context = context;
    }

    /// Register (or replace) a source's audio. Empty `id` or a 0 rate
    /// refuses — a source that cannot be addressed or timed poisons
    /// every region pointing at it.
    pub fn add_source(
        &mut self,
        id: &str,
        sample_rate: u32,
        samples: Vec<f32>,
    ) -> Result<AraAudioSourceId> {
        if id.is_empty() {
            return Err(AraError::BadArgument("source id must not be empty".to_string()));
        }
        if sample_rate == 0 {
            return Err(AraError::BadArgument(format!(
                "source `{id}` needs a nonzero sample rate"
            )));
        }
        let sid = AraAudioSourceId(id.to_string());
        self.sources.insert(
            id.to_string(),
            AraAudioSource {
                id: sid.clone(),
                sample_rate,
                samples,
            },
        );
        Ok(sid)
    }

    pub fn get_source(&self, id: &str) -> Option<&AraAudioSource> {
        self.sources.get(id)
    }

    pub fn source_count(&self) -> usize {
        self.sources.len()
    }

    pub fn region_count(&self) -> usize {
        self.regions.len()
    }

    /// Place an explicit second-range region. The source must already be
    /// shared; the range must validate.
    pub fn add_region(
        &mut self,
        clip_id: &str,
        source_id: &str,
        start_sec: f64,
        end_sec: f64,
        source_offset_sec: f64,
    ) -> Result<String> {
        if !self.sources.contains_key(source_id) {
            return Err(AraError::UnknownSource(source_id.to_string()));
        }
        let region = AraRegion::new(source_id, start_sec, end_sec).ok_or_else(|| {
            AraError::BadRegion(format!("range [{start_sec}, {end_sec}) is empty or invalid"))
        })?;
        Ok(self.push_region(clip_id, region, source_offset_sec))
    }

    /// Map one arrangement clip to a region entry: clip beats → seconds at
    /// the document tempo. Returns `Ok(None)` (skip, not error) when the
    /// clip span is empty/invalid. Deliberately does NOT require the source
    /// to be shared yet — mapping runs first, audio follows via
    /// [`AraDocument::add_source`]; rendering refuses unshared sources.
    pub fn add_region_for_clip(&mut self, clip: &Clip, source_id: &str) -> Result<Option<String>> {
        if !clip.start_beats.is_finite() || !clip.length_beats.is_finite() || clip.length_beats <= 0.0
        {
            return Ok(None);
        }
        let start_sec = self.context.beats_to_sec(clip.start_beats);
        let end_sec = self.context.beats_to_sec(clip.start_beats + clip.length_beats);
        let region = AraRegion::new(source_id, start_sec, end_sec).ok_or_else(|| {
            AraError::BadRegion(format!("range [{start_sec}, {end_sec}) is empty or invalid"))
        })?;
        Ok(Some(self.push_region(&clip.id.clone(), region, 0.0)))
    }

    fn push_region(&mut self, clip_id: &str, region: AraRegion, source_offset_sec: f64) -> String {
        let id = format!("ara-region-{}", self.next_region);
        self.next_region += 1;
        self.regions.push(AraRegionEntry {
            id: id.clone(),
            clip_id: clip_id.to_string(),
            region,
            source_offset_sec,
        });
        id
    }

    /// Retarget a region entry's source-buffer base (see
    /// [`AraRegionEntry::source_offset_sec`]). `false` when no entry has
    /// that id.
    pub fn set_source_offset(&mut self, region_id: &str, offset_sec: f64) -> bool {
        match self.regions.iter_mut().find(|e| e.id == region_id) {
            Some(e) => {
                e.source_offset_sec = offset_sec;
                true
            }
            None => false,
        }
    }

    pub fn regions(&self) -> &[AraRegionEntry] {
        &self.regions
    }

    pub fn regions_for_clip(&self, clip_id: &str) -> Vec<&AraRegionEntry> {
        self.regions.iter().filter(|e| e.clip_id == clip_id).collect()
    }

    /// Entries whose span intersects `[start_sec, end_sec)`.
    pub fn regions_overlapping(&self, start_sec: f64, end_sec: f64) -> Vec<&AraRegionEntry> {
        self.regions
            .iter()
            .filter(|e| e.region.start_sec < end_sec && e.region.end_sec > start_sec)
            .collect()
    }

    /// Drop every source and region entry for `clip_id`. Idempotent:
    /// returns the number of region entries removed (0 when the clip was
    /// never mapped — detaching twice is not an error).
    pub fn remove_clip(&mut self, clip_id: &str) -> usize {
        let clip_key = clip_id.to_string();
        self.sources.remove(&clip_key);
        // Sources are keyed by clip id by convention (`from_project` +
        // `add_source`); a region may also point at a shared source owned
        // by another clip, so only the entries are counted.
        let before = self.regions.len();
        self.regions.retain(|e| e.clip_id != clip_id);
        before - self.regions.len()
    }
}

/// One analyzed note inside a region: absolute region-timeline span, a
/// measured pitch, and an editable gain. Analysis fills these in;
/// [`MockAraEffect::render`] reads them back — editing a note changes the
/// render, which is the round-trip the tests prove.
#[derive(Debug, Clone, PartialEq)]
pub struct AraNote {
    pub id: String,
    pub start_sec: f64,
    pub duration_sec: f64,
    /// Measured pitch (MIDI number) from [`MockAraEffect::analyze`].
    pub pitch_midi: u8,
    /// Editable per-note gain. 1.0 = analysis default, 0.0 = muted.
    pub gain: f32,
}

impl AraNote {
    /// `true` when region-timeline time `t` falls inside this note.
    pub fn contains(&self, t: f64) -> bool {
        t >= self.start_sec && t < self.start_sec + self.duration_sec
    }
}

/// Mock ARA test effect: region-wide gain, a naive transpose, and editable
/// analysis notes. The DSP is deliberately placeholder-grade (documented
/// per method); its contract is determinism and length preservation, so
/// tests can prove the host↔plugin edit loop without a real binary.
#[derive(Debug, Clone, PartialEq)]
pub struct MockAraEffect {
    /// Region-wide gain applied after per-note gains.
    pub region_gain: f32,
    /// Pitch shift in semitones, clamped to ±48 in render.
    pub transpose_semitones: i32,
    /// Analysis notes (empty = gain-only effect).
    pub notes: Vec<AraNote>,
}

impl MockAraEffect {
    pub fn new(region_gain: f32, transpose_semitones: i32) -> Self {
        Self {
            region_gain,
            transpose_semitones,
            notes: Vec::new(),
        }
    }

    /// Analyze `source` over `[region_start_sec, region_start_sec +
    /// region_len_sec)`: split the span into `note_count` equal notes and
    /// measure each note's pitch by zero-crossing rate. Empty/tone-less
    /// notes report MIDI 69 (A4) rather than failing — analysis must never
    /// refuse a renderable region.
    pub fn analyze(
        source: &[f32],
        sample_rate: u32,
        region_start_sec: f64,
        region_len_sec: f64,
        note_count: usize,
    ) -> Result<Self> {
        if sample_rate == 0 {
            return Err(AraError::BadArgument("analyze needs a nonzero sample rate".to_string()));
        }
        if !region_len_sec.is_finite() || region_len_sec <= 0.0 {
            return Err(AraError::BadRegion(format!(
                "analyze needs a positive region length, got {region_len_sec}"
            )));
        }
        if note_count == 0 {
            return Err(AraError::BadArgument("analyze needs note_count > 0".to_string()));
        }
        let sr = sample_rate as f64;
        let note_len = region_len_sec / note_count as f64;
        let mut notes = Vec::with_capacity(note_count);
        for i in 0..note_count {
            let start = region_start_sec + i as f64 * note_len;
            let from = ((start * sr).round().max(0.0)) as usize;
            let to = (((start + note_len) * sr).round().max(0.0) as usize).min(source.len());
            let pitch_midi = if from < to {
                pitch_of(&source[from..to], sample_rate)
            } else {
                69
            };
            notes.push(AraNote {
                id: format!("note-{i}"),
                start_sec: start,
                duration_sec: note_len,
                pitch_midi,
                gain: 1.0,
            });
        }
        Ok(Self {
            region_gain: 1.0,
            transpose_semitones: 0,
            notes,
        })
    }

    pub fn note(&self, id: &str) -> Option<&AraNote> {
        self.notes.iter().find(|n| n.id == id)
    }

    /// Edit one note's gain. `false` (not an error) when no note has that
    /// id — a stale edit after re-analysis must not fail a session.
    /// Non-finite gains are clamped to 0.0 rather than poisoning audio.
    pub fn set_note_gain(&mut self, note_id: &str, gain: f32) -> bool {
        match self.notes.iter_mut().find(|n| n.id == note_id) {
            Some(n) => {
                n.gain = if gain.is_finite() { gain } else { 0.0 };
                true
            }
            None => false,
        }
    }

    /// Render `input` (one region span, `input.len()` frames at
    /// `sample_rate`) with the current edits. Length-preserving by
    /// construction: output length always equals input length.
    ///
    /// Two stages, both placeholder-grade and documented as such: (1) a
    /// naive transpose — linear-interpolation resampling at
    /// `2^(st/12)`, padded with silence / truncated to keep the length
    /// (a real engine would time-correct; this one audibly squeezes, which
    /// is fine for proving the loop); (2) per-note gains by region time
    /// (`region_start_sec + frame/sr` inside a note picks up its gain)
    /// times `region_gain`.
    pub fn render(&self, input: &[f32], sample_rate: u32, region_start_sec: f64) -> Vec<f32> {
        if input.is_empty() {
            return Vec::new();
        }
        let stretched = if self.transpose_semitones == 0 {
            input.to_vec()
        } else {
            let st = self.transpose_semitones.clamp(-48, 48) as f64;
            transpose_resample(input, 2f64.powf(st / 12.0))
        };
        let sr = if sample_rate == 0 {
            44100.0
        } else {
            sample_rate as f64
        };
        let start = if region_start_sec.is_finite() {
            region_start_sec
        } else {
            0.0
        };
        stretched
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let t = start + i as f64 / sr;
                let mut g = self.region_gain;
                if let Some(note) = self.notes.iter().find(|n| n.contains(t)) {
                    g *= note.gain;
                }
                s * g
            })
            .collect()
    }
}

/// Zero-crossing pitch estimate of one note's samples: `freq =
/// crossings * sr / (2 * n)`, converted to MIDI. Returns 69 when the
/// segment is empty, silent (no crossings), or non-positive — analysis
/// reports a default, never an error.
fn pitch_of(samples: &[f32], sample_rate: u32) -> u8 {
    if samples.len() < 2 || sample_rate == 0 {
        return 69;
    }
    let mut crossings = 0u32;
    for pair in samples.windows(2) {
        if (pair[0] < 0.0) != (pair[1] < 0.0) {
            crossings += 1;
        }
    }
    if crossings == 0 {
        return 69;
    }
    let freq = f64::from(crossings) * f64::from(sample_rate) / (2.0 * samples.len() as f64);
    if !freq.is_finite() || freq <= 0.0 {
        return 69;
    }
    (69.0 + 12.0 * (freq / 440.0).log2()).round().clamp(0.0, 127.0) as u8
}

/// Length-preserving linear-interpolation resample: `out[i] =
/// interp(input, i * factor)`, zeros past the end. `factor > 1` shifts
/// pitch up (content shortens into silence), `factor < 1` shifts down
/// (content extends past the window and truncates).
fn transpose_resample(input: &[f32], factor: f64) -> Vec<f32> {
    if !factor.is_finite() || factor <= 0.0 {
        return vec![0.0; input.len()];
    }
    let mut out = vec![0.0f32; input.len()];
    for (i, dst) in out.iter_mut().enumerate() {
        let pos = i as f64 * factor;
        let lo = pos.floor() as usize;
        if lo >= input.len() {
            break;
        }
        let frac = (pos - lo as f64) as f32;
        let a = input[lo];
        let b = if lo + 1 < input.len() { input[lo + 1] } else { 0.0 };
        *dst = a + (b - a) * frac;
    }
    out
}

/// Host handle: owns the [`AraDocument`] real operations run against.
/// `is_available()` is `true` — the document model + mock effect path is
/// live. Loading real third-party binaries stays a follow-up
/// ([`AraHost::load_binary_plugin`] names exactly what is missing).
#[derive(Debug, Default)]
pub struct AraHost {
    document: AraDocument,
}

impl AraHost {
    /// New host with an empty document (120 BPM, 4/4, unknown key).
    pub fn new() -> Self {
        Self::default()
    }

    /// New host around an existing document (e.g. mapped from a project).
    pub fn with_document(document: AraDocument) -> Self {
        Self { document }
    }

    pub fn document(&self) -> &AraDocument {
        &self.document
    }

    pub fn document_mut(&mut self) -> &mut AraDocument {
        &mut self.document
    }

    /// `true`: the document model + mock effect path is hosted. (Real
    /// third-party binary hosting is still the follow-up — see
    /// [`AraHost::load_binary_plugin`].)
    pub fn is_available(&self) -> bool {
        true
    }

    /// Share one timeline clip's audio: register `samples` (mono f32 at
    /// `sample_rate`) as the clip's source and return its id. Re-sharing
    /// replaces the audio (timeline edits re-share; the plugin re-analyzes).
    pub fn share_source(
        &mut self,
        clip_id: &str,
        sample_rate: u32,
        samples: Vec<f32>,
    ) -> Result<AraAudioSourceId> {
        self.document.add_source(clip_id, sample_rate, samples)
    }

    /// Render an ARA effect's edited audio for a clip's first region:
    /// slice the source at the entry's offset, run the effect, return the
    /// region-length buffer. Errors when the clip has no region
    /// ([`AraError::BadRegion`]) or the region's source is unshared
    /// ([`AraError::UnknownSource`]).
    pub fn render_region(
        &self,
        clip_id: &str,
        effect: &MockAraEffect,
    ) -> Result<Vec<f32>> {
        let entry = self
            .document
            .regions_for_clip(clip_id)
            .into_iter()
            .next()
            .cloned()
            .ok_or_else(|| AraError::BadRegion(format!("no ARA region for clip `{clip_id}`")))?;
        let source = self
            .document
            .get_source(&entry.region.source.0)
            .ok_or_else(|| AraError::UnknownSource(entry.region.source.0.clone()))?;
        let len = entry.region.len_sec();
        let audio = source.read_range(
            entry.source_offset_sec,
            entry.source_offset_sec + len,
        );
        Ok(effect.render(&audio, source.sample_rate, entry.region.start_sec))
    }

    /// Drop every document-model object for `clip_id`. Idempotent:
    /// detaching a clip that was never shared is `Ok(())`, so teardown
    /// order never matters.
    pub fn detach_clip(&mut self, clip_id: &str) -> Result<()> {
        self.document.remove_clip(clip_id);
        Ok(())
    }

    /// Load a real third-party ARA binary. Still the documented follow-up:
    /// refuses with exactly what is missing — factory entry hosting (ARA
    /// factory handshake through the VST3/AU binary's plug-in extension)
    /// plus the document controller lifecycle (create/bind/destroy,
    /// analysis + playback-renderer roles, edit invalidation).
    pub fn load_binary_plugin(&self, _path: &str) -> Result<()> {
        Err(AraError::AraUnimplemented("load_binary_plugin"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine_440(sample_rate: u32, secs: f64) -> Vec<f32> {
        let n = (secs * sample_rate as f64).round() as usize;
        (0..n)
            .map(|i| {
                (2.0 * std::f64::consts::PI * 440.0 * i as f64 / sample_rate as f64).sin() as f32
            })
            .collect()
    }

    fn clip(id: &str, track: &str, start: f64, len: f64) -> Clip {
        Clip {
            id: id.to_string(),
            track_id: track.to_string(),
            name: id.to_string(),
            start_beats: start,
            length_beats: len,
            kind: crate::model::ClipKind::Audio,
            source: format!("take:{id}"),
        }
    }

    #[test]
    fn region_rejects_empty_and_inverted_ranges() {
        assert!(AraRegion::new("clip-a", 1.0, 1.0).is_none());
        assert!(AraRegion::new("clip-a", 2.0, 1.0).is_none());
        assert!(AraRegion::new("clip-a", f64::NAN, 2.0).is_none());
        let ok = AraRegion::new("clip-a", 1.0, 2.5).expect("valid range");
        assert_eq!(ok.len_sec(), 1.5);
    }

    #[test]
    fn musical_context_maps_beats_and_hears_key_errors() {
        let ctx = MusicalContext::new(120.0).expect("tempo");
        assert_eq!(ctx.beats_to_sec(4.0), 2.0);
        assert_eq!(ctx.sec_to_beats(2.0), 4.0);
        assert!(MusicalContext::new(0.0).is_none());
        assert!(MusicalContext::new(f64::NAN).is_none());
        assert!(MusicalContext::new(-120.0).is_none());
        let keyed = MusicalContext::new(100.0)
            .expect("tempo")
            .with_key(9, true)
            .expect("A minor");
        assert_eq!(keyed.key_tonic, Some(9));
        assert!(keyed.key_minor);
        assert!(MusicalContext::new(100.0).expect("tempo").with_key(12, false).is_none());
    }

    #[test]
    fn sources_offer_random_sample_access() {
        let mut doc = AraDocument::new(MusicalContext::default());
        // 1s ramp at 4Hz: sample k = k/4.
        let samples: Vec<f32> = (0..4).map(|k| k as f32).collect();
        doc.add_source("clip-a", 4, samples).expect("share");
        let src = doc.get_source("clip-a").expect("source");
        assert_eq!(src.len_frames(), 4);
        assert_eq!(src.duration_sec(), 1.0);
        assert_eq!(src.sample_at(0.0), 0.0);
        assert_eq!(src.sample_at(0.5), 2.0); // nearest: 0.5*4 = 2
        assert_eq!(src.sample_at(99.0), 3.0); // clamped to the tail
        assert_eq!(src.sample_at(-1.0), 0.0); // clamped to the head
        assert_eq!(src.read_range(0.0, 0.5), vec![0.0, 1.0]);
        assert!(src.read_range(1.0, 1.0).is_empty());
        assert!(src.read_range(2.0, 1.0).is_empty());
        // Whole-range reads past the tail clamp instead of failing.
        assert_eq!(src.read_range(0.5, 99.0), vec![2.0, 3.0]);
        assert!(doc.get_source("ghost").is_none());
        assert!(matches!(
            doc.add_source("", 4, vec![0.0]),
            Err(AraError::BadArgument(_))
        ));
        assert!(matches!(
            doc.add_source("x", 0, vec![0.0]),
            Err(AraError::BadArgument(_))
        ));
    }

    #[test]
    fn from_project_maps_clips_to_a_region_sequence() {
        let mut p = Project::new("p", "P");
        p.tempo = 120.0; // 0.5s per beat
        p.clips.push(clip("a", "trk", 0.0, 2.0));
        p.clips.push(clip("b", "trk", 4.0, 4.0));
        p.clips.push(clip("empty", "trk", 8.0, 0.0)); // skipped, not an error
        let doc = AraDocument::from_project(&p);
        assert_eq!(doc.context().tempo_bpm, 120.0);
        assert_eq!(doc.region_count(), 2);
        assert_eq!(doc.source_count(), 0); // mapping shares no audio yet
        let a = &doc.regions_for_clip("a")[0];
        assert_eq!((a.region.start_sec, a.region.end_sec), (0.0, 1.0));
        assert_eq!(a.region.source.0, "a");
        let b = &doc.regions_for_clip("b")[0];
        assert_eq!((b.region.start_sec, b.region.end_sec), (2.0, 4.0));
        assert!(doc.regions_for_clip("empty").is_empty());
        // Overlap query sees the timeline, not the clip list.
        assert_eq!(doc.regions_overlapping(0.5, 2.5).len(), 2);
        assert_eq!(doc.regions_overlapping(4.0, 9.0).len(), 0);
    }

    #[test]
    fn host_shares_renders_and_detaches_a_document() {
        let mut host = AraHost::new();
        assert!(host.is_available());
        // No region yet: rendering names the missing piece, not a crash.
        assert!(matches!(
            host.render_region("clip-a", &MockAraEffect::new(1.0, 0)),
            Err(AraError::BadRegion(_))
        ));
        // Region without audio: the source check fires.
        host.document_mut()
            .add_source("clip-a", 8000, vec![0.0; 8000])
            .expect("share");
        // (add_source registered the source; map a 1s region onto it)
        let region_id = host
            .document_mut()
            .add_region("clip-a", "clip-a", 0.0, 1.0, 0.0)
            .expect("region");
        assert!(region_id.starts_with("ara-region-"));
        let out = host
            .render_region("clip-a", &MockAraEffect::new(0.5, 0))
            .expect("render");
        assert_eq!(out.len(), 8000);
        // Detach is idempotent: twice is fine, and the region is gone.
        host.detach_clip("clip-a").expect("detach");
        host.detach_clip("clip-a").expect("detach again");
        assert!(matches!(
            host.render_region("clip-a", &MockAraEffect::new(1.0, 0)),
            Err(AraError::BadRegion(_))
        ));
    }

    #[test]
    fn real_binary_loading_names_its_missing_pieces() {
        let host = AraHost::new();
        let err = host.load_binary_plugin("/lib/Melodyne.vst3").expect_err("refuses");
        assert!(matches!(err, AraError::AraUnimplemented("load_binary_plugin")));
        let msg = err.to_string();
        assert!(msg.contains("factory entry hosting"), "{msg}");
        assert!(msg.contains("document controller lifecycle"), "{msg}");
    }

    #[test]
    fn mock_effect_edit_roundtrip_analyze_edit_render() {
        let sr = 8000u32;
        let audio = sine_440(sr, 1.0);
        // Analyze: 4 equal notes over the 1s region; 440Hz sine measures
        // as MIDI 69 in every note.
        let mut fx = MockAraEffect::analyze(&audio, sr, 0.0, 1.0, 4).expect("analyze");
        assert_eq!(fx.notes.len(), 4);
        assert!(fx.notes.iter().all(|n| n.pitch_midi == 69));
        assert_eq!(fx.notes[0].start_sec, 0.0);
        assert_eq!(fx.notes[2].start_sec, 0.5);
        // Unedited render is (near-)identity at unity gain.
        let clean = fx.render(&audio, sr, 0.0);
        assert_eq!(clean.len(), audio.len());
        for (a, b) in audio.iter().zip(clean.iter()) {
            assert!((a - b).abs() < 1e-5);
        }
        // Edit: mute the first two notes (the first half-second).
        assert!(fx.set_note_gain("note-0", 0.0));
        assert!(fx.set_note_gain("note-1", 0.0));
        assert!(!fx.set_note_gain("ghost", 0.0)); // stale edit: false, not error
        let edited = fx.render(&audio, sr, 0.0);
        assert_eq!(edited.len(), audio.len());
        assert!(edited[..4000].iter().all(|s| *s == 0.0));
        assert!(edited[4000..].iter().any(|s| s.abs() > 0.1));
        // Region gain scales the whole render.
        fx.region_gain = 0.5;
        let half = fx.render(&audio, sr, 0.0);
        for (a, b) in edited.iter().zip(half.iter()) {
            assert!((a * 0.5 - b).abs() < 1e-5);
        }
    }

    #[test]
    fn mock_effect_transpose_keeps_length_and_moves_pitch() {
        let sr = 8000u32;
        let audio = sine_440(sr, 1.0);
        let up = MockAraEffect::new(1.0, 12);
        let out = up.render(&audio, sr, 0.0);
        assert_eq!(out.len(), audio.len());
        // +12 reads twice as fast: frame i hears input 2i.
        assert!((out[100] - audio[200]).abs() < 1e-5);
        assert!(out[7999] == 0.0); // past-the-end pads with silence
        assert_ne!(out, audio);
        // Down a 12th still fills the window (truncated, not empty).
        let down = MockAraEffect::new(1.0, -12);
        let low = down.render(&audio, sr, 0.0);
        assert_eq!(low.len(), audio.len());
        assert!(low.iter().any(|s| s.abs() > 0.1));
        // Analyze refuses unusable geometry; silence analyzes to defaults.
        assert!(matches!(
            MockAraEffect::analyze(&audio, 0, 0.0, 1.0, 4),
            Err(AraError::BadArgument(_))
        ));
        assert!(matches!(
            MockAraEffect::analyze(&audio, sr, 0.0, 0.0, 4),
            Err(AraError::BadRegion(_))
        ));
        assert!(matches!(
            MockAraEffect::analyze(&audio, sr, 0.0, 1.0, 0),
            Err(AraError::BadArgument(_))
        ));
        let silent = MockAraEffect::analyze(&vec![0.0; 8000], sr, 0.0, 1.0, 2).expect("analyze");
        assert!(silent.notes.iter().all(|n| n.pitch_midi == 69));
    }
}
