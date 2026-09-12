extends Node

## Demo driver: loads the overworld-v1 fixture package, builds the test UI
## (six state buttons, four param sliders, four SFX buttons), and posts
## GameStateSnapshots ({state, values}) to the music + SFX players.
##
## Replace the package dir + cue map with your own export; the UI stays the
## same until your game wires set_state()/trigger() into its own state.

const PACKAGE_DIR := "res://demo/overworld-v1"
const CUE_ID := "cue_overworld"
const BANK_ID := "bank_adventure"

const STATES := ["field", "combat", "dungeon", "boss", "village", "night"]
const PARAMS := [
	{"id": "threat", "min": 0.0, "max": 1.0, "default": 0.0},
	{"id": "time_of_day", "min": 0.0, "max": 24.0, "default": 12.0},
	{"id": "health_low", "min": 0.0, "max": 1.0, "default": 0.0},
	{"id": "mounted", "min": 0.0, "max": 1.0, "default": 0.0},
]
const EVENTS := ["player.footstep", "sword.swing", "ui.click", "horse.gallop"]

var _music: CcezMusicPlayer
var _sfx: CcezSfxPlayer
var _values: Dictionary = {}
var _status: Label


func _ready() -> void:
	for p in PARAMS:
		_values[str(p["id"])] = float(p["default"])
	var cue_map: Dictionary = JSON.parse_string(FileAccess.get_file_as_string("res://demo/cue_overworld.json"))
	if cue_map == null:
		_fail("cannot parse res://demo/cue_overworld.json")
		return
	var cue_maps := {CUE_ID: cue_map}
	var package: Dictionary = CcezBankLoader.load_package(PACKAGE_DIR, cue_maps)
	if not package["ok"]:
		_fail("package load failed: %s" % str(package["error"]))
		return

	_music = CcezMusicPlayer.new()
	add_child(_music)
	var err: String = _music.setup(package, CUE_ID, cue_map)
	if err != "":
		_fail("music setup failed: %s" % err)
		return

	_sfx = CcezSfxPlayer.new()
	add_child(_sfx)
	err = _sfx.setup(package, BANK_ID, cue_map.get("params", PARAMS))
	if err != "":
		_fail("sfx setup failed: %s" % err)
		return

	_build_ui()
	_music.set_state("field")
	_report()


func _fail(message: String) -> void:
	push_error(message)
	var label := Label.new()
	label.text = message
	add_child(label)


func _build_ui() -> void:
	var root := VBoxContainer.new()
	root.set_anchors_preset(Control.PRESET_FULL_RECT)
	root.add_theme_constant_override("separation", 8)
	add_child(root)

	var title := Label.new()
	title.text = "Overworld audio demo — GA-4 package: overworld-v1"
	root.add_child(title)

	var states := HBoxContainer.new()
	states.add_theme_constant_override("separation", 4)
	root.add_child(states)
	for s in STATES:
		var b := Button.new()
		b.text = s
		b.pressed.connect(_on_state.bind(s))
		states.add_child(b)

	for p in PARAMS:
		var row := HBoxContainer.new()
		root.add_child(row)
		var name := Label.new()
		name.custom_minimum_size = Vector2(110, 0)
		name.text = str(p["id"])
		row.add_child(name)
		var slider := HSlider.new()
		slider.custom_minimum_size = Vector2(260, 0)
		slider.min_value = float(p["min"])
		slider.max_value = float(p["max"])
		slider.step = (float(p["max"]) - float(p["min"])) / 100.0
		slider.value = float(p["default"])
		slider.value_changed.connect(_on_param.bind(str(p["id"])))
		row.add_child(slider)
		var val := Label.new()
		val.name = "Val_" + str(p["id"])
		val.text = str(float(p["default"]))
		row.add_child(val)

	var sfx_row := HBoxContainer.new()
	sfx_row.add_theme_constant_override("separation", 4)
	root.add_child(sfx_row)
	for e in EVENTS:
		var b := Button.new()
		b.text = e
		b.pressed.connect(_on_sfx.bind(e))
		sfx_row.add_child(b)

	_status = Label.new()
	_status.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	root.add_child(_status)


## Posts a GameStateSnapshot: the named state plus all continuous values.
func _snapshot() -> Dictionary:
	return {"state": _music.current_state(), "values": _values.duplicate()}


func _on_state(state: String) -> void:
	_music.set_state(state)
	_report()


func _on_param(value: float, param_id: String) -> void:
	_values[param_id] = value
	var label: Label = find_child("Val_" + param_id, true, false)
	if label != null:
		label.text = "%.2f" % value
	_report()


func _on_sfx(event_id: String) -> void:
	var result: String = _sfx.trigger(event_id, _values)
	var snap: Dictionary = _snapshot()
	_status.text = "trigger '%s' -> %s\nsnapshot: state=%s values=%s\n%s" % [
		event_id, result, str(snap["state"]), str(snap["values"]), _audible_line()]


func _audible_line() -> String:
	return "state '%s' audible layers: %s" % [_music.current_state(), str(_music.layers_for_state(_music.current_state()))]


func _report() -> void:
	if _status != null:
		_status.text = _audible_line()
