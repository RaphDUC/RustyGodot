mod protocol;

use bevy::prelude::*;
use bevy::app::{ScheduleRunnerPlugin, RunMode};
use bincode;
use snl::GameSocket;
use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{info, warn};
use tracing_subscriber::FmtSubscriber;

use crate::protocol::{
    DisconnectPacket, InputPacket, InputState, NetworkID, PacketType, PingRequest, PingResponse, SpawnPacket,
    StatePacket, WelcomePacket,
};

// ------------------------------------------------------------------------
// Server State Resources
// ------------------------------------------------------------------------

/// Global resource to hold the server's network state.
#[derive(Resource)]
struct ServerState {
    /// The UDP socket wrapper (SNL library).
    socket: GameSocket,
    /// Counter for generating unique NetworkIDs.
    next_network_id: NetworkID,
    /// List of connected client addresses (e.g., "127.0.0.1:12345").
    clients: Vec<String>,
    /// Map of Client Address -> Bevy Entity ID (for input processing)
    addr_to_entity: HashMap<String, Entity>,
    /// Map of Client Address -> Assigned NetworkID (for disconnect broadcast)
    addr_to_id: HashMap<String, NetworkID>,
    /// Last heartbeat timestamp for timeout handling
    last_heartbeat: HashMap<String, std::time::Instant>,
    /// Track last sequence number per client for Packet Ordering (Lab 2)
    client_last_sequence: HashMap<String, u32>,
    /// Timer accumulation for broadcast frequency control (Optimisation Lab 2)
    broadcast_timer: f32,
    /// Configurable broadcast rate (Hz)
    broadcast_rate_hz: f32,
}

#[derive(Resource)]
struct ServerConfig {
    tick_rate_hz: f64,
}

/// Component added to every entity that should be synchronized over the network.
#[derive(Component)]
#[allow(dead_code)] // Suppress warnings for now as requested
struct NetworkedEntity {
    id: NetworkID,
    type_id: u32,
}

/// Component to store the last known input state for an entity.
#[derive(Component, Default)]
struct PlayerInput {
    current: InputState,
}

fn main() {
    // 0. Initialize Logging (Tracing)
    let subscriber = FmtSubscriber::builder()
        .with_max_level(tracing::Level::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber)
        .expect("setting default subscriber failed");

    info!("Starting RustyGodot Server...");

    // Read config from args or environment if needed, here we default to 60.0, but we can make it configurable.
    let args: Vec<String> = std::env::args().collect();

    let tick_rate_hz: f64 = args.get(1).and_then(|t| t.parse().ok()).unwrap_or(60.0);
    let broadcast_rate_hz: f32 = args.get(2).and_then(|t| t.parse().ok()).unwrap_or(60.0);

    info!("Tick rate set to {} Hz", tick_rate_hz);
    info!("Broadcast rate set to {} Hz", broadcast_rate_hz);

    // 1. Initialize SNL: Bind to port 4242 on all interfaces.
    let socket = GameSocket::new("0.0.0.0:4242").expect("Failed to bind SNL socket");

    // 2. Configure Bevy App
    App::new()
        .insert_resource(ServerConfig { tick_rate_hz })
        .add_plugins(MinimalPlugins.set(ScheduleRunnerPlugin::run_loop(
            Duration::from_secs_f64(1.0 / tick_rate_hz),
        )))
        .insert_resource(ServerState {
            socket,
            next_network_id: 100, // Starts at 100 as per instructions
            clients: Vec::new(),
            addr_to_entity: HashMap::new(),
            addr_to_id: HashMap::new(), // Initialize new map
            last_heartbeat: HashMap::new(),
            client_last_sequence: HashMap::new(),
            broadcast_timer: 0.0,
            broadcast_rate_hz,
        })
        // AJOUT: Le système move_players était manquant ! Sans lui, le serveur reçoit les inputs
        // mais ne met jamais à jour la position des entités, donc elles restent à (0,0).
        .add_systems(Update, (handle_network, move_players, broadcast_state, handle_timeouts))
        .run();
}

/// Main Network System running every frame.
fn handle_network(
    mut commands: Commands,
    mut state: ResMut<ServerState>,
    mut query: Query<(Entity, &mut PlayerInput, &NetworkedEntity, &Transform)>
) {
    let mut buffer = [0u8; 1024];

    // Non-blocking receive loop.
    while let Some((size, sender_addr)) = state.socket.poll(&mut buffer) {
        // Update heartbeat
        state
            .last_heartbeat
            .insert(sender_addr.clone(), std::time::Instant::now());

        // Peek at packet type (Byte 0)
        if size > 0 {
            let packet_type = buffer[0];

            match packet_type {
                // PacketType::Input = 2
                2 => {
                    if let Ok(input_packet) = bincode::deserialize::<InputPacket>(&buffer[..size]) {
                        // Check Sequence (Lab 2 Requirement)
                        let last_seq = state
                            .client_last_sequence
                            .entry(sender_addr.clone())
                            .or_insert(0);

                        if input_packet.sequence > *last_seq {
                            *last_seq = input_packet.sequence;

                            // Process inputs
                            if (input_packet.count as usize) > 0 {
                                // FIX: Client sends newest input at index 0 (push_front)
                                // Old code reading last_idx was reading the oldest input of the history!
                                let latest_input = input_packet.inputs[0];

                                // Apply to the correct entity
                                if let Some(entity) = state.addr_to_entity.get(&sender_addr) {
                                    if let Ok((_entry_entity, mut input_comp, _, _)) = query.get_mut(*entity) {
                                        input_comp.current = latest_input;

                                        // MODULAR LOG (Debug)
                                        // Display the received binary mask every frame if not null
                                        if latest_input.0 != 0 {
                                            // info!("[Server] Input Active: {:#010b}", latest_input.0);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                // PacketType::PingRequest = 3
                3 => {
                    if let Ok(ping) = bincode::deserialize::<PingRequest>(&buffer[..size]) {
                        // Respond immediately
                        let t1 = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap()
                            .as_millis() as u64;
                        let response = PingResponse {
                            packet_type: 4, // PacketType::PingResponse
                            id: ping.id,
                            t0: ping.t0,
                            t1,
                        };

                        if let Ok(data) = bincode::serialize(&response) {
                            let _ = state.socket.send(&sender_addr, &data);
                        }
                    }
                }
                // PacketType::Disconnect = 6
                6 => {
                    if let Ok(disconnect) = bincode::deserialize::<DisconnectPacket>(&buffer[..size]) {
                        info!("[Server] Disconnect received from Client {} (ID: {}).", sender_addr, disconnect.network_id);
                        
                        // Explicit removal
                        if let Some(pos) = state.clients.iter().position(|x| *x == sender_addr) {
                            state.clients.remove(pos);
                        }

                        // Broadcast Disconnect to others
                        let broadcast_packet = DisconnectPacket {
                            packet_type: PacketType::Disconnect as u8,
                            network_id: disconnect.network_id,
                        };
                        if let Ok(data) = bincode::serialize(&broadcast_packet) {
                            for client in &state.clients {
                                let _ = state.socket.send(client, &data);
                            }
                        }

                        if let Some(entity) = state.addr_to_entity.remove(&sender_addr) {
                            state.addr_to_id.remove(&sender_addr);
                            commands.entity(entity).despawn();
                            info!("[Server] Despawned entity for disconnecting client.");
                        }
                    }
                }
                _ => {}
            }
        }

        // --- Connection Handling ---
        if !state.clients.contains(&sender_addr) {
            info!("[Server] Client {} connected.", sender_addr);
            state.clients.push(sender_addr.clone());

            // 1. Create the entity
            let new_id = state.next_network_id;
            state.next_network_id += 1;

            // Store ID mapping
            state.addr_to_id.insert(sender_addr.clone(), new_id);

            // 1.1 Send Welcome Packet (Assign ID to Client)
            let welcome = WelcomePacket {
                packet_type: PacketType::Welcome as u8,
                network_id: new_id,
            };
            let welcome_data = unsafe {
                std::slice::from_raw_parts(
                    &welcome as *const WelcomePacket as *const u8,
                    std::mem::size_of::<WelcomePacket>(),
                )
            };
            let _ = state.socket.send(&sender_addr, welcome_data);

            // 1.2 Send Existing Entities to New Client
            for (_, _, net_entity, transform) in query.iter() {
                let spawn_existing = SpawnPacket {
                    packet_type: PacketType::Spawn as u8,
                    network_id: net_entity.id,
                    type_id: net_entity.type_id,
                    x: transform.translation.x,
                    y: transform.translation.y,
                };
                let spawn_data = unsafe {
                    std::slice::from_raw_parts(
                        &spawn_existing as *const SpawnPacket as *const u8,
                        std::mem::size_of::<SpawnPacket>(),
                    )
                };
                let _ = state.socket.send(&sender_addr, spawn_data);
            }

            let entity = commands
                .spawn((
                    NetworkedEntity {
                        id: new_id,
                        type_id: 1,
                    },
                    Transform::from_xyz(0.0, 0.0, 0.0),
                    PlayerInput::default(),
                ))
                .id();

            state.addr_to_entity.insert(sender_addr.clone(), entity);

            // 2. Broadcast Spawn to ALL clients
            let packet = SpawnPacket {
                packet_type: PacketType::Spawn as u8,
                network_id: new_id,
                type_id: 1,
                x: 0.0,
                y: 0.0,
            };
            let data = unsafe {
                std::slice::from_raw_parts(
                    &packet as *const SpawnPacket as *const u8,
                    std::mem::size_of::<SpawnPacket>(),
                )
            };

            for client_addr in &state.clients {
                let _ = state.socket.send(client_addr, data);
            }

            info!("[Server] Spawning Entity ID {} for all clients.", new_id);

            // Note: In a real system, we should also send existing entities to the NEW client.
            // For now, only new spawns are broadcasted (Lab 1 scope).
            // Lab 3 Sync (StateUpdate) will fix this naturally.
        }
    }
}

/// System: Apply Inputs to Movement
fn move_players(time: Res<Time>, mut query: Query<(&mut Transform, &PlayerInput)>) {
    const SPEED: f32 = 200.0; // Pixels per second
    let delta = time.delta_secs();
    
    // Server-Side Input Mapping (Decoupled from protocol)
    const BIT_UP: u8 = 0;
    const BIT_DOWN: u8 = 1;
    const BIT_LEFT: u8 = 2;
    const BIT_RIGHT: u8 = 3;

    for (mut transform, input) in query.iter_mut() {
        let i = &input.current;
        let mut velocity = Vec3::ZERO;

        if i.is_active(BIT_UP) { velocity.y -= 1.0; }
        if i.is_active(BIT_DOWN) { velocity.y += 1.0; }
        if i.is_active(BIT_LEFT) { velocity.x -= 1.0; }
        if i.is_active(BIT_RIGHT) { velocity.x += 1.0; }

        if velocity.length_squared() > 0.0 {
            velocity = velocity.normalize() * SPEED;
            transform.translation += velocity * delta;
        }
    }
}

/// System: Broadcast State (Snapshot).
/// Runs every frame (60Hz). In prod, maybe 20Hz.
fn broadcast_state(mut state: ResMut<ServerState>, time: Res<Time>, query: Query<(&Transform, &NetworkedEntity)>) {
    // Configurable variable for broadcast frequency (Hz).
    let broadcast_interval: f32 = 1.0 / state.broadcast_rate_hz;

    state.broadcast_timer += time.delta_secs();
    if state.broadcast_timer < broadcast_interval {
        return;
    }
    state.broadcast_timer -= broadcast_interval;

    for (transform, net_entity) in query.iter() {
        let packet = StatePacket {
            packet_type: PacketType::StateUpdate as u8,
            network_id: net_entity.id,
            x: transform.translation.x,
            y: transform.translation.y,
        };

        let data = unsafe {
            std::slice::from_raw_parts(
                &packet as *const StatePacket as *const u8,
                std::mem::size_of::<StatePacket>(),
            )
        };

        // Send to everyone
        for client in &state.clients {
            let _ = state.socket.send(client, data);
        }
    }
}

/// System: Handle Timeouts.
fn handle_timeouts(mut commands: Commands, mut state: ResMut<ServerState>, time: Res<Time>) {
    // Check every second roughly
    if time.elapsed_secs() % 1.0 > 0.1 { return; }

    // Timeout adjusted to 5s for responsiveness.
    let timeout_duration = Duration::from_secs(5);
    let now = std::time::Instant::now();
    let mut to_remove_addrs = Vec::new();

    for client in &state.clients {
        if let Some(last) = state.last_heartbeat.get(client) {
            if now.duration_since(*last) > timeout_duration {
                to_remove_addrs.push(client.clone());
            }
        } else {
             // Should verify why heartbeat not set (maybe initial connection)
             // For now, safe default is to track from first packet.
             // If never received packet, we can probably safely remove if connected long ago
             // But let's assume heartbeat is set on first packet.
        }
    }

    for client_addr in to_remove_addrs {
        info!("[Server] Client {} timed out. Disconnecting.", client_addr);

        // Remove from clients list
        if let Some(pos) = state.clients.iter().position(|x| *x == client_addr) {
            state.clients.remove(pos);
        }

        // Identify NetworkID for broadcast
        let net_id_opt = state.addr_to_id.remove(&client_addr);

        // Despawn entity
        if let Some(entity) = state.addr_to_entity.remove(&client_addr) {
            commands.entity(entity).despawn();
        }

        // Broadcast Disconnect (Despawn) to remaining clients
        if let Some(net_id) = net_id_opt {
             let packet = DisconnectPacket {
                packet_type: PacketType::Disconnect as u8,
                network_id: net_id,
            };
            if let Ok(data) = bincode::serialize(&packet) {
                // Send to all REMAINING clients
                for other_client in &state.clients {
                    let _ = state.socket.send(other_client, &data);
                }
            }
        }
    }
}
