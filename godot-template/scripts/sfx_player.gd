class_name CcezSfxPlayer
extends Node

## Triggers named SFX events from a v1 bank: uniform pool pick, per-trigger
## volume/pitch humanization from a seeded RNG, cooldown drops, polyphony
## steal-oldest, and RTPC-style param bindings.
##
## Playback-time failures stay silent-by-design (GA-1 engine rule): unknown
## event ids are dropped with a debug warning, never an error. Missing
## values read as the param default; unknown params are ignored per trigger.

const UNKNOWN_EVENT := "UnknownEvent"
const DROPPED_COOLDOWN := "DroppedCooldown"

var _bank: Dictionary = {}
var _params: Dictionary = {} # param_id -> {min, max, default}
var _streams: Dictionary = {} # "event_id#path" -> AudioStreamWAV
var _voices: Dictionary = {} # event_id -> Array[AudioStreamPlayer] (oldest first)
var _last_ms: Dictionary = {} # event_id -> last accepted trigger msec
var _rng := RandomNumberGenerator.new()


## bank: one SfxBank body. params: [{id, min, max, default}, ...].
## seed: export seed for deterministic replays (DAW default 2026).
func setup(package: Dictionary, bank_id: String, params: Array, seed: int = 2026) -> String:
	_rng.seed = seed
	for p in params:
		_params[str(p.get("id", ""))] = {
			"min": float(p.get("min", 0.0)),
			"max": float(p.get("max", 1.0)),
			"default": float(p.get("default", 0.0)),
		}
	var banks: Dictionary = package["banks"]
	if not banks.has(bank_id):
		return "package holds no bank '%s'" % bank_id
	_bank = banks[bank_id]
	var base: String = str(package["package_dir"])
	var manifest: Dictionary = package["manifest"]
	for stem in manifest["stems"]:
		if str(stem.get("kind", "")) != "SfxClip":
			continue
		var key: String = "%s#%s" % [str(stem["source_id"]), str(stem["path"])]
		var stream: AudioStreamWAV = load(base + str(stem["path"])) as AudioStreamWAV
		if stream == null:
			return "cannot load audio stream: %s" % str(stem["path"])
		stream.loop_mode = AudioStreamWAV.LOOP_DISABLED
		_streams[key] = stream
	return ""


func _event(event_id: String) -> Dictionary:
	for event in (_bank as Dictionary).get("events", []):
		if str(event.get("id", "")) == event_id:
			return event
	return {}


## RTPC: clamp the game value into the param range, normalize to 0-1,
## map linearly onto the binding output range. Unknown params ignored.
func rtpc_output(binding: Dictionary, values: Dictionary) -> float:
	var pid: String = str(binding.get("param", ""))
	if not _params.has(pid):
		return -1.0 # unknown param: ignored per trigger
	var decl: Dictionary = _params[pid]
	var lo: float = decl["min"]
	var hi: float = decl["max"]
	var v: float = float(values.get(pid, decl["default"]))
	var clamped: float = clampf(v, minf(lo, hi), maxf(lo, hi))
	var norm: float = 0.0
	if hi != lo:
		norm = (clamped - lo) / (hi - lo)
	return float(binding.get("min", 0.0)) + norm * (float(binding.get("max", 1.0)) - float(binding.get("min", 0.0)))


## Triggers `event_id` once. Returns "Played", "UnknownEvent", or
## "DroppedCooldown". values: {param_id: float} for RTPC bindings.
func trigger(event_id: String, values: Dictionary = {}) -> String:
	var event: Dictionary = _event(event_id)
	if event.is_empty():
		push_warning("CcezSfxPlayer: unknown event '%s' dropped" % event_id)
		return UNKNOWN_EVENT
	var now_ms: int = int(Time.get_ticks_msec())
	var cooldown: int = int(event.get("cooldown_ms", 0))
	if _last_ms.has(event_id) and now_ms - int(_last_ms[event_id]) < cooldown:
		return DROPPED_COOLDOWN
	_last_ms[event_id] = now_ms

	var pool: Array = event.get("clip_ids", [])
	var pick: int = _rng.randi_range(0, pool.size() - 1)
	var gain: float = float(event.get("volume", 0.8))
	gain *= 1.0 + _rng.randf_range(-1.0, 1.0) * float(event.get("volume_random", 0.0))
	var pitch_st: float = _rng.randf_range(-1.0, 1.0) * float(event.get("pitch_random", 0.0))
	# RTPC bindings bend the trigger: volume targets scale gain,
	# pitch targets add semitones. Other targets are ignored (v1 scope).
	for binding in event.get("rtpc", []):
		var out: float = rtpc_output(binding, values)
		if out < 0.0:
			continue
		match str(binding.get("target_param", "")):
			"volume":
				gain *= out
			"pitch":
				pitch_st += out
	var stream: AudioStreamWAV = _stream_for(event_id, pick)
	if stream == null:
		push_warning("CcezSfxPlayer: no stream for '%s' pick %d" % [event_id, pick])
		return UNKNOWN_EVENT
	_play(event, stream, clampf(gain, 0.0, 2.0), pitch_st)
	return "Played"


## Streams are keyed by event id + manifest path suffix _<n>.wav,
## where <n> is the index into the event's clip_ids pool.
func _stream_for(event_id: String, pick: int) -> AudioStreamWAV:
	for key in _streams:
		var ks: String = str(key)
		if ks.begins_with(event_id + "#") and ks.ends_with("_%d.wav" % pick):
			return _streams[key] as AudioStreamWAV
	return null


func _play(event: Dictionary, stream: AudioStreamWAV, gain: float, pitch_st: float) -> void:
	var eid: String = str(event.get("id", ""))
	if not _voices.has(eid):
		_voices[eid] = []
	var live: Array = _voices[eid]
	# Reuse a finished voice before stealing the oldest.
	for voice in live:
		if not (voice as AudioStreamPlayer).playing:
			_start_voice(voice, stream, gain, pitch_st)
			return
	var max_poly: int = maxi(int(event.get("max_polyphony", 4)), 1)
	if live.size() >= max_poly:
		var oldest: AudioStreamPlayer = live.pop_front()
		_start_voice(oldest, stream, gain, pitch_st)
		live.push_back(oldest)
		return
	var voice := AudioStreamPlayer.new()
	voice.name = "Voice_%s_%d" % [eid, live.size()]
	add_child(voice)
	_start_voice(voice, stream, gain, pitch_st)
	live.push_back(voice)


func _start_voice(voice: AudioStreamPlayer, stream: AudioStreamWAV, gain: float, pitch_st: float) -> void:
	voice.stream = stream
	voice.volume_db = linear_to_db(maxf(gain, 0.0001))
	voice.pitch_scale = pow(2.0, pitch_st / 12.0)
	voice.play()
