/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The settings page: a tiny HTTP server on 127.0.0.1 that serves one HTML file
//! and forwards JSON requests to the same handler as the control socket.
//!
//! `GET /` is the page. `POST /api` takes one request object (see control.rs).
//! Only this machine can connect, and requests from other web sites are refused.

use crate::control;
use crate::service::Shared;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
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

/// True when the request names this server as `Host` and, if it carries an
/// `Origin`, comes from our own page. This blocks other web sites (cross-site
/// POSTs and DNS rebinding) from changing settings through the user's browser.
fn is_local(req: &Request, port: u16) -> bool {
    let ours = [format!("127.0.0.1:{port}"), format!("localhost:{port}")];
    ours.contains(&req.host)
        && req.origin.as_ref().map_or(true, |o| ours.iter().any(|h| *o == format!("http://{h}")))
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

fn serve(shared: &Shared, stream: TcpStream, port: u16) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let Some(req) = read_request(&stream) else {
        return respond(&stream, "400 Bad Request", "text/plain", b"bad request");
    };
    if !is_local(&req, port) {
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

/// Starts the server on 127.0.0.1. Failing to bind is reported, not fatal.
pub fn start(shared: Arc<Shared>) {
    let ui = shared.config().ui;
    if !ui.enabled {
        return;
    }
    let listener = match TcpListener::bind((Ipv4Addr::LOCALHOST, ui.port)) {
        Ok(l) => l,
        Err(e) => return eprintln!("settings page: cannot listen on 127.0.0.1:{}: {e}", ui.port),
    };
    eprintln!("settings page: http://127.0.0.1:{}/", ui.port);
    let _ = std::thread::Builder::new().name("web".into()).spawn(move || {
        for stream in listener.incoming().flatten() {
            let shared = shared.clone();
            let _ = std::thread::Builder::new()
                .name("web-client".into())
                .spawn(move || serve(&shared, stream, ui.port));
        }
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
    fn accepts_own_page_and_plain_clients() {
        assert!(is_local(&req("127.0.0.1:5757", None), 5757));
        assert!(is_local(&req("localhost:5757", Some("http://localhost:5757")), 5757));
        assert!(is_local(&req("127.0.0.1:5757", Some("http://127.0.0.1:5757")), 5757));
    }

    #[test]
    fn refuses_other_sites_and_rebinding() {
        assert!(!is_local(&req("127.0.0.1:5757", Some("https://evil.example")), 5757));
        assert!(!is_local(&req("evil.example:5757", None), 5757));
        assert!(!is_local(&req("127.0.0.1:5757", Some("http://127.0.0.1:9999")), 5757));
        assert!(!is_local(&req("127.0.0.1:9999", None), 5757));
        assert!(!is_local(&req("", None), 5757));
    }
}
