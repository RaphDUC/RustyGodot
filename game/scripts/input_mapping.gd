extends Resource
class_name InputMapping

# Dictionary: "Godot Action Name" -> "Bit Index (0-7)".
@export var action_map: Dictionary = {
	"ui_up": 0,
	"ui_down": 1,
	"ui_left": 2,
	"ui_right": 3,
	"ui_accept": 4
}
