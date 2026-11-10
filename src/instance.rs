//! One program for all the stickmen. The first stickman.exe becomes the
//! host; running it again just asks the host (over 127.0.0.1, never the
//! network) for one more stickman and exits. All stickmen then share one
//! process: less memory, and they can interact directly.

use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::time::Duration;

const PORT: u16 = 47_809;
const PING: &[u8] = b"STK3?";
const PONG: &[u8] = b"STK3!";
const SPAWN: &[u8] = b"STK3+";

pub enum Start {
    /// We're the host: run, and listen for more stickmen.
    Host(Host),
    /// Another stickman.exe is the host and will add our stickman: exit.
    Forwarded,
    /// Couldn't tell (port used by something else): run on our own.
    Alone,
}

pub struct Host {
    sock: UdpSocket,
}

pub fn start() -> Start {
    let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, PORT);
    if let Ok(sock) = UdpSocket::bind(addr) {
        return match sock.set_nonblocking(true) {
            Ok(()) => Start::Host(Host { sock }),
            Err(_) => Start::Alone,
        };
    }
    // Someone has the port: check it's really a stickman host.
    let Ok(sock) = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)) else { return Start::Alone };
    let _ = sock.set_read_timeout(Some(Duration::from_millis(400)));
    if sock.send_to(PING, addr).is_err() {
        return Start::Alone;
    }
    let mut buf = [0u8; 16];
    match sock.recv_from(&mut buf) {
        Ok((n, _)) if &buf[..n] == PONG => {
            let _ = sock.send_to(SPAWN, addr);
            Start::Forwarded
        }
        _ => Start::Alone,
    }
}

impl Host {
    /// How many new stickmen were asked for since last time.
    pub fn poll(&self) -> u32 {
        let mut spawns = 0;
        let mut buf = [0u8; 16];
        for _ in 0..64 {
            match self.sock.recv_from(&mut buf) {
                Ok((n, from)) => {
                    let msg = &buf[..n];
                    if msg == PING {
                        let _ = self.sock.send_to(PONG, from);
                    } else if msg == SPAWN {
                        spawns += 1;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                // Windows reports "connection reset" after sends to closed ports.
                Err(_) => continue,
            }
        }
        spawns
    }
}
