/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The settings page: a tiny HTTP server that serves one HTML file and forwards
//! JSON requests to the same handler as the control socket.
//!
//! `GET /` is the page. `POST /api` takes one request object (see control.rs).
//!
//! By default only this machine can connect. With `[ui] lan = true` other devices
//! on the local network can too; there is no password, so the checks below are
//! what keeps the wider internet and other web sites out.

use crate::control;
use crate::service::Shared;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::Arc;
use std::time::Duration;

const PAGE: &str = include_str!("ui.html");
const MAX_BODY: usize = 64 * 1024;

struct Request {
    method: String,
    path: String,
    host: String,
    origin: Option<String>,
    body: Vec<u8>,
}

fn read_request(stream: &TcpStream) -> Option<Request> {
    let mut reader = BufReader::new(stream.take(MAX_BODY as u64 + 8192));
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let (method, path) = (parts.next()?.to_string(), parts.next()?.to_string());
    let (mut host, mut origin, mut length) = (String::new(), None, 0usize);
    loop {
        line.clear();
        if reader.read_line(&mut line).ok()? == 0 || line.trim().is_empty() {
            break;
        }
        let (name, value) = line.split_once(':')?;
        let value = value.trim();
        match name.to_ascii_lowercase().as_str() {
            "host" => host = value.to_string(),
            "origin" => origin = Some(value.to_string()),
            "content-length" => length = value.parse().ok()?,
            _ => {}
        }
    }
    if length > MAX_BODY {
        return None;
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).ok()?;
    Some(Request { method, path, host, origin, body })
}

/// True when the request addresses this server by `localhost` or, in LAN mode, by
/// an IP address, and any `Origin` is that same address. A web site elsewhere can
/// only reach us through the user's browser under its own name (cross-site request
/// or DNS rebinding), and a name is never accepted.
fn is_local(req: &Request, port: u16, lan: bool) -> bool {
    let Some((name, p)) = req.host.rsplit_once(':') else { return false };
    if p.parse() != Ok(port) {
        return false;
    }
    let known = name == "localhost"
        || name.parse::<Ipv4Addr>().is_ok_and(|ip| ip.is_loopback() || (lan && ip.is_private()));
    known && req.origin.as_ref().map_or(true, |o| *o == format!("http://{}", req.host))
}

/// Peers we answer: this machine, and in LAN mode private and link-local addresses.
fn peer_allowed(peer: SocketAddr, lan: bool) -> bool {
    match peer.ip() {
        IpAddr::V4(ip) => ip.is_loopback() || (lan && (ip.is_private() || ip.is_link_local())),
        IpAddr::V6(ip) => ip.is_loopback(),
    }
}

/// This machine's address on the local network, if it has one. Connecting a UDP
/// socket sends nothing; it only makes the kernel pick the outgoing interface.
pub fn lan_address() -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect((Ipv4Addr::new(192, 0, 2, 1), 9)).ok()?;
    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(ip) if ip.is_private() => Some(ip),
        _ => None,
    }
}

fn respond(mut stream: &TcpStream, status: &str, kind: &str, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
}

fn serve(shared: &Shared, stream: TcpStream, port: u16, lan: bool) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    if !stream.peer_addr().is_ok_and(|p| peer_allowed(p, lan)) {
        return respond(&stream, "403 Forbidden", "text/plain", b"forbidden");
    }
    let Some(req) = read_request(&stream) else {
        return respond(&stream, "400 Bad Request", "text/plain", b"bad request");
    };
    if !is_local(&req, port, lan) {
        return respond(&stream, "403 Forbidden", "text/plain", b"forbidden");
    }
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/") => respond(&stream, "200 OK", "text/html; charset=utf-8", PAGE.as_bytes()),
        ("POST", "/api") => {
            let reply = match serde_json::from_slice::<Value>(&req.body) {
                Ok(v) => control::handle_value(shared, &v),
                Err(e) => json!({"ok": false, "error": format!("bad JSON: {e}")}),
            };
            respond(&stream, "200 OK", "application/json", reply.to_string().as_bytes());
        }
        _ => respond(&stream, "404 Not Found", "text/plain", b"not found"),
    }
}

/// Serves until the LAN setting changes, then returns so the caller can rebind.
fn listen(shared: &Arc<Shared>, port: u16, lan: bool) {
    let address = if lan { Ipv4Addr::UNSPECIFIED } else { Ipv4Addr::LOCALHOST };
    let listener = match TcpListener::bind((address, port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("settings page: cannot listen on {address}:{port}: {e}");
            std::thread::sleep(Duration::from_secs(5));
            return;
        }
    };
    match lan.then(lan_address).flatten() {
        Some(ip) => eprintln!("settings page: http://127.0.0.1:{port}/ and, on the local network, http://{ip}:{port}/"),
        None => eprintln!("settings page: http://127.0.0.1:{port}/"),
    }
    // Non-blocking accept so a change of the setting is noticed.
    let _ = listener.set_nonblocking(true);
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                let shared = shared.clone();
                let _ = std::thread::Builder::new()
                    .name("web-client".into())
                    .spawn(move || serve(&shared, stream, port, lan));
            }
            Err(_) => {
                if shared.config.lock().unwrap().ui.lan != lan {
                    return;
                }
                std::thread::sleep(Duration::from_millis(40));
            }
        }
    }
}

/// Starts the server. Failing to bind is reported, not fatal.
pub fn start(shared: Arc<Shared>) {
    let ui = shared.config().ui;
    if !ui.enabled {
        return;
    }
    let _ = std::thread::Builder::new().name("web".into()).spawn(move || loop {
        let lan = shared.config.lock().unwrap().ui.lan;
        listen(&shared, ui.port, lan);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(host: &str, origin: Option<&str>) -> Request {
        Request {
            method: "POST".into(),
            path: "/api".into(),
            host: host.into(),
            origin: origin.map(String::from),
            body: Vec::new(),
        }
    }

    #[test]
    fn local_mode_accepts_only_this_machine() {
        assert!(is_local(&req("127.0.0.1:5757", None), 5757, false));
        assert!(is_local(&req("localhost:5757", Some("http://localhost:5757")), 5757, false));
        assert!(!is_local(&req("192.168.1.20:5757", None), 5757, false));
        assert!(!peer_allowed("192.168.1.30:4000".parse().unwrap(), false));
        assert!(peer_allowed("127.0.0.1:4000".parse().unwrap(), false));
    }

    #[test]
    fn lan_mode_accepts_private_addresses_only() {
        assert!(is_local(&req("192.168.1.20:5757", None), 5757, true));
        assert!(is_local(&req("10.0.0.5:5757", Some("http://10.0.0.5:5757")), 5757, true));
        assert!(is_local(&req("127.0.0.1:5757", None), 5757, true));
        assert!(!is_local(&req("8.8.8.8:5757", None), 5757, true));
        assert!(peer_allowed("192.168.1.30:4000".parse().unwrap(), true));
        assert!(peer_allowed("10.1.2.3:4000".parse().unwrap(), true));
        assert!(!peer_allowed("8.8.8.8:4000".parse().unwrap(), true));
        assert!(!peer_allowed("[2001:db8::1]:4000".parse().unwrap(), true));
    }

    #[test]
    fn refuses_other_sites_and_rebinding_in_both_modes() {
        for lan in [false, true] {
            // Cross-site request from a page on another origin.
            assert!(!is_local(&req("127.0.0.1:5757", Some("https://evil.example")), 5757, lan));
            assert!(!is_local(&req("192.168.1.20:5757", Some("https://evil.example")), 5757, lan));
            // DNS rebinding: the browser sends the attacker's host name.
            assert!(!is_local(&req("evil.example:5757", None), 5757, lan));
            assert!(!is_local(&req("evil.example:5757", Some("http://evil.example:5757")), 5757, lan));
            assert!(!is_local(&req("127.0.0.1:9999", None), 5757, lan));
            assert!(!is_local(&req("127.0.0.1:5757", Some("http://127.0.0.1:9999")), 5757, lan));
            assert!(!is_local(&req("", None), 5757, lan));
        }
    }
}
