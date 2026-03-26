use godot::classes::{INode, Node, Node2D, PackedScene, Time};
use godot::prelude::*;
use serde::{Deserialize, Serialize};
use snl::GameSocket;
use std::collections::{HashMap, VecDeque};

struct MyExtension;

#[gdextension]
unsafe impl ExtensionLibrary for MyExtension {}

/// --- PROTOCOL DEFINITION ---
/// Must match the server's struct layout exactly (Binary Protocol).
#[repr(C, packed)]
#[derive(Debug, Clone, Copy, Serialize, Deserialize)] // Added Serialize, Deserialize
struct SpawnPacket {
    pub packet_type: u8,
    pub network_id: u32,
    pub type_id: u32,
    pub x: f32,
    pub y: f32,
}

#[repr(C, packed)]
struct WelcomePacket {
    packet_type: u8,
    network_id: u32,
}

#[repr(C, packed)]
struct StatePacket {
    packet_type: u8,
    network_id: u32,
    x: f32,
    y: f32,
}

/// Bitbox (1 byte) for input compression
#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default)]
#[repr(transparent)]
struct InputState(pub u8);

impl InputState {
    pub fn new(bits: u8) -> Self {
        Self(bits)
    }
}

/// Packet sent by Client to Server containing input history.
#[derive(Serialize, Deserialize, Debug, Clone)]
struct InputPacket {
    pub packet_type: u8, // = 2
    pub sequence: u32,
    /// Fixed size array for zero-allocation history (20 frames)
    pub inputs: [InputState; 20], 
    pub count: u8, 
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
struct PingRequest {
    pub packet_type: u8, // = 3
    pub id: u32,
    pub t0: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
struct PingResponse {
    pub packet_type: u8, // = 4
    pub id: u32,
    pub t0: u64,
    pub t1: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
struct DisconnectPacket {
    pub packet_type: u8, // = 6
    pub network_id: u32,
}

// --- LINKING CONTEXT ---
// Type alias for the factory function (Lambda) that creates Godot Nodes.
// This allows us to map a generic TypeID (integer) to a specific Scene instantiation logic.
type CreationLambda = Box<dyn Fn() -> Gd<Node2D>>;

#[derive(Clone)]
struct DelayedPacket {
    deliver_time: f64,
    sender_addr: String,
    data: Vec<u8>,
}

/// The Main Manager for Network Replication in Godot.
///
/// It handles:
/// 1. Socket initialization and polling.
/// 2. Packet deserialization.
/// 3. Instantiating entities via the Linking Context (Registry).
#[derive(GodotClass)]
#[class(base=Node)]
struct NetworkManager {
    /// UDP Socket wrapper (from SNL library).
    socket: Option<GameSocket>,
    /// Map of active network entities (NetworkID -> Godot Node).
    network_objects: HashMap<u32, Gd<Node2D>>,
    /// Registry mapping TypeIDs to Creation Functions (The "Linking Context").
    registry: HashMap<u32, CreationLambda>,

    // --- Lab 2: Inputs & Latency ---
    sequence_id: u32,
    input_history: VecDeque<InputState>,
    ping_timer: f64,

    // Local Client ID
    local_network_id: u32,

    // --- Prediction Tuning ---
    #[export]
    correction_threshold: f32, // Distance threshold before forcing a correction

    #[export]
    correction_smoothing: f32, // Lerp factor for smooth correction (0.0 to 1.0)

    #[export]
    simulated_latency_ms: f64, // Artificial lag in milliseconds for testing

    incoming_queue: Vec<DelayedPacket>,

    base: Base<Node>,
}

#[godot_api]
impl NetworkManager {
    // Porting signals from C++:
    // signal packet_received(sender_ip: String, sender_port: int, data: PackedByteArray)
    #[signal]
    fn packet_received(sender_ip: GString, sender_port: i32, data: PackedByteArray);

    /// Porting bind_port from C++
    #[func]
    fn bind_port(&mut self, port: i32) -> bool {
        self.socket = None; // Close existing

        // snl::GameSocket expects "IP:PORT"
        let address = if port == 0 {
            "0.0.0.0:0".to_string()
        } else {
            format!("0.0.0.0:{}", port)
        };

        match GameSocket::new(&address) {
            Ok(s) => {
                self.socket = Some(s);
                godot_print!("UDP socket bound to {}", address);
                true
            }
            Err(e) => {
                godot_print!("[ERROR] Failed to bind socket: {:?}", e);
                false
            }
        }
    }

    /// Porting send_packet from C++
    #[func]
    fn send_packet(&mut self, ip: GString, port: i32, data: PackedByteArray) {
        if let Some(socket) = &self.socket {
            let target = format!("{}:{}", ip, port);
            let bytes = data.to_vec(); // Convert Godot packed array to Rust Vec<u8>
            if let Err(e) = socket.send(&target, &bytes) {
                godot_print!("[ERROR] Failed to send packet: {:?}", e);
            }
        } else {
            godot_print!("[ERROR] Cannot send packet: Socket not bound.");
        }
    }

    #[func]
    fn register_node(&mut self, node: Gd<Node>) {
        // Try to cast to Node2D since our map stores Gd<Node2D>
        if let Ok(node2d) = node.clone().try_cast::<Node2D>() {
            // Check if the node has a "network_id" property exposed to Godot
            let id_var = node2d.get("network_id");
            if !id_var.is_nil() {
                // Be careful with type conversion, assume it fits in u32
                let id = id_var.to::<u32>();
                if id > 0 {
                    self.network_objects.insert(id, node2d);
                    godot_print!("[Client] Node manually registered with ID: {}", id);
                } else {
                    godot_print!("[WARN] register_node: 'network_id' is 0 or invalid.");
                }
            } else {
                godot_print!("[WARN] register_node: Node missing 'network_id' property.");
            }
        } else {
            godot_print!("[WARN] register_node: Node is not a Node2D.");
        }
    }

    #[func]
    fn serialize_snapshot(&self) -> PackedByteArray {
        let mut buffer = Vec::new();
        // Simple serialization: Count (u32) + List of { ID (u32), X (f32), Y (f32) }

        let count = self.network_objects.len() as u32;
        buffer.extend_from_slice(&count.to_le_bytes());

        for (id, node) in &self.network_objects {
            // Note: Assuming node is valid. If a node is free in Godot but not removed from map, this might panic.
            // In a robust system, we would check instance validity.
            // if !node.is_instance_valid() { continue; }

            let pos = node.get_position();
            buffer.extend_from_slice(&id.to_le_bytes());
            buffer.extend_from_slice(&pos.x.to_le_bytes());
            buffer.extend_from_slice(&pos.y.to_le_bytes());
        }

        PackedByteArray::from(buffer.as_slice())
    }

    /// Function called by Player GDScript to send inputs.
    /// Mapping (Action -> Bits) was done by the Godot script.
    #[func]
    fn send_input(&mut self, input_mask: u32) { // Changed to u32 to be safe with GDScript types.
        if self.socket.is_none() { return; }

        let mask_u8 = input_mask as u8;

        // Debug to verify what GDScript is sending.
        if mask_u8 != 0 && self.sequence_id % 60 == 0 {
             godot_print!("[Client] Sending Input Mask: {:#010b} (Seq: {})", mask_u8, self.sequence_id);
        }

        // 1. UPDATE HISTORY FIRST
        let new_state = InputState(mask_u8);
        self.input_history.push_front(new_state);
        if self.input_history.len() > 20 {
            self.input_history.pop_back();
        }

        // 2. CONSTRUCT PACKET
        let mut inputs_array = [InputState::default(); 20];
        let count = self.input_history.len().min(20);

        for (i, state) in self.input_history.iter().take(20).enumerate() {
            inputs_array[i] = *state;
        }

        let packet = InputPacket {
            packet_type: 2, // Input Packet
            sequence: self.sequence_id,
            inputs: inputs_array,
            count: count as u8,
        };

        // RELIABLE SOLUTION: Use bincode like the server.
        let encoded = bincode::serialize(&packet).unwrap_or_default();

        if let Some(socket) = &self.socket {
            let _ = socket.send("127.0.0.1:4242", &encoded);
        }

        self.sequence_id += 1;
    }

}

#[godot_api]
impl INode for NetworkManager {
    fn init(base: Base<Node>) -> Self {
        // 1. Initialize Socket immediately (construction time)
        let socket = match GameSocket::new("0.0.0.0:0") {
            Ok(s) => {
                godot_print!("UDP socket bound automatically in init (Port 0).");
                Some(s)
            }
            Err(e) => {
                godot_print!("[ERROR] Failed to auto-bind socket: {:?}", e);
                None
            }
        };

        // 2. Initialize Registry immediately
        let mut registry: HashMap<u32, CreationLambda> = HashMap::new();
        registry.insert(1, Box::new(|| {
            let scene = load::<PackedScene>("res://Player.tscn");
            scene.instantiate().expect("Failed to instantiate Player scene").cast::<Node2D>()
        }));

        Self {
            socket,
            network_objects: HashMap::new(),
            registry,
            base,
            sequence_id: 0,
            input_history: VecDeque::with_capacity(20),
            ping_timer: 0.0,
            local_network_id: 0, // 0 means unassigned
            correction_threshold: 40.0,
            correction_smoothing: 0.1,
            simulated_latency_ms: 0.0,
            incoming_queue: Vec::new(),
        }
    }

    fn ready(&mut self) {
        // Note: If a GDScript extends this class and defines _ready(),
        // this method might NOT be called unless super.ready() is used.
        // That's why we moved critical setup to init().

        // We still attempt to send a hello packet here if not overridden
        self.send_packet("127.0.0.1".into(), 4242, PackedByteArray::from(&b"Hello Server"[..]));
    }

    fn exit_tree(&mut self) {
        // Graceful Disconnect: Send a packet to tell server we are leaving.
        if let Some(socket) = &self.socket {
            godot_print!("[Client] Sending Disconnect packet...");
            let packet = DisconnectPacket {
                packet_type: 6,
                network_id: self.local_network_id,
            };
            
            if let Ok(data) = bincode::serialize(&packet) {
                // Best effort send
                let _ = socket.send("127.0.0.1:4242", &data);
            }
        }
    }

    fn process(&mut self, delta: f64) {
        // --- PART B: Latency Measurement (Ping Loop) ---
        // Send a ping every second.
        self.ping_timer += delta;
        if self.ping_timer >= 1.0 {
            self.ping_timer = 0.0;
            self.send_ping();
        }

        let current_time = godot::classes::Time::singleton().get_ticks_msec() as f64;

        if let Some(socket) = &mut self.socket {
            let mut buffer = [0u8; 1024];

            // Non-blocking poll loop: Instead of processing immediately, we add to queue
            while let Some((size, sender_addr)) = socket.poll(&mut buffer) {
                let deliver_time = current_time + self.simulated_latency_ms;
                self.incoming_queue.push(DelayedPacket {
                    deliver_time,
                    sender_addr,
                    data: buffer[0..size].to_vec(),
                });
            }
        }

        // Collect packets ready to be processed
        let mut ready_packets = Vec::new();
        self.incoming_queue.retain(|packet| {
            if packet.deliver_time <= current_time {
                ready_packets.push(packet.clone());
                false // remove from queue
            } else {
                true // keep in queue
            }
        });

        let mut spawns_to_process = Vec::new();
        let mut received_events: Vec<(String, i32, Vec<u8>)> = Vec::new();

        // Process ready packets
        for packet in ready_packets {
            let size = packet.data.len();
            let data_vec = packet.data;
            let sender_addr = packet.sender_addr;

            // Parse sender address (assuming snl returns "IP:PORT" string)
            let parts: Vec<&str> = sender_addr.split(':').collect();
            let ip = parts.get(0).unwrap_or(&"0.0.0.0").to_string();
            let port = parts.get(1).unwrap_or(&"0").parse::<i32>().unwrap_or(0);

            received_events.push((ip, port, data_vec.clone()));

            if size > 0 {
                let packet_type = data_vec[0];
                let buffer = data_vec.as_slice();

                match packet_type {
                         // Welcome (Lab 2 Fix)
                         0 => {
                             if size >= std::mem::size_of::<WelcomePacket>() {
                                let net_id = unsafe {
                                    let raw = buffer.as_ptr() as *const WelcomePacket;
                                    std::ptr::read_unaligned(std::ptr::addr_of!((*raw).network_id))
                                };

                                // RECONNECTION / TIMEOUT MANAGEMENT
                                // If we already had a different local ID, we were disconnected/reconnected.
                                // Clean up the old avatar to avoid "double character".
                                if self.local_network_id != 0 && self.local_network_id != net_id {
                                    godot_print!("[Client] Reconnection detected (ID changed {} -> {}). Cleaning up old local player.", self.local_network_id, net_id);
                                    if let Some(mut old_node) = self.network_objects.remove(&self.local_network_id) {
                                        old_node.queue_free();
                                    }
                                }

                                self.local_network_id = net_id;
                                godot_print!("[Client] Assigned Local Network ID: {}", net_id);

                                // FORCE UPDATE if entity already exists (Fix for Spawn arriving before Welcome)
                                if let Some(node) = self.network_objects.get_mut(&net_id) {
                                    godot_print!("[Client] Updating existing Local Player entity...");
                                    node.call("setup", &[
                                        net_id.to_variant(),
                                        true.to_variant() // is_local = true
                                    ]);
                                }
                             }
                         },
                         // SPAWN (Lab 1)
                         1 => {
                             if size >= std::mem::size_of::<SpawnPacket>() {
                                let (_, net_id, t_id, px, py) = unsafe {
                                    let raw = buffer.as_ptr() as *const SpawnPacket;
                                    (
                                        std::ptr::read_unaligned(std::ptr::addr_of!((*raw).packet_type)),
                                        std::ptr::read_unaligned(std::ptr::addr_of!((*raw).network_id)),
                                        std::ptr::read_unaligned(std::ptr::addr_of!((*raw).type_id)),
                                        std::ptr::read_unaligned(std::ptr::addr_of!((*raw).x)),
                                        std::ptr::read_unaligned(std::ptr::addr_of!((*raw).y)),
                                    )
                                };
                                godot_print!("[Client] Queueing Spawn: NetID={} TypeID={} Pos=({},{})", net_id, t_id, px, py);
                                spawns_to_process.push((net_id, t_id, px, py));
                             } else {
                                godot_print!("[Client] Error: SPAWN packet too small ({} < {})", size, std::mem::size_of::<SpawnPacket>());
                             }
                         },
                         // PingResponse (Lab 2)
                         4 => {
                             if let Ok(pong) = bincode::deserialize::<PingResponse>(&buffer[0..size]) {
                                 let t3 = godot::classes::Time::singleton().get_ticks_msec(); // Client Receive Time
                                 let rtt = t3 - pong.t0; // RTT = t3 - t1 (client send time)
                                 // Note: pong.t1 is Server time, useful for Clock Synchronization (offset = t1 - t0 - RTT/2), but Lab 2 asks for RTT.
                                 godot_print!("[Ping #{}] RTT = {} ms", pong.id, rtt);
                                 
                                 // Bonus: Display RTT in the window title for debug
                                 // let title = format!("RustyGodot Client - RTT: {} ms", rtt);
                                 // DisplayServer::singleton().window_set_title(title.into());
                             }
                         },
                         // StateUpdate (Lab 3 Prep / Lab 2 Movement Replication)
                         5 => {
                             if size >= std::mem::size_of::<StatePacket>() {
                                 // Unsafe read for packed struct
                                 let (net_id, px, py) = unsafe {
                                     let raw = buffer.as_ptr() as *const StatePacket;
                                     (
                                         std::ptr::read_unaligned(std::ptr::addr_of!((*raw).network_id)),
                                         std::ptr::read_unaligned(std::ptr::addr_of!((*raw).x)),
                                         std::ptr::read_unaligned(std::ptr::addr_of!((*raw).y)),
                                     )
                                 };

                                 // Update Node position
                                 if let Some(node) = self.network_objects.get_mut(&net_id) {
                                     let server_pos = Vector2::new(px, py);
                                     if net_id != self.local_network_id {
                                         // Remote player: straight update (could be interpolated too, but basic for now)
                                         node.set_position(server_pos);
                                     } else {
                                         // Local player: "Less naive" Client-Side Prediction Correction
                                         let current_pos = node.get_position();
                                         let distance = current_pos.distance_to(server_pos);

                                         // If local prediction diverges too much from server truth, correct it smoothly
                                         if distance > self.correction_threshold {
                                             let new_pos = current_pos.lerp(server_pos, self.correction_smoothing);
                                             node.set_position(new_pos);
                                             godot_print!("[Client] Prediction divergence ({} > {}). Correcting position...", distance, self.correction_threshold);
                                         }
                                     }
                                 }
                             }
                         },
                         // Disconnect (Bonus / Fix)
                         6 => {
                             if let Ok(pkt) = bincode::deserialize::<DisconnectPacket>(&buffer[0..size]) {
                                 let net_id = pkt.network_id;
                                 godot_print!("[Client] Player Disconnected: {}", net_id);
                                 
                                 if let Some(mut node) = self.network_objects.remove(&net_id) {
                                     node.queue_free();
                                 }
                             }
                         }
                         _ => {
                             godot_print!("[Client] Unknown PacketType: {}", packet_type);
                         }
                    }
                }
            }

        // 1. Emit signals (for generic GDScript usage)
        for (ip, port, data) in received_events {
             let packed_data = PackedByteArray::from(data.as_slice());
             self.base_mut().emit_signal("packet_received", &[
                 ip.to_variant(),
                 port.to_variant(),
                 packed_data.to_variant(),
             ]);
        }

        // 2. Process Lab 1 Spawns
        for (net_id, type_id, x, y) in spawns_to_process {
            self.process_spawn(net_id, type_id, x, y);
        }
    }
}

impl NetworkManager {
    /// Registers a factory function for a specific TypeID.
    /// This is the core of the "Linking Context".
    fn register_type<F>(&mut self, type_id: u32, factory: F)
    where F: Fn() -> Gd<Node2D> + 'static
    {
        self.registry.insert(type_id, Box::new(factory));
    }

    fn send_ping(&mut self) {
        if let Some(socket) = &self.socket {
            // Lab 2 Part B: Send PingRequest
            let t0 = Time::singleton().get_ticks_msec();
            let ping = PingRequest {
                packet_type: 3, // PingRequest
                id: self.sequence_id, // Use sequence as ping ID
                t0,
            };

            if let Ok(data) = bincode::serialize(&ping) {
                 let _ = socket.send("127.0.0.1:4242", &data);
            }
        }
    }

    /// Handles the instantiation of a network entity.
    fn process_spawn(&mut self, net_id: u32, type_id: u32, x: f32, y: f32) {
        // Idempotency check: Don't spawn if already exists
        if self.network_objects.contains_key(&net_id) {
            return;
        }

        // Lookup the factory in the registry
        if let Some(factory) = self.registry.get(&type_id) {
            let mut node2d = factory();
            node2d.set_position(Vector2::new(x, y));

            // Upcast to Node to add it to the Scene Tree.
            // FIXED: Pass by Value (Gd<T> is a smart pointer)
            let node_to_add = node2d.clone().upcast::<Node>();
            
            // Lab 3 Prep: Call setup script to display ID
            // Check if this is OUR player
            let is_local = self.local_network_id > 0 && net_id == self.local_network_id;

            node2d.call("setup", &[
                net_id.to_variant(),
                is_local.to_variant()
            ]);

            self.base_mut().add_child(&node_to_add);

            // Store in our local map for future updates (Move, Destroy...)
            self.network_objects.insert(net_id, node2d);

            godot_print!(
                "[Client] Spawn Success: NetworkID={} (Type {}) at ({}, {})",
                net_id,
                type_id,
                x,
                y
            );
        } else {
            godot_print!("[ERROR] TypeID {} not found in LinkingContext!", type_id);
        }
    }
}
