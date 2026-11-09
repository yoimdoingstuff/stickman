//! Lets several stickman.exe processes see, stand on, hit and wake each
//! other.
//!
//! Each process binds one UDP port in a small range on 127.0.0.1 (never the
//! network) and a few times a second sends its position and body outline to
//! every other port in the range. Hits and pokes are sent the same way and
//! the target picks them out.

use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::time::{Duration, Instant};

use crate::math::{v2, Rect, Vec2};

/// Bumped whenever the message layout changes.
const MAGIC: [u8; 4] = *b"STK2";
const BASE_PORT: u16 = 47_810;
const SLOTS: u16 = 16;
/// Often enough that standing on someone's head looks smooth.
const SEND_EVERY: f32 = 1.0 / 30.0;
const TIMEOUT: Duration = Duration::from_millis(1000);

const KIND_STATE: u8 = 1;
const KIND_HIT: u8 = 2;
const KIND_POKE: u8 = 3;

const FLAG_RAGDOLL: u8 = 1;
const FLAG_SLEEPING: u8 = 2;

#[derive(Clone, Copy, Debug)]
pub struct Peer {
    pub id: u32,
    pub hip: Vec2,
    pub vel: Vec2,
    pub ragdoll: bool,
    pub sleeping: bool,
    /// Bounding box of its body.
    pub body: Rect,
    seen: Instant,
}

pub enum Incoming {
    Hit { from: u32, impulse: Vec2 },
    Poke { from: u32 },
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

/// What our own body is doing, to share with the others.
pub struct Me {
    pub hip: Vec2,
    pub vel: Vec2,
    pub ragdoll: bool,
    pub sleeping: bool,
    pub body: Rect,
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

    fn header(&self, kind: u8) -> Vec<u8> {
        let mut msg = Vec::with_capacity(48);
        msg.extend_from_slice(&MAGIC);
        msg.push(kind);
        msg.extend_from_slice(&self.id.to_le_bytes());
        msg
    }

    pub fn send_hit(&self, target: u32, impulse: Vec2) {
        let mut msg = self.header(KIND_HIT);
        msg.extend_from_slice(&target.to_le_bytes());
        put_f32(&mut msg, impulse.x);
        put_f32(&mut msg, impulse.y);
        self.broadcast(&msg);
    }

    pub fn send_poke(&self, target: u32) {
        let mut msg = self.header(KIND_POKE);
        msg.extend_from_slice(&target.to_le_bytes());
        self.broadcast(&msg);
    }

    /// Send our state now and then, read everything waiting, return what
    /// was aimed at us.
    pub fn update(&mut self, dt: f32, me: &Me) -> Vec<Incoming> {
        let mut incoming = Vec::new();
        if self.sock.is_none() {
            return incoming;
        }

        self.timer -= dt;
        if self.timer <= 0.0 {
            self.timer = SEND_EVERY;
            let mut msg = self.header(KIND_STATE);
            for v in [me.hip.x, me.hip.y, me.vel.x, me.vel.y, me.body.l, me.body.t, me.body.r, me.body.b] {
                put_f32(&mut msg, v);
            }
            let flags = (if me.ragdoll { FLAG_RAGDOLL } else { 0 }) | (if me.sleeping { FLAG_SLEEPING } else { 0 });
            msg.push(flags);
            self.broadcast(&msg);
        }

        let mut buf = [0u8; 96];
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
                KIND_STATE if n >= 42 => {
                    let f = |i: usize| get_f32(b, 9 + i * 4);
                    let peer = Peer {
                        id: from,
                        hip: v2(f(0), f(1)),
                        vel: v2(f(2), f(3)),
                        body: Rect { l: f(4), t: f(5), r: f(6), b: f(7) },
                        ragdoll: b[41] & FLAG_RAGDOLL != 0,
                        sleeping: b[41] & FLAG_SLEEPING != 0,
                        seen: Instant::now(),
                    };
                    match self.list.iter_mut().find(|p| p.id == from) {
                        Some(p) => *p = peer,
                        None => self.list.push(peer),
                    }
                }
                KIND_HIT if n >= 21 => {
                    if get_u32(b, 9) == self.id {
                        incoming.push(Incoming::Hit { from, impulse: v2(get_f32(b, 13), get_f32(b, 17)) });
                    }
                }
                KIND_POKE if n >= 13 => {
                    if get_u32(b, 9) == self.id {
                        incoming.push(Incoming::Poke { from });
                    }
                }
                _ => {}
            }
        }
        self.list.retain(|p| p.seen.elapsed() < TIMEOUT);
        incoming
    }

    pub fn get(&self, id: u32) -> Option<&Peer> {
        self.list.iter().find(|p| p.id == id)
    }
}
