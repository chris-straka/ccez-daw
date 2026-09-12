//! Track J: browser library truth + deterministic similarity search.
//!
//! One palette indexes four browsable kinds — samples, presets, plugins,
//! projects — over a single [`LibraryItem`] shape. Ranking is cosine
//! similarity over small hashed bag-of-words embeddings ([`embed_text`]).
//!
//! The embedding is intentionally dependency-free and deterministic (FNV-1a
//! token hashing, L2-normalized) so Rust core and the TS mirror in
//! `ui/src/browser/library.ts` agree exactly. Real neural embeddings arrive
//! later behind the sidecar interface (`ui/src/browser/embeddings.ts`);
//! the model is deliberately unpinned here — this module is the fallback
//! truth and the test oracle, not the final ranker.
//!
//! These types are additive to the frozen v0 contracts: they extend the
//! generated `project.ts` without touching existing shapes or the IPC table.

use serde::{Deserialize, Serialize};

/// Embedding width. Small on purpose: keyword-concept search, not ML.
pub const EMBED_DIM: usize = 64;

/// The four browsable library kinds behind the one palette.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LibraryKind {
    Sample,
    Preset,
    Plugin,
    Project,
}

/// One browsable entry: a sample, preset, plugin, or project.
///
/// `text` is the searchable description; ranking reads
/// `name + tags + text` (see [`item_text`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryItem {
    pub id: String,
    pub kind: LibraryKind,
    pub name: String,
    pub tags: Vec<String>,
    pub text: String,
}

/// One ranked search hit: the item id plus cosine score in `[-1, 1]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LibraryHit {
    pub id: String,
    pub score: f32,
}

fn item(id: &str, kind: LibraryKind, name: &str, tags: &[&str], text: &str) -> LibraryItem {
    LibraryItem {
        id: id.to_string(),
        kind,
        name: name.to_string(),
        tags: tags.iter().map(|t| t.to_string()).collect(),
        text: text.to_string(),
    }
}

/// Seeded demo library: two entries per kind. The concept-query test below
/// (mirrored in `ui/tests/browser.test.ts`) pins `preset_warm_pad` as the
/// top hit for a "warm ambient pad" concept query. Keep the two files in
/// sync when editing seeds.
pub fn seed_library() -> Vec<LibraryItem> {
    vec![
        item(
            "preset_warm_pad",
            LibraryKind::Preset,
            "Warm Analog Pad",
            &["warm", "analog", "pad", "ambient", "synth", "lush"],
            "Lush warm analog-style pad with slow attack for ambient intros and beds",
        ),
        item(
            "preset_punchy_bass",
            LibraryKind::Preset,
            "Punchy Synth Bass",
            &["bass", "synth", "punchy", "mono", "reese"],
            "Punchy mono synth bass with fast filter envelope for drive sections",
        ),
        item(
            "sample_boomy_kick",
            LibraryKind::Sample,
            "Boomy 808 Kick",
            &["kick", "808", "boom", "drums", "sub"],
            "Deep boomy 808 kick drum sample with long sub decay",
        ),
        item(
            "sample_crispy_snare",
            LibraryKind::Sample,
            "Crispy Snare",
            &["snare", "drums", "crisp", "backbeat", "clap"],
            "Crispy acoustic snare with bright transient for backbeats",
        ),
        item(
            "plugin_vintage_verb",
            LibraryKind::Plugin,
            "Vintage Hall Reverb",
            &["reverb", "hall", "space", "vintage", "send"],
            "Vintage hall reverb plugin for sends, long lush tails",
        ),
        item(
            "plugin_tape_delay",
            LibraryKind::Plugin,
            "Tape Echo Delay",
            &["delay", "echo", "tape", "dotted", "send"],
            "Tape-style echo delay plugin with wow flutter and dotted repeats",
        ),
        item(
            "project_midnight_demo",
            LibraryKind::Project,
            "Midnight Drive Demo",
            &["demo", "synthwave", "night", "driving", "template"],
            "Synthwave demo project with driving bass and neon pads template",
        ),
        item(
            "project_lofi_sketch",
            LibraryKind::Project,
            "Lofi Sketch",
            &["lofi", "hiphop", "chill", "keys", "template"],
            "Chill lofi hiphop sketch project with dusty keys and soft drums template",
        ),
    ]
}

/// Lowercase alphanumeric tokenization. Non-`[a-z0-9]` bytes are separators.
/// Mirrored exactly by `tokenize` in `ui/src/browser/library.ts`.
pub fn tokenize(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
        .collect()
}

/// FNV-1a 32-bit hash. Mirrored exactly by `fnv1a32` in TS (`Math.imul`).
pub fn fnv1a32(bytes: &[u8]) -> u32 {
    let mut hash: u32 = 2_166_136_261;
    for b in bytes {
        hash ^= u32::from(*b);
        hash = hash.wrapping_mul(16_777_619);
    }
    hash
}

/// Deterministic hashed bag-of-words embedding, L2-normalized.
/// Zero vector (empty input) stays zero.
pub fn embed_text(text: &str) -> Vec<f32> {
    let mut v = vec![0.0f32; EMBED_DIM];
    for token in tokenize(text) {
        let idx = (fnv1a32(token.as_bytes()) % EMBED_DIM as u32) as usize;
        v[idx] += 1.0;
    }
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
    v
}

/// Searchable document text for one item.
pub fn item_text(item: &LibraryItem) -> String {
    let mut parts = vec![item.name.clone()];
    parts.extend(item.tags.iter().cloned());
    parts.push(item.text.clone());
    parts.join(" ")
}

/// Cosine similarity. Inputs are already normalized, so this is the dot
/// product; returns 0 when either side is a zero vector.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    if dot.is_nan() { 0.0 } else { dot }
}

/// Rank `items` by concept similarity to `query` (kind-filtered when
/// `kind` is `Some`). Ties break by ascending id for determinism.
/// Empty queries score 0 everywhere and return id order.
pub fn search_filtered(
    query: &str,
    items: &[LibraryItem],
    kind: Option<&LibraryKind>,
    top_k: usize,
) -> Vec<LibraryHit> {
    let q = embed_text(query);
    let mut hits: Vec<LibraryHit> = items
        .iter()
        .filter(|it| kind.is_none_or(|k| &it.kind == k))
        .map(|it| LibraryHit {
            id: it.id.clone(),
            score: cosine(&q, &embed_text(&item_text(it))),
        })
        .collect();
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.id.cmp(&b.id))
    });
    hits.truncate(top_k);
    hits
}

/// Rank all kinds (see [`search_filtered`]).
pub fn search(query: &str, items: &[LibraryItem], top_k: usize) -> Vec<LibraryHit> {
    search_filtered(query, items, None, top_k)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedding_is_deterministic_and_normalized() {
        let a = embed_text("Warm Analog Pad");
        let b = embed_text("Warm Analog Pad");
        assert_eq!(a.len(), EMBED_DIM);
        assert_eq!(a, b);
        let norm: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-6, "norm was {norm}");
        assert_eq!(embed_text(""), vec![0.0; EMBED_DIM]);
    }

    #[test]
    fn concept_query_returns_seeded_pad_first() {
        let lib = seed_library();
        let hits = search("cozy warm analog pad for an ambient intro", &lib, 3);
        assert!(!hits.is_empty());
        assert_eq!(hits[0].id, "preset_warm_pad", "hits: {hits:?}");
    }

    #[test]
    fn search_respects_kind_filter() {
        let lib = seed_library();
        let hits = search_filtered("kick drum", &lib, Some(&LibraryKind::Sample), 8);
        assert!(!hits.is_empty());
        assert_eq!(hits[0].id, "sample_boomy_kick", "hits: {hits:?}");
        for h in &hits {
            let it = lib.iter().find(|i| i.id == h.id).expect("seed id");
            assert_eq!(it.kind, LibraryKind::Sample);
        }
    }

    #[test]
    fn seed_items_have_unique_ids_and_cover_all_kinds() {
        let lib = seed_library();
        let mut ids: Vec<&str> = lib.iter().map(|i| i.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), lib.len());
        for kind in [
            LibraryKind::Sample,
            LibraryKind::Preset,
            LibraryKind::Plugin,
            LibraryKind::Project,
        ] {
            assert!(lib.iter().any(|i| i.kind == kind), "missing {kind:?}");
        }
    }

    #[test]
    fn library_items_round_trip_through_json() {
        let lib = seed_library();
        let json = serde_json::to_string(&lib).expect("serialize");
        let back: Vec<LibraryItem> = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(lib, back);
    }
}
