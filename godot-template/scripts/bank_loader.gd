class_name CcezBankLoader
extends RefCounted

## Reads a CCEZ GA-4 export package straight off disk.
##
## Package layout (frozen v1, see docs/notes/ga-export-loader.md):
##   package.json, bank_<id>.json, stems/<cue>_<layer>.wav, sfx/<event>_<n>.wav
##
## Fail-loud-at-load rule: every problem below is a load error returned in
## the result Dictionary. A package that passed `export-validator` already
## satisfies all of these; the loader re-checks because packages travel
## through pipelines that corrupt things.
##
## NOTE on cue tempo and layer maps: the v1 manifest does NOT ship the
## AdaptiveCue (tempo, per-layer state lists, transitions) — that stays
## authoring-side data. The title supplies it as a cue-map Dictionary
## (see demo/cue_overworld.json), and `load_package` cross-checks the map
## against the manifest so a stale map is a load error, not a silent bug.

## Beats at the cue tempo -> audio samples. Mirrors the loader spec section 2.
## Uses floor() for the loop start, matching the DAW-side validator math.
static func beats_to_samples(beats: float, tempo: float, sample_rate: int) -> int:
	if tempo <= 0.0:
		return 0
	var seconds: float = beats * 60.0 / tempo
	if beats == floor(beats):
		return int(floor(seconds * float(sample_rate)))
	return int(round(seconds * float(sample_rate)))


## Reads one JSON file. Returns {"ok": true, "data": ...} or {"ok": false, "error": ...}.
static func _read_json(path: String) -> Dictionary:
	if not FileAccess.file_exists(path):
		return {"ok": false, "error": "missing file: %s" % path}
	var text: String = FileAccess.get_file_as_string(path)
	var parsed: Variant = JSON.parse_string(text)
	if parsed == null:
		return {"ok": false, "error": "malformed JSON: %s" % path}
	return {"ok": true, "data": parsed}


## Validates the manifest Dictionary. Returns "" when valid, else an error.
static func _check_manifest(manifest: Dictionary) -> String:
	if not manifest.has("schema_version") or int(manifest["schema_version"]) != 1:
		return "unsupported schema_version (v1 loader accepts only 1)"
	if not manifest.has("validator_version") or str(manifest["validator_version"]) != "1":
		return "unsupported validator_version (v1 loader accepts only \"1\")"
	if manifest.get("cue_ids", []).is_empty():
		return "manifest has no cue_ids"
	if manifest.get("bank_ids", []).is_empty():
		return "manifest has no bank_ids"
	if manifest.get("stems", []).is_empty():
		return "manifest has no stems"
	return ""


## Validates one bank body against its filename id. Returns "" when valid.
static func _check_bank(bank: Dictionary, bank_id: String) -> String:
	if str(bank.get("id", "")) != bank_id:
		return "bank id mismatch: file bank_%s.json holds id '%s'" % [bank_id, str(bank.get("id", ""))]
	if int(bank.get("schema_version", 0)) != 1:
		return "bank '%s' has unsupported schema_version" % bank_id
	var seen: Dictionary = {}
	for event in bank.get("events", []):
		var eid: String = str(event.get("id", ""))
		if eid == "":
			return "bank '%s' holds an event with no id" % bank_id
		if seen.has(eid):
			return "bank '%s' has duplicate event id '%s'" % [bank_id, eid]
		seen[eid] = true
		if event.get("clip_ids", []).is_empty():
			return "event '%s' has an empty clip pool" % eid
	return ""


## Validates one stem entry (loop ranges only; sample counts are checked
## by the engine when the WAV decodes). Returns "" when valid.
static func _check_stem(stem: Dictionary) -> String:
	var start: float = float(stem.get("loop_start_beats", 0.0))
	var end: float = float(stem.get("loop_end_beats", 0.0))
	match str(stem.get("kind", "")):
		"MusicLayer":
			if not (0.0 <= start and start < end):
				return "music stem '%s' has bad loop %s..%s (need 0 <= start < end)" % [str(stem.get("path", "")), str(start), str(end)]
			if str(stem.get("source_layer_id", "")) == "":
				return "music stem '%s' names no source layer" % str(stem.get("path", ""))
		"SfxClip":
			if start != 0.0 or end != 0.0:
				return "sfx stem '%s' must be 0, 0 one-shot, got %s..%s" % [str(stem.get("path", "")), str(start), str(end)]
		_:
			return "stem '%s' has unknown kind '%s'" % [str(stem.get("path", "")), str(stem.get("kind", ""))]
	return ""


## Cross-checks the title-supplied cue map against the manifest stems.
## cue_map shape: {"tempo": float, "layers": [{"id": str, "states": [str], "volume": float}], ...}.
static func _check_cue_map(cue_map: Dictionary, cue_id: String, stems: Array) -> String:
	if float(cue_map.get("tempo", 0.0)) <= 0.0:
		return "cue map for '%s' names no positive tempo" % cue_id
	var mapped: Dictionary = {}
	for layer in cue_map.get("layers", []):
		mapped[str(layer.get("id", ""))] = true
	for stem in stems:
		if str(stem.get("source_id", "")) != cue_id:
			continue
		if str(stem.get("kind", "")) != "MusicLayer":
			continue
		var lid: String = str(stem.get("source_layer_id", ""))
		if not mapped.has(lid):
			return "cue map for '%s' is missing layer '%s' (stale map?)" % [cue_id, lid]
	return ""


## Loads and validates a package.
##   package_dir: e.g. "res://demo/overworld-v1" (trailing slash optional)
##   cue_maps: {cue_id: cue_map Dictionary} supplied by the title.
## Returns {"ok": true, "manifest": ..., "banks": {id: bank}, "package_dir": ...}
## or {"ok": false, "error": ...}. Never half-loads: error means use nothing.
static func load_package(package_dir: String, cue_maps: Dictionary = {}) -> Dictionary:
	var base: String = package_dir.rstrip("/") + "/"
	var m: Dictionary = _read_json(base + "package.json")
	if not m["ok"]:
		return {"ok": false, "error": m["error"]}
	var manifest: Dictionary = m["data"]
	var err: String = _check_manifest(manifest)
	if err != "":
		return {"ok": false, "error": err}

	var banks: Dictionary = {}
	for bank_id in manifest["bank_ids"]:
		var bid: String = str(bank_id)
		var b: Dictionary = _read_json(base + "bank_" + bid + ".json")
		if not b["ok"]:
			return {"ok": false, "error": b["error"]}
		err = _check_bank(b["data"], bid)
		if err != "":
			return {"ok": false, "error": err}
		banks[bid] = b["data"]

	for stem in manifest["stems"]:
		err = _check_stem(stem)
		if err != "":
			return {"ok": false, "error": err}
		if not FileAccess.file_exists(base + str(stem["path"])):
			return {"ok": false, "error": "stem file missing: %s" % str(stem["path"])}

	for cue_id in manifest["cue_ids"]:
		var cid: String = str(cue_id)
		if not cue_maps.has(cid):
			return {"ok": false, "error": "no cue map supplied for '%s' (title must provide tempo + layer states)" % cid}
		err = _check_cue_map(cue_maps[cid], cid, manifest["stems"])
		if err != "":
			return {"ok": false, "error": err}

	return {"ok": true, "manifest": manifest, "banks": banks, "package_dir": base}
