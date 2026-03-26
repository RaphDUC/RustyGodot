use snl::GameSocket;
use std::thread;
use std::time::{Duration, Instant};
use std::mem;

// Local copy of the packet structure for testing.
// Must be identical to the server's definition (including `packed`).
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
struct SpawnPacket {
    pub packet_type: u8,
    pub network_id: u32,
    pub type_id: u32,
    pub x: f32,
    pub y: f32,
}

/// A simple test client to validate server behavior.
///
/// Scenario:
/// 1. Connects to the server.
/// 2. Sends a handshake/hello packet.
/// 3. Waits for a SPAWN packet (OpCode 1).
/// 4. Validates the packet content (Entity ID 100).
fn main() {
    println!("--- Starting Test Client ---");

    // 1. Bind to a different port than the server (e.g., 8080) to avoid conflict.
    let client_socket = GameSocket::new("127.0.0.1:8080").expect("Failed to bind client socket");

    // Server address (localhost)
    let server_addr = "127.0.0.1:4242";

    // 2. Send a handshake packet to register with the server.
    // The server registers the client upon receiving the first packet.
    client_socket.send(server_addr, b"Hello Server").expect("Failed to send handshake");
    println!("> Handshake sent to server ({}). Awaiting SPAWN...", server_addr);

    // 3. Receive Loop (Timeout: 3 seconds)
    // We poll the socket for a response.
    let mut buffer = [0u8; 1024];
    let start = Instant::now();

    while start.elapsed() < Duration::from_secs(3) {
        if let Some((size, sender)) = client_socket.poll(&mut buffer) {
            // Only process packets from the target server
            if sender == server_addr {
                println!("< Packet received: {} bytes", size);

                // validate packet size
                if size == mem::size_of::<SpawnPacket>() {
                    // Reconstruct struct from raw bytes.
                    // IMPORTANT: Use `read_unaligned` because `packed` structs (or network buffers)
                    // might not be aligned to 4-byte boundaries in memory, which would panic/corrupt on some CPUs.
                    let (packet_type, network_id, type_id) = unsafe {
                        // Read the full struct without assuming alignment
                        let packet: SpawnPacket = std::ptr::read_unaligned(
                            buffer.as_ptr() as *const SpawnPacket
                        );

                        // Extract fields safely
                        let packet_type = packet.packet_type;

                        // Although `packet` is a local copy, reading its fields might still be unaligned
                        // if the compiler didn't align the struct itself due to `packed`.
                        // Using `addr_of!` ensures we get a raw pointer to the field, then `read_unaligned` reads it safely.
                        let network_id = std::ptr::read_unaligned(
                            std::ptr::addr_of!(packet.network_id)
                        );
                        let type_id = std::ptr::read_unaligned(
                            std::ptr::addr_of!(packet.type_id)
                        );

                        (packet_type, network_id, type_id)
                    };

                    println!("  [Decoded Data]");
                    println!("  PacketType : {}", packet_type);
                    println!("  NetworkID  : {}", network_id);
                    println!("  TypeID     : {}", type_id);

                    // Assertions for validation
                    if packet_type == 1 && network_id == 100 {
                        println!("\n[SUCCESS] The server correctly spawned Entity 100!");
                        return;
                    }
                } else {
                    println!("  [ERROR] Unexpected size (Received: {}, Expected: {})", size, mem::size_of::<SpawnPacket>());
                }
            }
        }
        // Avoid 100% CPU usage loop
        thread::sleep(Duration::from_millis(10));
    }

    println!("\n[FAILURE] Timed out with no valid response from server.");
    // Exit with error code if failed
    std::process::exit(1);
}