//! Lets several stickman.exe processes see and hit each other.
//!
//! Each process binds one UDP port in a small range on 127.0.0.1 (never the
//! network) and a few times a second sends its position to every other port
//! in the range. Hits are sent the same way and the target picks them out.

use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::time::{Duration, Instant};

use crate::math::{v2, Vec2};

const MAGIC: [u8; 4] = *b"STK1";
const BASE_PORT: u16 = 47_810;
const SLOTS: u16 = 16;
const SEND_EVERY: f32 = 0.05;
const TIMEOUT: Duration = Duration::from_millis(1000);

const KIND_STATE: u8 = 1;
const KIND_HIT: u8 = 2;

#[derive(Clone, Copy, Debug)]
pub struct Peer {
    pub id: u32,
    pub hip: Vec2,
    pub vel: Vec2,
    pub ragdoll: bool,
    seen: Instant,
}

pub struct Hit {
    pub from: u32,
    pub impulse: Vec2,
}

pub struct Peers {
    sock: Option<UdpSocket>,
    port: u16,
    pub id: u32,
    pub list: Vec<Peer>,
    timer: f32,
}

fn put_f32(buf: &mut Vec<u8>, v: f32) {
    buf.extend_from_slice(&v.to_le_bytes());
}

fn get_f32(b: &[u8], at: usize) -> f32 {
    f32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn get_u32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

impl Peers {
    pub fn new() -> Self {
        let id = std::process::id();
        for i in 0..SLOTS {
            let port = BASE_PORT + i;
            if let Ok(sock) = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)) {
                if sock.set_nonblocking(true).is_ok() {
                    return Peers { sock: Some(sock), port, id, list: Vec::new(), timer: 0.0 };
                }
            }
        }
        // All slots taken: run alone.
        Peers { sock: None, port: 0, id, list: Vec::new(), timer: 0.0 }
    }

    fn broadcast(&self, msg: &[u8]) {
        let Some(sock) = &self.sock else { return };
        for i in 0..SLOTS {
            let port = BASE_PORT + i;
            if port != self.port {
                let _ = sock.send_to(msg, SocketAddrV4::new(Ipv4Addr::LOCALHOST, port));
            }
        }
    }

    pub fn send_hit(&self, target: u32, impulse: Vec2) {
        let mut msg = Vec::with_capacity(21);
        msg.extend_from_slice(&MAGIC);
        msg.push(KIND_HIT);
        msg.extend_from_slice(&self.id.to_le_bytes());
        msg.extend_from_slice(&target.to_le_bytes());
        put_f32(&mut msg, impulse.x);
        put_f32(&mut msg, impulse.y);
        self.broadcast(&msg);
    }

    /// Send our state now and then, read everything waiting, return hits on us.
    pub fn update(&mut self, dt: f32, hip: Vec2, vel: Vec2, ragdoll: bool) -> Vec<Hit> {
        let mut hits = Vec::new();
        if self.sock.is_none() {
            return hits;
        }

        self.timer -= dt;
        if self.timer <= 0.0 {
            self.timer = SEND_EVERY;
            let mut msg = Vec::with_capacity(26);
            msg.extend_from_slice(&MAGIC);
            msg.push(KIND_STATE);
            msg.extend_from_slice(&self.id.to_le_bytes());
            put_f32(&mut msg, hip.x);
            put_f32(&mut msg, hip.y);
            put_f32(&mut msg, vel.x);
            put_f32(&mut msg, vel.y);
            msg.push(ragdoll as u8);
            self.broadcast(&msg);
        }

        let mut buf = [0u8; 64];
        for _ in 0..256 {
            let Some(sock) = &self.sock else { break };
            let n = match sock.recv_from(&mut buf) {
                Ok((n, _)) => n,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                // Windows reports "connection reset" for sends to empty ports.
                Err(_) => continue,
            };
            let b = &buf[..n];
            if n < 9 || b[..4] != MAGIC {
                continue;
            }
            let from = get_u32(b, 5);
            if from == self.id {
                continue;
            }
            match b[4] {
                KIND_STATE if n >= 26 => {
                    let peer = Peer {
                        id: from,
                        hip: v2(get_f32(b, 9), get_f32(b, 13)),
                        vel: v2(get_f32(b, 17), get_f32(b, 21)),
                        ragdoll: b[25] != 0,
                        seen: Instant::now(),
                    };
                    match self.list.iter_mut().find(|p| p.id == from) {
                        Some(p) => *p = peer,
                        None => self.list.push(peer),
                    }
                }
                KIND_HIT if n >= 21 => {
                    if get_u32(b, 9) == self.id {
                        hits.push(Hit { from, impulse: v2(get_f32(b, 13), get_f32(b, 17)) });
                    }
                }
                _ => {}
            }
        }
        self.list.retain(|p| p.seen.elapsed() < TIMEOUT);
        hits
    }

    pub fn get(&self, id: u32) -> Option<&Peer> {
        self.list.iter().find(|p| p.id == id)
    }
}
