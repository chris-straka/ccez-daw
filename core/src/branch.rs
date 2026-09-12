//! Git-like branches over the event-sourced op log.
//!
//! Teaching note: the project document is just "replay the op log", so a
//! branch is just "replay the shared prefix, then replay my extra ops".
//! That is the whole trick — no snapshots, no file copies. Compare is set
//! difference on op sequence numbers; merge is three-way diff against the
//! fork point with same-`target` edits surfacing as conflicts.
//!
//! This module is engine-side state only. It reuses the frozen `Op` /
//! `OpKind` types and adds no new IPC or project-schema surface, so the
//! typegen drift gate (`bun run check`) is unaffected.
//!
//! Undo history survives restart because the undo/redo stacks are fields of
//! [`BranchStore`], which serializes to JSON via [`BranchStore::save`] /
//! [`BranchStore::load`]. Every undo also appends an `UndoMarker` op to the
//! log, so redoable steps stay addressable after reload (per
//! `contracts/op-log-format.md`).

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::model::{Op, OpKind};

/// The branch every store is born with. Never deletable.
pub const MAIN_BRANCH: &str = "main";

/// One branch: the shared prefix it forked from plus its own extra ops.
///
/// `parent` names the branch it forked from and `base_seq` is the tip
/// sequence number of `parent` at fork time. The full history is resolved
/// by [`BranchStore::history`]: parent ops with `seq <= base_seq`, then
/// `ops` in order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Branch {
    pub name: String,
    pub parent: Option<String>,
    pub base_seq: u64,
    pub ops: Vec<Op>,
}

/// One entry of the cross-session undo history: which branch an applied op
/// landed on, plus the op itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UndoEntry {
    pub branch: String,
    pub op: Op,
}

/// Ops present on exactly one side of a comparison, at op granularity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchDiff {
    pub only_in_a: Vec<Op>,
    pub only_in_b: Vec<Op>,
}

/// One merge conflict: both sides edited the same `target` since the fork
/// point but to different effects (`kind` / `value_json` differ).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeConflict {
    pub target: String,
    pub source_op: Op,
    pub target_op: Op,
}

/// Preview (and, via [`BranchStore::merge_apply`], application) of a merge:
/// clean ops to append plus conflicts that need a human.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeOutcome {
    pub merged: Vec<Op>,
    pub conflicts: Vec<MergeConflict>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchError {
    NotFound(String),
    AlreadyExists(String),
    CannotDeleteMain,
    WouldCycle(String),
}

impl fmt::Display for BranchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BranchError::NotFound(name) => write!(f, "no such branch: {name}"),
            BranchError::AlreadyExists(name) => write!(f, "branch exists: {name}"),
            BranchError::CannotDeleteMain => write!(f, "cannot delete branch `main`"),
            BranchError::WouldCycle(name) => write!(f, "branch cycle via: {name}"),
        }
    }
}

impl std::error::Error for BranchError {}

/// Owns every branch, the global sequence counter, and the undo/redo stacks.
///
/// Sequence numbers are globally unique across branches (one counter), so
/// compare/merge can treat "same op" as "same `seq`" and everything else as
/// divergence. Serialize the whole store for restart-safe undo.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BranchStore {
    branches: HashMap<String, Branch>,
    next_seq: u64,
    undo_stack: Vec<UndoEntry>,
    redo_stack: Vec<UndoEntry>,
}

impl BranchStore {
    /// Empty store with a single `main` branch at `base_seq` 0.
    pub fn new() -> Self {
        let mut branches = HashMap::new();
        branches.insert(
            MAIN_BRANCH.to_string(),
            Branch {
                name: MAIN_BRANCH.to_string(),
                parent: None,
                base_seq: 0,
                ops: Vec::new(),
            },
        );
        Self {
            branches,
            next_seq: 1,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        }
    }

    pub fn branch_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.branches.keys().cloned().collect();
        names.sort();
        names
    }

    pub fn get(&self, name: &str) -> Result<&Branch, BranchError> {
        self.branches
            .get(name)
            .ok_or_else(|| BranchError::NotFound(name.to_string()))
    }

    /// Fork `name` from the current tip of `from`.
    pub fn create_branch(&mut self, name: &str, from: &str) -> Result<(), BranchError> {
        if self.branches.contains_key(name) {
            return Err(BranchError::AlreadyExists(name.to_string()));
        }
        let base_seq = self.tip_seq(from).ok_or_else(|| BranchError::NotFound(from.to_string()))?;
        self.branches.insert(
            name.to_string(),
            Branch {
                name: name.to_string(),
                parent: Some(from.to_string()),
                base_seq,
                ops: Vec::new(),
            },
        );
        Ok(())
    }

    pub fn delete_branch(&mut self, name: &str) -> Result<(), BranchError> {
        if name == MAIN_BRANCH {
            return Err(BranchError::CannotDeleteMain);
        }
        self.branches
            .remove(name)
            .ok_or_else(|| BranchError::NotFound(name.to_string()))?;
        Ok(())
    }

    /// Tip sequence number of a branch (0 when empty).
    pub fn tip_seq(&self, name: &str) -> Option<u64> {
        self.history(name).ok().and_then(|h| h.last().map(|op| op.seq))
    }

    /// Full op history of a branch: shared prefix, then its own ops.
    pub fn history(&self, name: &str) -> Result<Vec<Op>, BranchError> {
        let branch = self.get(name)?;
        let mut out = Vec::new();
        // Walk parents iteratively; depth is bounded by branch count.
        let mut chain: Vec<&Branch> = vec![branch];
        let mut seen: HashSet<&str> = HashSet::from([branch.name.as_str()]);
        while let Some(parent_name) = chain.last().and_then(|b| b.parent.as_deref()) {
            if !seen.insert(parent_name) {
                return Err(BranchError::WouldCycle(parent_name.to_string()));
            }
            let parent = self.get(parent_name)?;
            chain.push(parent);
            if chain.len() > self.branches.len() + 1 {
                return Err(BranchError::WouldCycle(parent_name.to_string()));
            }
        }
        // Each ancestor contributes the ops at or below the fork point of the
        // child that branched from it: `chain[i]` forked from `chain[i+1]`
        // at `chain[i].base_seq`.
        for (i, ancestor) in chain.iter().skip(1).enumerate() {
            let ceiling = chain[i].base_seq;
            for op in &ancestor.ops {
                if op.seq <= ceiling {
                    out.push(op.clone());
                }
            }
        }
        out.extend(branch.ops.iter().cloned());
        out.sort_by_key(|op| op.seq);
        out.dedup_by_key(|op| op.seq);
        Ok(out)
    }

    /// Append one op to `branch`, assigning the next global `seq`.
    /// Clears the redo stack (standard undo semantics).
    pub fn apply(
        &mut self,
        branch: &str,
        actor: &str,
        kind: OpKind,
        target: &str,
        value_json: &str,
    ) -> Result<Op, BranchError> {
        if !self.branches.contains_key(branch) {
            return Err(BranchError::NotFound(branch.to_string()));
        }
        let op = Op {
            seq: self.next_seq,
            actor: actor.to_string(),
            kind,
            target: target.to_string(),
            value_json: value_json.to_string(),
        };
        self.next_seq += 1;
        self.branches
            .get_mut(branch)
            .expect("checked above")
            .ops
            .push(op.clone());
        self.undo_stack.push(UndoEntry {
            branch: branch.to_string(),
            op: op.clone(),
        });
        self.redo_stack.clear();
        Ok(op)
    }

    /// A/B compare at op granularity: ops reachable from one side only.
    pub fn compare(&self, a: &str, b: &str) -> Result<BranchDiff, BranchError> {
        let ha = self.history(a)?;
        let hb = self.history(b)?;
        let seqs_b: HashSet<u64> = hb.iter().map(|op| op.seq).collect();
        let seqs_a: HashSet<u64> = ha.iter().map(|op| op.seq).collect();
        Ok(BranchDiff {
            only_in_a: ha.into_iter().filter(|op| !seqs_b.contains(&op.seq)).collect(),
            only_in_b: hb.into_iter().filter(|op| !seqs_a.contains(&op.seq)).collect(),
        })
    }

    /// Three-way merge preview of `source` into `target`.
    ///
    /// Base = ops common to both sides (shared prefix by `seq`); each new
    /// source op merges cleanly unless `target` added an op for the same
    /// `target` with a different effect. Identical same-target edits on both
    /// sides are idempotent and skipped, not conflicts.
    pub fn merge_preview(&self, source: &str, target: &str) -> Result<MergeOutcome, BranchError> {
        let diff = self.compare(source, target)?;
        // Target-side divergence = everything target has that source lacks.
        let target_new = diff.only_in_b;
        let mut by_target: HashMap<&str, &Op> = HashMap::new();
        for op in &target_new {
            by_target.insert(op.target.as_str(), op);
        }
        let mut outcome = MergeOutcome {
            merged: Vec::new(),
            conflicts: Vec::new(),
        };
        for op in diff.only_in_a {
            match by_target.get(op.target.as_str()) {
                None => outcome.merged.push(op),
                Some(top) if top.kind == op.kind && top.value_json == op.value_json => {
                    // Same edit on both sides: already in effect, skip.
                }
                Some(top) => outcome.conflicts.push(MergeConflict {
                    target: op.target.clone(),
                    source_op: op,
                    target_op: (*top).clone(),
                }),
            }
        }
        Ok(outcome)
    }

    /// Apply a merge: append the clean ops to `target` with fresh `seq`s so
    /// the log stays monotonic. Conflicting ops are left out for a human.
    /// Returns the preview (with re-sequenced `merged` ops).
    pub fn merge_apply(&mut self, source: &str, target: &str) -> Result<MergeOutcome, BranchError> {
        let preview = self.merge_preview(source, target)?;
        if !self.branches.contains_key(target) {
            return Err(BranchError::NotFound(target.to_string()));
        }
        let mut resequenced = Vec::with_capacity(preview.merged.len());
        for op in &preview.merged {
            let mut fresh = op.clone();
            fresh.seq = self.next_seq;
            self.next_seq += 1;
            self.branches
                .get_mut(target)
                .expect("checked above")
                .ops
                .push(fresh.clone());
            self.undo_stack.push(UndoEntry {
                branch: target.to_string(),
                op: fresh.clone(),
            });
            resequenced.push(fresh);
        }
        if !resequenced.is_empty() {
            self.redo_stack.clear();
        }
        Ok(MergeOutcome {
            merged: resequenced,
            conflicts: preview.conflicts,
        })
    }

    /// Undo the most recently applied op on any branch. Records an
    /// `UndoMarker` in the log pointing at the undone `seq`, and moves the
    /// entry to the redo stack. Returns the marker, or `None` when empty.
    pub fn undo(&mut self, actor: &str) -> Option<Op> {
        let entry = self.undo_stack.pop()?;
        let marker = Op {
            seq: self.next_seq,
            actor: actor.to_string(),
            kind: OpKind::UndoMarker,
            target: entry.op.seq.to_string(),
            value_json: serde_json::to_string(&entry.branch)
                .unwrap_or_else(|_| format!("\"{}\"", entry.branch)),
        };
        self.next_seq += 1;
        if let Some(branch) = self.branches.get_mut(&entry.branch) {
            branch.ops.push(marker.clone());
        }
        self.redo_stack.push(entry);
        Some(marker)
    }

    /// Redo the most recently undone op with a fresh `seq`. Returns the
    /// re-applied op, or `None` when the redo stack is empty.
    pub fn redo(&mut self) -> Option<Op> {
        let entry = self.redo_stack.pop()?;
        let mut op = entry.op.clone();
        op.seq = self.next_seq;
        self.next_seq += 1;
        if let Some(branch) = self.branches.get_mut(&entry.branch) {
            branch.ops.push(op.clone());
            self.undo_stack.push(UndoEntry {
                branch: entry.branch,
                op: op.clone(),
            });
            Some(op)
        } else {
            None
        }
    }

    pub fn undo_depth(&self) -> usize {
        self.undo_stack.len()
    }

    pub fn redo_depth(&self) -> usize {
        self.redo_stack.len()
    }

    /// Persist the whole store (branches, counter, undo/redo) to `path`.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(path, json)
    }

    /// Reload a store saved with [`BranchStore::save`]. Undo history comes
    /// along, so undo/redo keep working across restarts.
    pub fn load(path: &Path) -> std::io::Result<Self> {
        let json = std::fs::read_to_string(path)?;
        serde_json::from_str(&json)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::OpKind;

    fn two_track_setup() -> BranchStore {
        let mut store = BranchStore::new();
        store
            .apply("main", "ui", OpKind::TrackAdded, "trk_a", "\"A\"")
            .unwrap();
        store
            .apply("main", "ui", OpKind::TrackAdded, "trk_b", "\"B\"")
            .unwrap();
        store.create_branch("mix", "main").unwrap();
        store
            .apply("mix", "ui", OpKind::ParamSet, "trk_a:volume", "0.5")
            .unwrap();
        store
            .apply("main", "ui", OpKind::TempoSet, "transport", "128")
            .unwrap();
        store
    }

    #[test]
    fn branch_compare_lists_each_sides_unique_ops() {
        let store = two_track_setup();
        let diff = store.compare("mix", "main").unwrap();
        assert_eq!(diff.only_in_a.len(), 1);
        assert_eq!(diff.only_in_a[0].target, "trk_a:volume");
        assert_eq!(diff.only_in_b.len(), 1);
        assert_eq!(diff.only_in_b[0].kind, OpKind::TempoSet);
    }

    #[test]
    fn branch_merge_applies_clean_ops_with_fresh_seqs() {
        let mut store = two_track_setup();
        let outcome = store.merge_apply("mix", "main").unwrap();
        assert!(outcome.conflicts.is_empty());
        assert_eq!(outcome.merged.len(), 1);
        // Re-sequenced past the previous tip so the log stays monotonic.
        assert!(outcome.merged[0].seq > 4);
        let history = store.history("main").unwrap();
        assert!(history.iter().any(|op| op.target == "trk_a:volume"));
    }

    #[test]
    fn branch_merge_surfaces_same_target_conflict() {
        let mut store = BranchStore::new();
        store
            .apply("main", "ui", OpKind::TrackAdded, "trk_a", "\"A\"")
            .unwrap();
        store.create_branch("alt", "main").unwrap();
        store
            .apply("main", "ui", OpKind::ParamSet, "trk_a:volume", "0.9")
            .unwrap();
        store
            .apply("alt", "ui", OpKind::ParamSet, "trk_a:volume", "0.2")
            .unwrap();
        let outcome = store.merge_preview("alt", "main").unwrap();
        assert!(outcome.merged.is_empty());
        assert_eq!(outcome.conflicts.len(), 1);
        assert_eq!(outcome.conflicts[0].target, "trk_a:volume");
        // merge_apply must not smuggle the conflicting op into the target.
        let applied = store.merge_apply("alt", "main").unwrap();
        assert_eq!(applied.conflicts.len(), 1);
        let history = store.history("main").unwrap();
        assert_eq!(
            history
                .iter()
                .filter(|op| op.target == "trk_a:volume")
                .count(),
            1
        );
    }

    #[test]
    fn identical_edits_on_both_sides_are_not_conflicts() {
        let mut store = BranchStore::new();
        store
            .apply("main", "ui", OpKind::TrackAdded, "trk_a", "\"A\"")
            .unwrap();
        store.create_branch("alt", "main").unwrap();
        for branch in ["main", "alt"] {
            store
                .apply(branch, "ui", OpKind::ParamSet, "trk_a:volume", "0.5")
                .unwrap();
        }
        let outcome = store.merge_preview("alt", "main").unwrap();
        assert!(outcome.conflicts.is_empty());
        assert!(outcome.merged.is_empty());
    }

    #[test]
    fn undo_history_survives_restart() {
        let mut store = BranchStore::new();
        store
            .apply("main", "ui", OpKind::TrackAdded, "trk_a", "\"A\"")
            .unwrap();
        store
            .apply("main", "ui", OpKind::TempoSet, "transport", "128")
            .unwrap();
        store.undo("ui").unwrap();
        assert_eq!(store.undo_depth(), 1);
        assert_eq!(store.redo_depth(), 1);

        let path = std::env::temp_dir().join(format!(
            "ccez-branch-test-{}.json",
            std::process::id()
        ));
        store.save(&path).unwrap();
        let mut reloaded = BranchStore::load(&path).unwrap();
        let _ = std::fs::remove_file(&path);

        // Stacks round-tripped: undo the remaining op, redo both.
        assert_eq!(reloaded.undo_depth(), 1);
        assert_eq!(reloaded.redo_depth(), 1);
        let marker = reloaded.undo("ui").unwrap();
        assert_eq!(marker.kind, OpKind::UndoMarker);
        assert_eq!(reloaded.redo_depth(), 2);
        assert!(reloaded.redo().is_some());
        assert!(reloaded.redo().is_some());
        assert!(reloaded.redo().is_none());
    }
}
