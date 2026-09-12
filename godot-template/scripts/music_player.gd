class_name CcezMusicPlayer
extends Node

## One AudioStreamPlayer per music stem; state changes crossfade layer
## volumes instead of restarting streams, so loops never click or restart.
##
## Selection mirrors AdaptiveCue.layers_for_state: a layer is audible when
## its `states` list is empty (always-on bed) or contains the active state.
## Transition lookup mirrors AdaptiveCue.transition_for: the first rule
## matching from->to wins; no rule means a Cut. Cut/Fade are honored;
## BarWait/Stinger fall back to a Fade and log once (v1 template scope).

var _cue_id: String = ""
var _tempo: float = 120.0
var _layers: Array = []
var _transitions: Array = []
var _players: Dictionary = {} # layer_id -> AudioStreamPlayer
var _state: String = ""
var _tweens: Dictionary = {} # layer_id -> Tween
var _warned_kinds: Dictionary = {}


## Builds one looping voice per MusicLayer stem of `cue_id`.
## cue_map: {"tempo": float, "layers": [...], "transitions": [...]}.
func setup(package: Dictionary, cue_id: String, cue_map: Dictionary) -> String:
	_cue_id = cue_id
	_tempo = float(cue_map.get("tempo", 120.0))
	_layers = cue_map.get("layers", [])
	_transitions = cue_map.get("transitions", [])
	var base: String = str(package["package_dir"])
	for stem in (package["manifest"] as Dictionary)["stems"]:
		if str(stem.get("source_id", "")) != cue_id:
			continue
		if str(stem.get("kind", "")) != "MusicLayer":
			continue
		var lid: String = str(stem["source_layer_id"])
		var stream: AudioStreamWAV = load(base + str(stem["path"])) as AudioStreamWAV
		if stream == null:
			return "cannot load audio stream: %s" % str(stem["path"])
		# Whole-file loops: the validator guarantees the file holds exactly
		# loop_end - loop_start beats, so FORWARD over the full range is
		# seamless. Beats -> samples at the CUE tempo, never project tempo.
		var mix_rate: int = int(stream.mix_rate)
		stream.loop_mode = AudioStreamWAV.LOOP_FORWARD
		stream.loop_begin = CcezBankLoader.beats_to_samples(float(stem["loop_start_beats"]), _tempo, mix_rate)
		stream.loop_end = CcezBankLoader.beats_to_samples(float(stem["loop_end_beats"]), _tempo, mix_rate)
		var player := AudioStreamPlayer.new()
		player.name = "Layer_" + lid
		player.stream = stream
		player.volume_db = -80.0
		add_child(player)
		_players[lid] = player
	if _players.is_empty():
		return "cue '%s' has no music stems in the package" % cue_id
	_state = str(cue_map.get("default_state", "field"))
	return ""


func current_state() -> String:
	return _state


## Layers audible in `state` (bed layers included). Mirrors layers_for_state.
func layers_for_state(state: String) -> Array:
	var out: Array = []
	for layer in _layers:
		var states: Array = layer.get("states", [])
		if states.is_empty() or states.has(state):
			out.append(str(layer["id"]))
	return out


## First rule matching from->to, or {} when none matches (Cut).
func transition_for(from_state: String, to_state: String) -> Dictionary:
	for rule in _transitions:
		if str(rule.get("from", "")) == from_state and str(rule.get("to", "")) == to_state:
			return rule
	return {}


func _layer_volume(lid: String) -> float:
	for layer in _layers:
		if str(layer.get("id", "")) == lid:
			return float(layer.get("volume", 0.8))
	return 0.8


## Switches the active state. Starts voices on first call; afterwards only
## volumes move, so no loop ever restarts mid-bar.
func set_state(state: String) -> void:
	var first_start: bool = _state == "" or _all_silent()
	var from_state: String = _state
	_state = state
	var audible: Array = layers_for_state(state)
	if first_start:
		for lid in _players:
			var player: AudioStreamPlayer = _players[lid]
			if not player.playing:
				player.play()
		_snap_volumes(audible)
		return
	var rule: Dictionary = transition_for(from_state, state)
	var kind: String = str(rule.get("kind", "Cut"))
	var fade_beats: float = float(rule.get("fade_beats", 0.0))
	if kind != "Cut" and kind != "Fade":
		if not _warned_kinds.has(kind):
			_warned_kinds[kind] = true
			push_warning("CcezMusicPlayer: transition kind '%s' falls back to Fade (v1 scope)" % kind)
		kind = "Fade"
	if kind == "Cut":
		_snap_volumes(audible)
		return
	var fade_sec: float = maxf(fade_beats * 60.0 / _tempo, 0.05)
	for lid in _players:
		var player: AudioStreamPlayer = _players[lid]
		if not player.playing:
			player.play()
		var target_db: float = -80.0
		if audible.has(lid):
			target_db = linear_to_db(maxf(_layer_volume(lid), 0.0001))
		_fade_to(lid, player, target_db, fade_sec)


func _all_silent() -> bool:
	for lid in _players:
		if (_players[lid] as AudioStreamPlayer).playing:
			return false
	return true


func _snap_volumes(audible: Array) -> void:
	for lid in _players:
		var player: AudioStreamPlayer = _players[lid]
		if not player.playing:
			player.play()
		_kill_tween(lid)
		if audible.has(lid):
			player.volume_db = linear_to_db(maxf(_layer_volume(lid), 0.0001))
		else:
			player.volume_db = -80.0


func _fade_to(lid: String, player: AudioStreamPlayer, target_db: float, fade_sec: float) -> void:
	_kill_tween(lid)
	var tween: Tween = create_tween().set_parallel(false)
	tween.tween_property(player, "volume_db", target_db, fade_sec)
	_tweens[lid] = tween


func _kill_tween(lid: String) -> void:
	if _tweens.has(lid) and is_instance_valid(_tweens[lid]):
		(_tweens[lid] as Tween).kill()
	_tweens.erase(lid)
