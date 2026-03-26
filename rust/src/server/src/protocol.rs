use serde::{Deserialize, Serialize};
use std::fmt;

/// Unique identifier for a networked entity.
pub type NetworkID = u32;
/// Unique identifier for the type of entity (e.g., Player = 1).
pub type TypeID = u32;

/// Network Operation Codes (OpCodes).
/// Identifies the type of packet being sent.
#[repr(u8)] // Forces the enum to be encoded as a single byte
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketType {
    Welcome = 0,
    Spawn = 1,
    Input = 2,
    PingRequest = 3,
    PingResponse = 4,
    StateUpdate = 5,
    Disconnect = 6,
}

/// Packet sent by the server to assigning a NetworkID to a connecting client.
#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct WelcomePacket {
    pub packet_type: u8, // = 0
    pub network_id: NetworkID,
}

/// Packet sent by the server to instruct clients to spawn an entity.
///
/// # Binary Layout
/// Uses `#[repr(C, packed)]` to ensure a strict memory layout without padding.
/// This is crucial for cross-language compatibility (e.g., C++ servers or direct memory access).
///
/// Layout (17 bytes total):
/// - packet_type: u8 (1 byte)
/// - network_id:  u32 (4 bytes, Little Endian)
/// - type_id:     u32 (4 bytes, Little Endian)
/// - x:           f32 (4 bytes, IEEE 754)
/// - y:           f32 (4 bytes, IEEE 754)
#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct SpawnPacket {
    pub packet_type: u8,
    pub network_id: NetworkID,
    pub type_id: TypeID,
    pub x: f32,
    pub y: f32,
}

/// Packet sent by Server to *ALL* clients to update an entity's position.
/// This corresponds to the "State" or "Snapshot" of the Lab 3 preparation.
#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct StatePacket {
    pub packet_type: u8, // = 5
    pub network_id: NetworkID,
    pub x: f32,
    pub y: f32,
    pub last_processed_sequence: u32, // Added for Server Reconciliation
}

/// Bitbox (1 byte) for input compression
/// Bits:
/// 0: UP
/// 1: DOWN
/// 2: LEFT
/// 3: RIGHT
/// 4: ACTION
#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default)]
#[repr(transparent)]
pub struct InputState(pub u8);

impl InputState {
    pub fn new(bits: u8) -> Self {
        Self(bits)
    }

    pub fn is_active(&self, bit_index: u8) -> bool {
        (self.0 & (1 << bit_index)) != 0
    }
}

impl fmt::Display for InputState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Bits[{:08b}]", self.0)
    }
}

/// A Run-Length Encoded block of inputs.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default)]
pub struct RleInput {
    pub state: InputState,
    pub count: u8,
}

/// Packet sent by Client to Server containing input history.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct InputPacket {
    pub packet_type: u8, // = 2
    pub sequence: u32,
    /// Run-Length Encoded history to save UDP bandwidth
    pub inputs: Vec<RleInput>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct PingRequest {
    pub packet_type: u8, // = 3
    pub id: u32,
    pub t0: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct PingResponse {
    pub packet_type: u8, // = 4
    pub id: u32,
    pub t0: u64,
    pub t1: u64,
}


/// Packet sent by the client to notify the server of a graceful disconnection.
#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct DisconnectPacket {
    pub packet_type: u8, // = 6
    pub network_id: NetworkID,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem;

    #[test]
    fn validate_packet_layout_and_values() {
        // 1. Verify exact size (Packed = no padding)
        let expected_size = 17; // 1 (u8) + 4 (u32) + 4 (u32) + 4 (f32) + 4 (f32)
        assert_eq!(mem::size_of::<SpawnPacket>(), expected_size, "Incorrect struct size");

        // 2. Simulate data
        let packet = SpawnPacket {
            packet_type: 1,
            network_id: 100,
            type_id: 50,
            x: 10.5,
            y: -5.0,
        };

        // 3. Convert to raw bytes (as the server does)
        let bytes = unsafe {
            std::slice::from_raw_parts(
                &packet as *const SpawnPacket as *const u8,
                mem::size_of::<SpawnPacket>()
            )
        };

        // 4. Byte-by-byte verification (Standard Little Endian on x86)
        // packet_type (1)
        assert_eq!(bytes[0], 1);

        // network_id (100) -> [100, 0, 0, 0]
        assert_eq!(bytes[1], 100);
        assert_eq!(bytes[2], 0);

        // type_id (50) -> [50, 0, 0, 0] at index 1+4 = 5
        assert_eq!(bytes[5], 50);

        println!("Memory layout validated: {:?}", bytes);
    }
}