#!/usr/bin/env python3
"""Template self-test: validates the fixture package against the loader spec
(docs/notes/ga-export-loader.md) without needing a Godot binary.

Checks: manifest shapes, bank bodies, WAV headers + loop-length math,
cue-map <-> manifest consistency, layers_for_state for all six states,
transition lookup, and RTPC math. Mirrors scripts/*.gd logic in Python.

Usage: python3 tools/self_test.py [package-dir]
Exit 0 = all checks pass, 1 = any failure.
"""
import json
import math
import struct
import sys
from pathlib import Path

FAILURES = []


def check(name, cond, detail=""):
    print(("PASS " if cond else "FAIL ") + name + (f" ({detail})" if detail and not cond else ""))
    if not cond:
        FAILURES.append(name)


def beats_to_samples(beats, tempo, rate):
    seconds = beats * 60.0 / tempo
    if beats == math.floor(beats):
        return int(math.floor(seconds * rate))
    return int(round(seconds * rate))


def read_wav_frames(path):
    with open(path, "rb") as f:
        data = f.read()
    assert data[0:4] == b"RIFF" and data[8:12] == b"WAVE", "not a WAV"
    pos, fmt, frames = 12, None, None
    while pos + 8 <= len(data):
        tag, size = struct.unpack("<4sI", data[pos : pos + 8])
        body = data[pos + 8 : pos + 8 + size]
        if tag == b"fmt ":
            fmt = struct.unpack("<HHIIHH", body[:16])
        if tag == b"data":
            frames = len(body) // (fmt[0] * fmt[5] // 8)
        pos += 8 + size + (size % 2)
    return fmt, frames


def main():
    pkg = Path(sys.argv[1] if len(sys.argv) > 1 else Path(__file__).parent.parent / "demo" / "overworld-v1")
    manifest = json.loads((pkg / "package.json").read_text())
    check("manifest schema_version == 1", manifest.get("schema_version") == 1)
    check("manifest validator_version == '1'", manifest.get("validator_version") == "1")
    check("manifest names cue_overworld", manifest.get("cue_ids") == ["cue_overworld"])
    check("manifest names bank_adventure", manifest.get("bank_ids") == ["bank_adventure"])

    bank = json.loads((pkg / "bank_bank_adventure.json").read_text())
    check("bank id matches filename", bank.get("id") == "bank_adventure")
    events = {e["id"]: e for e in bank.get("events", [])}
    check(
        "bank event ids unique",
        len(events) == len(bank.get("events", [])),
        "duplicate event id",
    )
    check(
        "all events have non-empty pools",
        all(e.get("clip_ids") for e in events.values()),
    )
    check(
        "expected events present",
        {"player.footstep", "sword.swing", "ui.click", "horse.gallop"} <= set(events),
        str(sorted(events)),
    )
    check(
        "footstep pool has 2 clips",
        len(events["player.footstep"]["clip_ids"]) == 2,
    )

    cue_map = json.loads((pkg.parent / "cue_overworld.json").read_text())
    tempo = cue_map["tempo"]
    check("cue tempo positive", tempo == 100.0, str(tempo))

    manifest_layers = {
        s["source_layer_id"]
        for s in manifest["stems"]
        if s["kind"] == "MusicLayer" and s["source_id"] == "cue_overworld"
    }
    mapped_layers = {layer["id"] for layer in cue_map["layers"]}
    check("cue map covers every manifest layer", manifest_layers == mapped_layers,
          f"manifest={sorted(manifest_layers)} map={sorted(mapped_layers)}")

    music_stems = [s for s in manifest["stems"] if s["kind"] == "MusicLayer"]
    sfx_stems = [s for s in manifest["stems"] if s["kind"] == "SfxClip"]
    check("6 music stems", len(music_stems) == 6, str(len(music_stems)))
    check("6 sfx stems", len(sfx_stems) == 6, str(len(sfx_stems)))
    for s in music_stems:
        ok_range = 0.0 <= s["loop_start_beats"] < s["loop_end_beats"]
        check(f"loop range {s['path']}", ok_range, str((s["loop_start_beats"], s["loop_end_beats"])))
        fmt, frames = read_wav_frames(pkg / s["path"])
        check(f"wav 48k mono16 {s['path']}", fmt[2] == 48000 and fmt[0] == 1 and fmt[5] == 16, str(fmt))
        want = beats_to_samples(s["loop_end_beats"] - s["loop_start_beats"], tempo, 48000)
        check(f"loop-length math {s['path']}", frames == want, f"frames={frames} want={want}")
    for s in sfx_stems:
        check(f"sfx one-shot {s['path']}", s["loop_start_beats"] == 0.0 and s["loop_end_beats"] == 0.0)
        check(f"sfx stem file exists {s['path']}", (pkg / s["path"]).exists())
        fmt, frames = read_wav_frames(pkg / s["path"])
        check(f"sfx 1.0s length {s['path']}", frames == 48000, str(frames))

    def layers_for_state(state):
        return [
            layer["id"]
            for layer in cue_map["layers"]
            if not layer["states"] or state in layer["states"]
        ]

    expected = {
        "field": ["bed"],
        "combat": ["bed", "drums"],
        "dungeon": ["bed", "drone"],
        "boss": ["bed", "drums", "brass"],
        "village": ["bed", "pad"],
        "night": ["bed", "pad", "bell"],
    }
    for state, want in expected.items():
        check(f"layers_for_state({state})", layers_for_state(state) == want,
              str(layers_for_state(state)))

    rules = {(r["from"], r["to"]): r for r in cue_map["transitions"]}
    check("field->combat is Fade 2 beats",
          rules[("field", "combat")]["kind"] == "Fade" and rules[("field", "combat")]["fade_beats"] == 2.0)
    check("no-rule pair means Cut", ("combat", "boss") not in rules)
    fade_sec = rules[("field", "dungeon")]["fade_beats"] * 60.0 / tempo
    check("field->dungeon fade = 2.4s", abs(fade_sec - 2.4) < 1e-9, str(fade_sec))

    def rtpc(param_min, param_max, value, out_min, out_max):
        clamped = min(max(value, param_min), param_max)
        norm = (clamped - param_min) / (param_max - param_min)
        return out_min + norm * (out_max - out_min)

    footstep_rtpc = events["player.footstep"]["rtpc"][0]
    check("footstep threat->0.5 gain 0.75",
          abs(rtpc(0, 1, 0.5, footstep_rtpc["min"], footstep_rtpc["max"]) - 0.75) < 1e-9)
    gallop_rtpc = events["horse.gallop"]["rtpc"][0]
    check("gallop mounted=1 gain 1.0",
          abs(rtpc(0, 1, 1.0, gallop_rtpc["min"], gallop_rtpc["max"]) - 1.0) < 1e-9)

    print(f"\n{len(FAILURES)} failure(s)" if FAILURES else "\nall checks passed")
    return 1 if FAILURES else 0


if __name__ == "__main__":
    sys.exit(main())
