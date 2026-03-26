extends NetworkManager

# The NetworkManager (Rust) already handles:
# 1. Socket initialization (bind_port(0)) in its native _ready.
# 2. Receive loop (poll()) in its native _process.
# 3. Automatic entity instantiation (Lab 1 Spawn).

func _ready():
	# Connect to the signal to display text messages (Debug/Echo).
	# The "packet_received" signal is defined in lib.rs.
	packet_received.connect(_on_packet_received_rust)

	print("NetworkManager (Rust) ready. Sending test...")

	# Test: Manual send of a String packet.
	var msg = "Hello from GDScript via Rust!"
	# send_packet is exposed by the Rust class.
	send_packet("127.0.0.1", 4242, msg.to_utf8_buffer())

# Callback for Rust signal.
func _on_packet_received_rust(sender_ip: String, sender_port: int, data: PackedByteArray):
	var message = data.get_string_from_utf8()
	print("[GDScript] Packet received from ", sender_ip, ":", sender_port, " | Content: ", message)

# NOTE: No need for _process(delta) here to call poll().
# The Rust class already does it in its internal implementation of _process.
