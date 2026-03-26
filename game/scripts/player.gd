extends CharacterBody2D

# Reference to the Debug Label.
@onready var label = $Label

var network_id: int = 0
var is_local: bool = false
var network_manager: Node = null

# Called by Rust (NetworkManager) right after spawn.
func setup(_network_id: int, _is_local: bool):
	network_id = _network_id
	is_local = _is_local

	if is_inside_tree():
		_update_ui()

func _ready():
	# Retrieve NetworkManager for sending inputs.
	if has_node("/root/Game/NetworkManager"):
		network_manager = get_node("/root/Game/NetworkManager")
	
	_update_ui()

func _update_ui():
	if label:
		label.text = "ID: %d" % network_id

		if is_local:
			label.text += " (YOU)"
			# Local player: Blueish tint.
			modulate = Color(0.8, 0.8, 1.0)

			# Add camera for local player.
			if not has_node("Camera2D"):
				var cam = Camera2D.new()
				add_child(cam)
				cam.make_current()
		else:
			# Remote players: Reddish tint and semi-transparent.
			modulate = Color(1.0, 0.8, 0.8, 0.5)

	# Control Management: Disable local physics for remote players.
	set_physics_process(is_local)

# Movement speed (Must match server speed: 200.0).
const SPEED = 200.0

# --- MODULAR INPUT (Lab 2 Refactor) ---
@export var input_map: Resource = preload("res://resources/input_config.tres")

var _has_warned = false

func _physics_process(_delta):
	# 1. Collect Modular Inputs.
	var input_mask: int = 0

	# Hardcoded fallback if the resource is not loaded correctly.
	var map_to_use = {
		"ui_up": 0, "ui_down": 1, "ui_left": 2, "ui_right": 3, "ui_accept": 4
	}

	# Attempt to use resource.
	if input_map and "action_map" in input_map and not input_map.action_map.is_empty():
		map_to_use = input_map.action_map
	else:
		if is_local and not _has_warned:
			print("[Player] WARNING: Using fallback input map (Resource missing or empty)")
			_has_warned = true

	for action in map_to_use:
		if Input.is_action_pressed(action):
			# Optional: Uncomment to confirm key detection.
			# print("[Player] Detected: ", action) 
			var bit = map_to_use[action]
			input_mask |= (1 << bit)

	# 2. Send to NetworkManager (Parent).
	if network_manager and network_manager.has_method("send_input"):
		network_manager.send_input(input_mask)

	# 3. Local Prediction (Visual only).
	var direction = Vector2.ZERO
	# Decode mask locally for prediction (assumes standard mapping for visuals).
	if (input_mask & (1 << 0)) != 0: direction.y -= 1 # Up
	if (input_mask & (1 << 1)) != 0: direction.y += 1 # Down
	if (input_mask & (1 << 2)) != 0: direction.x -= 1 # Left
	if (input_mask & (1 << 3)) != 0: direction.x += 1 # Right

	if direction:
		velocity = direction.normalized() * SPEED
	else:
		velocity = Vector2.ZERO

	# Apply movement and handle local collisions.
	move_and_slide()

	# ARCHITECTURE NOTE:
	# In this Lab (Authoritative Server), this local movement is a form of
	# naive "Client-Side Prediction".
	# 1. We move immediately for responsiveness (this code).
	# 2. In parallel, NetworkManager (Rust) sends these inputs to the server.
	# 3. The server simulates and returns the "true" position (StateUpdate).
	# 4. NetworkManager (Rust) receives position and does a hard `set_position`.
	#
	# Result: Smooth if server/client agree. 'Rubber banding' if they disagree/lag.
	#
	# This structure SOLVES the "Control Issue":
	# - Only the local player runs this code (thanks to set_physics_process(is_local)).
	# - Remote players are just puppets moved by the C++ code.

# Helper for bitwise check.
func is_bit_set(mask: int, index: int) -> bool:
	return (mask & (1 << index)) != 0
