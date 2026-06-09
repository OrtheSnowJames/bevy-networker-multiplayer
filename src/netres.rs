// SPDX-License-Identifier: MIT
use bevy::prelude::*;
use base64::{engine::general_purpose, Engine as _};
use bincode::config;
use networker_rs::net::{EasySocketServer, Socket};
use std::{
    net::{SocketAddr, TcpStream},
    sync::{Arc, Mutex},
    thread,
};

use crate::netmsg::NetMessage;

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

#[derive(Resource, Default)]
pub struct NetResource {
    state: Arc<Mutex<NetState>>,
    server: Option<Arc<EasySocketServer>>,
}

impl NetResource {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_server(&self) -> bool {
        self.state.lock().unwrap().is_server == Some(true)
    }

    pub fn join_server(&mut self, server_address: String) {
        let server_address: SocketAddr = server_address
            .parse()
            .expect("server_address must be a valid socket address");

        let stream = TcpStream::connect(server_address).expect("failed to connect to server");
        let socket = Socket::new_tcp(stream);
        self.bind_socket(socket);

        let mut state = self.state.lock().unwrap();
        state.server_address = Some(server_address);
        state.is_server = Some(false);
    }

    pub fn start_server(&mut self, port: u16) {
        let server = Arc::new(EasySocketServer::new());
        let state = Arc::clone(&self.state);

        server.on("connection", move |socket| {
            let socket_for_listener = socket.clone();
            let state_for_replication = Arc::clone(&state);
            socket.on("replication", move |payload| {
                if let Some(raw) = decode_replication_packet(payload) {
                    state_for_replication.lock().unwrap().inbox.push(raw);
                }
            });

            let state_for_message = Arc::clone(&state);
            socket.on("netmsg", move |payload| {
                if let Some(raw) = decode_raw_net_message(payload) {
                    state_for_message.lock().unwrap().message_inbox.push(raw);
                }
            });

            let mut state = state.lock().unwrap();
            state.new_connections.push(socket.clone());
            state.connections.push(socket.clone());
            thread::spawn(move || socket_for_listener.listen_tcp());
        });

        let address = format!("0.0.0.0:{port}");
        let server_for_thread = Arc::clone(&server);
        thread::spawn(move || {
            let _ = server_for_thread.listen_tcp(&address);
        });

        {
            let mut state = self.state.lock().unwrap();
            state.is_server = Some(true);
        }

        self.server = Some(server);
    }

    pub fn queue_packet(&mut self, packet: ReplicationPacket) {
        self.state.lock().unwrap().outbox.push(packet);
    }

    pub fn send_packet_to(&self, socket: &Socket, packet: ReplicationPacket) {
        let bytes = bincode::serde::encode_to_vec(packet, config::standard())
            .expect("failed to serialize replication packet");
        let payload = general_purpose::STANDARD.encode(bytes);
        socket.emit_with("replication", &payload);
    }

    pub fn inject_packet(&mut self, packet: ReplicationPacket) {
        self.state.lock().unwrap().inbox.push(packet);
    }

    pub fn queue_message<T: NetMessage>(&mut self, message: T) {
        let bytes = bincode::serde::encode_to_vec(&message, config::standard())
            .expect("failed to serialize network message");
        self.state.lock().unwrap().message_outbox.push(RawNetMessage {
            wire_id: T::WIRE_ID,
            bytes,
        });
    }

    pub fn drain_outbox(&mut self) -> Vec<ReplicationPacket> {
        self.state.lock().unwrap().outbox.drain(..).collect()
    }

    pub fn drain_inbox(&mut self) -> Vec<ReplicationPacket> {
        self.state.lock().unwrap().inbox.drain(..).collect()
    }

    pub fn drain_new_connections(&mut self) -> Vec<Socket> {
        self.state.lock().unwrap().new_connections.drain(..).collect()
    }

    pub fn drain_message_inbox(&mut self) -> Vec<RawNetMessage> {
        self.state.lock().unwrap().message_inbox.drain(..).collect()
    }

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
            let bytes = bincode::serde::encode_to_vec(packet, config::standard())
                .expect("failed to serialize replication packet");
            let payload = general_purpose::STANDARD.encode(bytes);
            for socket in &connections {
                socket.emit_with("replication", &payload);
            }
        }

        for packet in message_packets {
            let payload = general_purpose::STANDARD.encode(packet.bytes);
            for socket in &connections {
                socket.emit_with("netmsg", &format!("{}|{}", packet.wire_id, payload));
            }
        }
    }

    pub fn poll_incoming(&mut self) {}

    fn bind_socket(&self, socket: Socket) {
        let state_for_replication = Arc::clone(&self.state);
        let socket_for_listener = socket.clone();
        socket.on("replication", move |payload| {
            if let Some(raw) = decode_replication_packet(payload) {
                state_for_replication.lock().unwrap().inbox.push(raw);
            }
        });

        let state_for_message = Arc::clone(&self.state);
        socket.on("netmsg", move |payload| {
            if let Some(raw) = decode_raw_net_message(payload) {
                state_for_message.lock().unwrap().message_inbox.push(raw);
            }
        });

        self.state.lock().unwrap().connections.push(socket.clone());
        thread::spawn(move || socket_for_listener.listen_tcp());
    }
}

#[derive(Debug, Clone)]
pub struct RawNetMessage {
    pub wire_id: u64,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum ReplicationPacket {
    SpawnEntity {
        network_id: u64,
        prefab_wire_id: u64,
    },
    DespawnEntity { network_id: u64 },
    UpdateComponent {
        network_id: u64,
        component_wire_id: u64,
        bytes: Vec<u8>,
    },
    UpdateResource {
        resource_wire_id: u64,
        bytes: Vec<u8>,
    },
}

fn decode_replication_packet(payload: &str) -> Option<ReplicationPacket> {
    let bytes = general_purpose::STANDARD.decode(payload).ok()?;
    bincode::serde::decode_from_slice::<ReplicationPacket, _>(&bytes, config::standard())
        .ok()
        .map(|(packet, _)| packet)
}

fn decode_raw_net_message(payload: &str) -> Option<RawNetMessage> {
    let (wire_id, payload) = payload.split_once('|')?;
    let wire_id = wire_id.parse().ok()?;
    let bytes = general_purpose::STANDARD.decode(payload).ok()?;
    Some(RawNetMessage { wire_id, bytes })
}
