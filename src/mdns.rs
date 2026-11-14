/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! A minimal mDNS responder, so the settings page has a name on the local network
//! (`http://dsconfig-<hostname>.local:5757/`) without Avahi or any setup.
//!
//! It answers address queries for exactly one name and nothing else. It shares UDP
//! port 5353 with any other responder on the machine (Avahi, systemd-resolved).

use crate::service::Shared;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::os::fd::{AsRawFd, FromRawFd};
use std::sync::Arc;
use std::time::Duration;

const GROUP: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
const PORT: u16 = 5353;
const TYPE_A: u16 = 1;
const TYPE_AAAA: u16 = 28;
const TYPE_NSEC: u16 = 47;
const TYPE_ANY: u16 = 255;
const TTL: u32 = 120;

/// One question from a query packet.
#[derive(Debug, PartialEq)]
struct Question {
    name: String,
    qtype: u16,
    /// The asker wants the reply sent straight back instead of to the group.
    unicast: bool,
}

/// Questions of a query packet; empty for responses and anything malformed.
fn parse_query(packet: &[u8]) -> Vec<Question> {
    let mut out = Vec::new();
    if packet.len() < 12 || packet[2] & 0x80 != 0 {
        return out;
    }
    let count = u16::from_be_bytes([packet[4], packet[5]]);
    let mut i = 12;
    for _ in 0..count.min(16) {
        let mut labels = Vec::new();
        loop {
            let Some(&len) = packet.get(i) else { return out };
            i += 1;
            if len == 0 {
                break;
            }
            // Compression pointers do not occur in the first question; give up on others.
            let Some(label) = (len < 64).then(|| packet.get(i..i + len as usize)).flatten() else {
                return out;
            };
            labels.push(String::from_utf8_lossy(label).to_ascii_lowercase());
            i += len as usize;
        }
        let Some(tail) = packet.get(i..i + 4) else { return out };
        i += 4;
        out.push(Question {
            name: labels.join("."),
            qtype: u16::from_be_bytes([tail[0], tail[1]]),
            unicast: tail[2] & 0x80 != 0,
        });
    }
    out
}

fn push_name(out: &mut Vec<u8>, name: &str) {
    for label in name.split('.') {
        out.push(label.len() as u8);
        out.extend_from_slice(label.as_bytes());
    }
    out.push(0);
}

/// A response carrying our address, plus a record saying the name has no other
/// record types (so clients do not wait for an IPv6 answer that will never come).
/// `legacy` is `Some((id, qtype))` for plain DNS clients that query from another port.
fn build_response(name: &str, ip: Ipv4Addr, legacy: Option<(u16, u16)>) -> Vec<u8> {
    let mut p = Vec::with_capacity(128);
    p.extend_from_slice(&legacy.map_or(0, |l| l.0).to_be_bytes());
    p.extend_from_slice(&[0x84, 0x00]); // response, authoritative
    p.extend_from_slice(&(legacy.is_some() as u16).to_be_bytes()); // questions
    p.extend_from_slice(&[0, 1, 0, 0, 0, 1]); // 1 answer, 0 authority, 1 additional
    if let Some((_, qtype)) = legacy {
        push_name(&mut p, name);
        p.extend_from_slice(&qtype.to_be_bytes());
        p.extend_from_slice(&[0, 1]);
    }
    // The cache-flush bit tells mDNS clients this is the only holder of the name.
    let class: [u8; 2] = if legacy.is_some() { [0, 1] } else { [0x80, 1] };
    push_name(&mut p, name);
    p.extend_from_slice(&TYPE_A.to_be_bytes());
    p.extend_from_slice(&class);
    p.extend_from_slice(&TTL.to_be_bytes());
    p.extend_from_slice(&[0, 4]);
    p.extend_from_slice(&ip.octets());

    let mut rdata = Vec::new();
    push_name(&mut rdata, name);
    rdata.extend_from_slice(&[0, 1, 0x40]); // window 0, one byte, bit 1 = type A
    push_name(&mut p, name);
    p.extend_from_slice(&TYPE_NSEC.to_be_bytes());
    p.extend_from_slice(&class);
    p.extend_from_slice(&TTL.to_be_bytes());
    p.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
    p.extend_from_slice(&rdata);
    p
}

fn setsockopt<T>(socket: &UdpSocket, level: i32, option: i32, value: &T) -> bool {
    // SAFETY: `value` points to a live T of the size passed.
    unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            level,
            option,
            value as *const T as *const libc::c_void,
            std::mem::size_of::<T>() as libc::socklen_t,
        ) == 0
    }
}

/// UDP socket on port 5353 that other responders on this machine can share.
fn open() -> std::io::Result<UdpSocket> {
    // SAFETY: plain socket creation; the fd is owned by the UdpSocket from here on.
    let socket = unsafe {
        let fd = libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0);
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        UdpSocket::from_raw_fd(fd)
    };
    let one: libc::c_int = 1;
    setsockopt(&socket, libc::SOL_SOCKET, libc::SO_REUSEADDR, &one);
    setsockopt(&socket, libc::SOL_SOCKET, libc::SO_REUSEPORT, &one);
    let address = libc::sockaddr_in {
        sin_family: libc::AF_INET as libc::sa_family_t,
        sin_port: PORT.to_be(),
        sin_addr: libc::in_addr { s_addr: 0 },
        sin_zero: [0; 8],
    };
    // SAFETY: `address` is a valid sockaddr_in of the size passed.
    let bound = unsafe {
        libc::bind(
            socket.as_raw_fd(),
            &address as *const _ as *const libc::sockaddr,
            std::mem::size_of_val(&address) as libc::socklen_t,
        )
    };
    if bound != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(socket)
}

/// This machine's private IPv4 addresses, one per network interface.
fn local_addresses() -> Vec<Ipv4Addr> {
    let mut found = Vec::new();
    let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs fills a linked list that we walk read-only and then free.
    unsafe {
        if libc::getifaddrs(&mut list) != 0 {
            return found;
        }
        let mut cursor = list;
        while !cursor.is_null() {
            let entry = &*cursor;
            if !entry.ifa_addr.is_null() && (*entry.ifa_addr).sa_family as i32 == libc::AF_INET {
                let sin = &*(entry.ifa_addr as *const libc::sockaddr_in);
                let ip = Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr));
                if ip.is_private() && !found.contains(&ip) {
                    found.push(ip);
                }
            }
            cursor = entry.ifa_next;
        }
        libc::freeifaddrs(list);
    }
    found
}

/// Listen for the group on every interface. Repeated calls pick up interfaces
/// that appeared later; joining twice is a harmless error.
fn join_all(socket: &UdpSocket) {
    for ip in local_addresses() {
        let request = libc::ip_mreq {
            imr_multiaddr: libc::in_addr { s_addr: u32::from(GROUP).to_be() },
            imr_interface: libc::in_addr { s_addr: u32::from(ip).to_be() },
        };
        setsockopt(socket, libc::IPPROTO_IP, libc::IP_ADD_MEMBERSHIP, &request);
    }
}

/// Our address as seen from `peer`: the kernel's routing picks the right interface.
fn address_towards(peer: Ipv4Addr) -> Option<Ipv4Addr> {
    let probe = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    probe.connect((peer, 9)).ok()?;
    match probe.local_addr().ok()? {
        SocketAddr::V4(a) if a.ip().is_private() => Some(*a.ip()),
        _ => None,
    }
}

/// Answers until LAN mode is switched off.
fn respond_loop(shared: &Shared, socket: &UdpSocket, name: &str) {
    let _ = socket.set_read_timeout(Some(Duration::from_secs(2)));
    let mut buffer = [0u8; 1500];
    let mut ticks = 0u32;
    loop {
        if !shared.config.lock().unwrap().ui.lan {
            return;
        }
        if ticks % 15 == 0 {
            join_all(socket);
        }
        ticks += 1;
        let Ok((len, SocketAddr::V4(from))) = socket.recv_from(&mut buffer) else { continue };
        if !(from.ip().is_private() || from.ip().is_link_local()) {
            continue;
        }
        let Some(question) = parse_query(&buffer[..len])
            .into_iter()
            .find(|q| q.name == name && matches!(q.qtype, TYPE_A | TYPE_AAAA | TYPE_ANY))
        else {
            continue;
        };
        let Some(ours) = address_towards(*from.ip()) else { continue };
        // Send from the interface the question came in on.
        let interface = libc::in_addr { s_addr: u32::from(ours).to_be() };
        setsockopt(socket, libc::IPPROTO_IP, libc::IP_MULTICAST_IF, &interface);
        if from.port() != PORT {
            // A plain DNS client: echo its id and question, reply to it directly.
            let id = u16::from_be_bytes([buffer[0], buffer[1]]);
            let _ = socket.send_to(&build_response(name, ours, Some((id, question.qtype))), from);
        } else if question.unicast {
            let _ = socket.send_to(&build_response(name, ours, None), from);
        } else {
            let _ = socket.send_to(&build_response(name, ours, None), SocketAddrV4::new(GROUP, PORT));
        }
    }
}

/// Runs the responder whenever LAN mode is on. Never fatal.
pub fn start(shared: Arc<Shared>) {
    let ui = shared.config().ui;
    if !ui.enabled {
        return;
    }
    let name = ui.mdns_host();
    let _ = std::thread::Builder::new().name("mdns".into()).spawn(move || loop {
        if shared.config.lock().unwrap().ui.lan {
            match open() {
                Ok(socket) => {
                    eprintln!("settings page: also http://{name}:{}/ on the local network", ui.port);
                    respond_loop(&shared, &socket, &name);
                }
                Err(e) => {
                    eprintln!("settings page: no .local name (mDNS port unavailable: {e})");
                    std::thread::sleep(Duration::from_secs(30));
                }
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(name: &str, qtype: u16, class: u16) -> Vec<u8> {
        let mut p = vec![0x12, 0x34, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
        push_name(&mut p, name);
        p.extend_from_slice(&qtype.to_be_bytes());
        p.extend_from_slice(&class.to_be_bytes());
        p
    }

    #[test]
    fn parses_questions() {
        let q = parse_query(&query("DSconfig.Local", TYPE_A, 0x8001));
        assert_eq!(q, vec![Question { name: "dsconfig.local".into(), qtype: TYPE_A, unicast: true }]);
        let q = parse_query(&query("other.local", TYPE_AAAA, 1));
        assert_eq!(q[0].qtype, TYPE_AAAA);
        assert!(!q[0].unicast);
    }

    #[test]
    fn ignores_responses_and_garbage() {
        let mut response = query("dsconfig.local", TYPE_A, 1);
        response[2] = 0x84;
        assert!(parse_query(&response).is_empty());
        assert!(parse_query(&[0; 5]).is_empty());
        assert!(parse_query(&[0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 63, b'a']).is_empty());
        // A compression pointer where a label should be.
        assert!(parse_query(&[0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0xC0, 0x0C, 0, 1, 0, 1]).is_empty());
    }

    #[test]
    fn response_carries_the_address() {
        let ip = Ipv4Addr::new(192, 168, 1, 47);
        let r = build_response("dsconfig.local", ip, None);
        assert_eq!(&r[..12], &[0, 0, 0x84, 0, 0, 0, 0, 1, 0, 0, 0, 1]);
        // name(16) type(2) class(2) ttl(4) rdlength(2) then the address
        assert_eq!(&r[12..28], b"\x08dsconfig\x05local\x00");
        assert_eq!(&r[28..32], &[0, 1, 0x80, 1]);
        assert_eq!(&r[38..42], &ip.octets());

        let legacy = build_response("dsconfig.local", ip, Some((0x1234, TYPE_A)));
        assert_eq!(&legacy[..6], &[0x12, 0x34, 0x84, 0, 0, 1]);
        // The echoed question comes first, and the cache-flush bit is not set.
        assert_eq!(&legacy[28..32], &[0, 1, 0, 1]);
        assert_eq!(&legacy[48..52], &[0, 1, 0, 1]);
    }
}
