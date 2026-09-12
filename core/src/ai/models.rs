//! Track P-2: AI sidecar model pins — a decision per sidecar, not a download.
//!
//! v1 left the neural models unpinned (open question); v3 closes it. Each
//! sidecar in [`super`] gets an explicit verdict in [`PINS`]: either a named
//! local model (id, approximate size, license, canonical download URL) or
//! `baselines-only` with a recorded reason. Rationale, sizes, and license
//! evidence live in `docs/notes/r-models.md`.
//!
//! Runtime rule: the DAW runs fully with models absent. [`resolve`] maps a
//! sidecar id to [`ModelResolution::Baseline`] unless the weight file is
//! already cached (see [`model_cache_dir`]); only then does the caller use
//! the existing one-interface HTTP seam (`HttpTranscriptionSidecar` in
//! `mcp/src/ai/sidecar.ts`, same `submit_*` / `apply_*` shapes in Rust).
//! This module performs no network I/O and adds no dependency — the
//! first-use download itself lives in the sidecar process / TS layer
//! (`ensureModelCached` in `mcp/src/ai/models.ts`), which writes the same
//! cache path probed by [`cached_weight_path`].
//!
//! Plans may change; op shapes and UI don't.

use std::path::PathBuf;

/// What v3 decided for one sidecar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelVerdict {
    /// A named local model, fetched on first use, served via the HTTP seam.
    Pinned {
        /// Unpinned-at-runtime model id (plain string on the seam).
        model_id: &'static str,
        /// Approximate weight size, for the download prompt + disk budget.
        size_mb: &'static str,
        /// Redistribution license (verified before bundling anything).
        license: &'static str,
        /// Canonical download source for the weights.
        url: &'static str,
        /// Weight filename inside [`model_cache_dir`].
        file: &'static str,
    },
    /// No model: the deterministic baseline is the shipped behavior; the
    /// HTTP seam stays available for a future pick.
    BaselinesOnly { reason: &'static str },
}

/// One sidecar's pin: stable sidecar id (the `ai:<id>` actor suffix) plus
/// its verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelPin {
    pub sidecar: &'static str,
    pub verdict: ModelVerdict,
}

/// The v3 verdict table. Six sidecars, six verdicts — "unpinned" is no
/// longer an option after v3.
pub const PINS: &[ModelPin] = &[
    ModelPin {
        sidecar: super::SIDECAR_DRUMS,
        verdict: ModelVerdict::Pinned {
            model_id: "oaf-drums",
            size_mb: "~40",
            license: "Apache-2.0 (Magenta; re-verify checkpoint header at wire-up)",
            url: "https://github.com/magenta/onsets-and-frames",
            file: "oaf-drums.ckpt",
        },
    },
    ModelPin {
        sidecar: super::SIDECAR_MELODY,
        verdict: ModelVerdict::Pinned {
            model_id: "basic-pitch",
            size_mb: "~15",
            license: "Apache-2.0 (Spotify Audio Intelligence Lab)",
            url: "https://github.com/spotify/basic-pitch",
            file: "basic-pitch.onnx",
        },
    },
    ModelPin {
        sidecar: super::SIDECAR_CHORDS,
        verdict: ModelVerdict::Pinned {
            model_id: "basic-pitch",
            size_mb: "~15",
            license: "Apache-2.0 (Spotify Audio Intelligence Lab)",
            url: "https://github.com/spotify/basic-pitch",
            file: "basic-pitch.onnx",
        },
    },
    ModelPin {
        sidecar: super::SIDECAR_SEPARATION,
        verdict: ModelVerdict::Pinned {
            model_id: "htdemucs",
            size_mb: "~80-350 by variant/precision (4-stem)",
            license: "MIT (Meta AI Research, code + weights)",
            url: "https://github.com/facebookresearch/demucs",
            file: "htdemucs.th",
        },
    },
    ModelPin {
        sidecar: super::SIDECAR_CLEANUP,
        verdict: ModelVerdict::Pinned {
            model_id: "deepfilternet3",
            size_mb: "~4-30",
            license: "MIT/Apache-2.0 dual (Rikorose/DeepFilterNet; re-verify LICENSE at wire-up)",
            url: "https://github.com/Rikorose/DeepFilterNet",
            file: "deepfilternet3.onnx",
        },
    },
    ModelPin {
        sidecar: super::SIDECAR_GROOVE,
        verdict: ModelVerdict::BaselinesOnly {
            reason: "groove transfer is an analytic MIDI timing/velocity transform \
                (extract + apply template); no neural model beats it at this task \
                for its size. HTTP seam stays open for a future style model.",
        },
    },
];

/// Look up the pin for a sidecar id.
pub fn pin_for(sidecar: &str) -> Option<&'static ModelPin> {
    PINS.iter().find(|p| p.sidecar == sidecar)
}

/// Weight cache directory: `$CCEZ_MODEL_DIR`, else the OS data dir's
/// `ccez-daw/models` (else the system temp dir — always a real directory,
/// never the project tree, never bundled with the app).
pub fn model_cache_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("CCEZ_MODEL_DIR") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.trim().is_empty() {
            let p = PathBuf::from(home)
                .join(".local")
                .join("share")
                .join("ccez-daw")
                .join("models");
            return p;
        }
    }
    std::env::temp_dir().join("ccez-daw-models")
}

/// Cache path for a pinned model's weights.
pub fn cached_weight_path(model_id: &str, file: &str) -> PathBuf {
    let _ = model_id;
    model_cache_dir().join(file)
}

/// How a sidecar should run right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelResolution {
    /// No weights cached (or no model pinned): run the deterministic
    /// baseline. The DAW is fully functional in this state.
    Baseline { sidecar: String },
    /// Weights are cached: serve `model_id` through the HTTP seam. The
    /// endpoint serves the cached weights; plans change, op shapes don't.
    Sidecar {
        sidecar: String,
        model_id: String,
        endpoint: String,
    },
}

/// Default local sidecar endpoint (loopback; the sidecar process serves the
/// cached weights behind the existing `POST /transcribe` shape).
pub const DEFAULT_SIDECAR_ENDPOINT: &str = "http://127.0.0.1:8765";

/// Resolve a sidecar id to how it should run. Models-absent is the
/// default: unknown ids, baselines-only verdicts, and missing weight files
/// all resolve to [`ModelResolution::Baseline`].
pub fn resolve(sidecar: &str) -> ModelResolution {
    let base = || ModelResolution::Baseline {
        sidecar: sidecar.to_string(),
    };
    let pin = match pin_for(sidecar) {
        Some(p) => p,
        None => return base(),
    };
    let pinned = match pin.verdict {
        ModelVerdict::Pinned {
            model_id, file, ..
        } => (model_id, file),
        ModelVerdict::BaselinesOnly { .. } => return base(),
    };
    let weight = cached_weight_path(pinned.0, pinned.1);
    if weight.is_file() {
        ModelResolution::Sidecar {
            sidecar: sidecar.to_string(),
            model_id: pinned.0.to_string(),
            endpoint: DEFAULT_SIDECAR_ENDPOINT.to_string(),
        }
    } else {
        base()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// `CCEZ_MODEL_DIR` is process-global: tests touching it run serially.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn every_sidecar_has_a_verdict() {
        let ids = [
            super::super::SIDECAR_DRUMS,
            super::super::SIDECAR_MELODY,
            super::super::SIDECAR_CHORDS,
            super::super::SIDECAR_SEPARATION,
            super::super::SIDECAR_CLEANUP,
            super::super::SIDECAR_GROOVE,
        ];
        assert_eq!(PINS.len(), ids.len());
        for id in ids {
            assert!(pin_for(id).is_some(), "sidecar {id} has no pin");
        }
    }

    #[test]
    fn groove_is_baselines_only() {
        let pin = pin_for(super::super::SIDECAR_GROOVE).expect("groove pin");
        assert!(matches!(
            pin.verdict,
            ModelVerdict::BaselinesOnly { .. }
        ));
    }

    #[test]
    fn models_absent_resolves_to_baseline() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        // Point the cache at an empty temp dir: no weights can be cached.
        let dir = std::env::temp_dir().join(format!("ccez-models-absent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("tmp cache dir");
        std::env::set_var("CCEZ_MODEL_DIR", &dir);
        for pin in PINS {
            let r = resolve(pin.sidecar);
            assert_eq!(
                r,
                ModelResolution::Baseline {
                    sidecar: pin.sidecar.to_string()
                },
                "sidecar {} must run baseline with models absent",
                pin.sidecar
            );
        }
        // Unknown ids also fall back to baseline, never panic.
        assert!(matches!(
            resolve("no-such-sidecar"),
            ModelResolution::Baseline { .. }
        ));
        std::env::remove_var("CCEZ_MODEL_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cached_weights_resolve_to_sidecar() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let dir = std::env::temp_dir().join(format!("ccez-models-present-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("tmp cache dir");
        std::env::set_var("CCEZ_MODEL_DIR", &dir);
        // Fake-cache the drums weights: presence is one file, nothing more.
        std::fs::write(dir.join("oaf-drums.ckpt"), b"fake weights").expect("fake weights");
        let r = resolve(super::super::SIDECAR_DRUMS);
        assert_eq!(
            r,
            ModelResolution::Sidecar {
                sidecar: super::super::SIDECAR_DRUMS.to_string(),
                model_id: "oaf-drums".to_string(),
                endpoint: DEFAULT_SIDECAR_ENDPOINT.to_string(),
            }
        );
        // Everything else still runs baseline.
        assert!(matches!(
            resolve(super::super::SIDECAR_MELODY),
            ModelResolution::Baseline { .. }
        ));
        std::env::remove_var("CCEZ_MODEL_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
