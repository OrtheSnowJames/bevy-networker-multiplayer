// SPDX-License-Identifier: MIT
//! Bridge between Bevy ECS state and the underlying socket transport.
//!
//! `NetResource` owns the client/server socket state, packet queues, and the
//! list of newly connected peers that need a full-world snapshot.
use bevy::prelude::*;
use bincode::config;
use networker_rs::net::{EasySocketServer, Socket};
use std::{
    net::{SocketAddr, UdpSocket},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use crate::netmsg::NetMessage;

/// Internal mutable network state protected by a mutex.
#[derive(Default)]
struct NetState {
    is_server: Option<bool>,
    server_address: Option<SocketAddr>,
    connections: Vec<Socket>,
    new_connections: Vec<Socket>,
    outbox: Vec<ReplicationPacket>,
    inbox: Vec<ReplicationPacket>,
    message_outbox: Vec<RawNetMessage>,
    message_inbox: Vec<RawNetMessage>,
}

/// Bevy resource that owns the networking transport and packet queues.
#[derive(Resource, Default)]
pub struct NetResource {
    state: Arc<Mutex<NetState>>,
    server: Option<Arc<EasySocketServer>>,
}

impl NetResource {
    /// Creates an empty networking resource.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `true` when the resource is currently running as server.
    pub fn is_server(&self) -> bool {
        self.state.lock().unwrap().is_server == Some(true)
    }

    /// Connects to a server and binds packet handlers to the new socket.
    pub fn join_server(&mut self, server_address: String) {
        let server_address: SocketAddr = server_address
            .parse()
            .expect("server_address must be a valid socket address");

        let udp_socket = UdpSocket::bind("0.0.0.0:0").expect("failed to open UDP socket");
        let socket = Socket::new_udp_with_peer(Arc::new(udp_socket), server_address);
        self.bind_client_socket(socket);

        if let Some(socket) = self.state.lock().unwrap().connections.last().cloned() {
            thread::spawn(move || {
                for _ in 0..20 {
                    socket.send("__networker_join__", []);
                    thread::sleep(Duration::from_millis(100));
                }
            });
        }

        let mut state = self.state.lock().unwrap();
        state.server_address = Some(server_address);
        state.is_server = Some(false);
    }

    /// Starts the server and installs the connection callback.
    pub fn start_server(&mut self, port: u16) {
        let server = Arc::new(EasySocketServer::new());
        let state = Arc::clone(&self.state);

        server.on("connection", move |socket| {
            let state_for_replication = Arc::clone(&state);
            socket.on_bytes(
                "replication",
                move |payload| match bincode::serde::decode_from_slice::<ReplicationPacket, _>(
                    payload,
                    config::standard(),
                ) {
                    Ok((raw, _)) => state_for_replication.lock().unwrap().inbox.push(raw),
                    Err(error) => {
                        warn!(
                            "server failed to decode replication packet bytes={} error={error}",
                            payload.len()
                        );
                    }
                },
            );

            let state_for_message = Arc::clone(&state);
            socket.on_bytes("netmsg", move |payload| {
                if let Ok((raw, _)) = bincode::serde::decode_from_slice::<RawNetMessage, _>(
                    payload,
                    config::standard(),
                ) {
                    state_for_message.lock().unwrap().message_inbox.push(raw);
                }
            });

            let mut state = state.lock().unwrap();
            state.new_connections.push(socket.clone());
            state.connections.push(socket.clone());
        });

        let address = format!("0.0.0.0:{port}");
        let server_for_thread = Arc::clone(&server);
        thread::spawn(move || {
            if let Err(error) = server_for_thread.listen_udp(&address) {
                eprintln!("server failed to listen on {address}: {error}");
            }
        });

        {
            let mut state = self.state.lock().unwrap();
            state.is_server = Some(true);
        }

        self.server = Some(server);
    }

    /// Adds a replication packet to the outgoing queue.
    pub fn queue_packet(&mut self, packet: ReplicationPacket) {
        self.state.lock().unwrap().outbox.push(packet);
    }

    /// Sends a replication packet immediately to one socket.
    pub fn send_packet_to(&self, socket: &Socket, packet: ReplicationPacket) {
        let reliable = packet.requires_reliable_delivery();
        let bytes = bincode::serde::encode_to_vec(packet, config::standard())
            .expect("failed to serialize replication packet");
        socket.send_with_reliability("replication", bytes, reliable);
    }

    /// Injects a packet directly into the incoming queue.
    pub fn inject_packet(&mut self, packet: ReplicationPacket) {
        self.state.lock().unwrap().inbox.push(packet);
    }

    /// Queues a typed network message for broadcast.
    pub fn queue_message<T: NetMessage>(&mut self, message: T) {
        let bytes = bincode::serde::encode_to_vec(&message, config::standard())
            .expect("failed to serialize network message");
        self.state
            .lock()
            .unwrap()
            .message_outbox
            .push(RawNetMessage {
                wire_id: T::WIRE_ID,
                bytes,
            });
    }

    /// Drains all queued outgoing replication packets.
    pub fn drain_outbox(&mut self) -> Vec<ReplicationPacket> {
        self.state.lock().unwrap().outbox.drain(..).collect()
    }

    /// Drains all received replication packets.
    pub fn drain_inbox(&mut self) -> Vec<ReplicationPacket> {
        self.state.lock().unwrap().inbox.drain(..).collect()
    }

    /// Drains sockets that connected since the last snapshot broadcast.
    pub fn drain_new_connections(&mut self) -> Vec<Socket> {
        self.state
            .lock()
            .unwrap()
            .new_connections
            .drain(..)
            .collect()
    }

    /// Drains raw messages regardless of wire type.
    pub fn drain_message_inbox(&mut self) -> Vec<RawNetMessage> {
        self.state.lock().unwrap().message_inbox.drain(..).collect()
    }

    /// Extracts only messages of type `T`, leaving the rest queued.
    pub fn drain_messages<T: NetMessage>(&mut self) -> Vec<T> {
        let messages = self.drain_message_inbox();
        let mut matched = Vec::new();
        let mut unmatched = Vec::new();

        for message in messages {
            if message.wire_id == T::WIRE_ID {
                if let Ok((message, _)) =
                    bincode::serde::decode_from_slice::<T, _>(&message.bytes, config::standard())
                {
                    matched.push(message);
                    continue;
                }
            }

            unmatched.push(message);
        }

        if !unmatched.is_empty() {
            self.state.lock().unwrap().message_inbox.extend(unmatched);
        }

        matched
    }

    /// Serializes and flushes all queued packets to connected clients.
    pub fn flush_outbox(&mut self) {
        let (packets, message_packets, connections) = {
            let state = self.state.lock().unwrap();
            if (state.outbox.is_empty() && state.message_outbox.is_empty())
                || state.connections.is_empty()
            {
                return;
            }

            (
                state.outbox.clone(),
                state.message_outbox.clone(),
                state.connections.clone(),
            )
        };

        {
            let mut state = self.state.lock().unwrap();
            state.outbox.clear();
            state.message_outbox.clear();
        }

        for packet in packets {
            let reliable = packet.requires_reliable_delivery();
            let bytes = bincode::serde::encode_to_vec(packet, config::standard())
                .expect("failed to serialize replication packet");
            for socket in &connections {
                socket.send_with_reliability("replication", bytes.clone(), reliable);
            }
        }

        for packet in message_packets {
            for socket in &connections {
                let bytes = bincode::serde::encode_to_vec(&packet, config::standard())
                    .expect("failed to serialize network message");
                socket.send("netmsg", bytes);
            }
        }
    }

    /// Placeholder hook for future socket polling implementations.
    pub fn poll_incoming(&mut self) {}

    /// Binds replication and message handlers to a socket.
    fn bind_client_socket(&self, socket: Socket) {
        let state_for_replication = Arc::clone(&self.state);
        let socket_for_listener = socket.clone();
        socket.on_bytes(
            "replication",
            move |payload| match bincode::serde::decode_from_slice::<ReplicationPacket, _>(
                payload,
                config::standard(),
            ) {
                Ok((raw, _)) => state_for_replication.lock().unwrap().inbox.push(raw),
                Err(error) => {
                    warn!(
                        "client failed to decode replication packet bytes={} error={error}",
                        payload.len()
                    );
                }
            },
        );

        let state_for_message = Arc::clone(&self.state);
        socket.on_bytes("netmsg", move |payload| {
            if let Ok((raw, _)) =
                bincode::serde::decode_from_slice::<RawNetMessage, _>(payload, config::standard())
            {
                state_for_message.lock().unwrap().message_inbox.push(raw);
            }
        });

        self.state.lock().unwrap().connections.push(socket.clone());
        thread::spawn(move || socket_for_listener.listen_udp());
    }
}

/// Raw wire payload for a typed network message.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RawNetMessage {
    /// Stable wire identifier for the message type.
    pub wire_id: u64,
    /// Serialized message bytes.
    pub bytes: Vec<u8>,
}

/// Packet types used by entity and resource replication.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum ReplicationPacket {
    /// Spawn a replicated entity, optionally with a prefab.
    SpawnEntity {
        network_id: u64,
        prefab_wire_id: u64,
    },
    /// Despawn a replicated entity.
    DespawnEntity { network_id: u64 },
    /// Update a replicated component on one entity.
    UpdateComponent {
        network_id: u64,
        component_wire_id: u64,
        sequence: u64,
        bytes: Vec<u8>,
    },
    /// Replace a replicated resource snapshot.
    UpdateResource {
        resource_wire_id: u64,
        bytes: Vec<u8>,
    },
}

impl ReplicationPacket {
    pub fn requires_reliable_delivery(&self) -> bool {
        matches!(
            self,
            Self::SpawnEntity { .. } | Self::DespawnEntity { .. } | Self::UpdateResource { .. }
        )
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::SpawnEntity { .. } => "SpawnEntity",
            Self::DespawnEntity { .. } => "DespawnEntity",
            Self::UpdateComponent { .. } => "UpdateComponent",
            Self::UpdateResource { .. } => "UpdateResource",
        }
    }
}
