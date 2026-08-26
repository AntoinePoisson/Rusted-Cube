//! Rusted Cube LAN server. Static files and WebSocket traffic share one process,
//! with one thread per connection and no external dependencies.
//!
//! Owns the seed and the edit list. Clients generate terrain locally from the
//! seed, so only poses and edits travel.

// Sharing the file keeps wasm-bindgen and web-sys out of the server build.
#[path = "../../src/protocol.rs"]
#[allow(dead_code)]
mod protocol;
mod sha1;
mod websocket;

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use protocol::{ClientMessage, Edit, PlayerId, Pose, ServerMessage, DEFAULT_PORT};

use websocket::Frame;

const MAX_HEADER_BYTES: usize = 8 * 1024;

struct Client {
    stream: TcpStream,
    pose: Pose,
}

struct World {
    seed: u32,
    edits: HashMap<[i32; 3], u8>,
    clients: HashMap<PlayerId, Client>,
    next_id: PlayerId,
}

impl World {
    fn broadcast(&mut self, origin: PlayerId, message: &ServerMessage) {
        let mut departed = self.send_to_others(origin, &message.encode());
        while let Some(id) = departed.pop() {
            departed.extend(self.send_to_others(id, &ServerMessage::PlayerLeft { id }.encode()));
        }
    }

    fn send_to_others(&mut self, origin: PlayerId, text: &str) -> Vec<PlayerId> {
        let mut dead = Vec::new();
        for (id, client) in self.clients.iter_mut() {
            if *id == origin {
                continue;
            }
            if websocket::write_text(&mut client.stream, text).is_err() {
                dead.push(*id);
            }
        }
        for id in &dead {
            self.clients.remove(id);
        }
        dead
    }
}

fn main() {
    let seed = std::env::args()
        .nth(1)
        .and_then(|argument| argument.parse().ok())
        .unwrap_or(1_337_u32);

    let world = Arc::new(Mutex::new(World {
        seed,
        edits: HashMap::new(),
        clients: HashMap::new(),
        next_id: 1,
    }));
    let site_root = Arc::new(site_root());

    let listener = match TcpListener::bind(("0.0.0.0", DEFAULT_PORT)) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("Could not bind port {DEFAULT_PORT}: {error}");
            return;
        }
    };

    println!("Rusted Cube server listening on port {DEFAULT_PORT} (seed {seed})");
    println!("  http://localhost:{DEFAULT_PORT}");
    if let Some(address) = local_address() {
        println!("  http://{address}:{DEFAULT_PORT}   <- share this on your network");
    }

    for incoming in listener.incoming() {
        let Ok(stream) = incoming else { continue };
        let world = Arc::clone(&world);
        let site_root = Arc::clone(&site_root);
        std::thread::spawn(move || handle_connection(stream, world, site_root));
    }
}

fn handle_connection(mut stream: TcpStream, world: Arc<Mutex<World>>, site_root: Arc<PathBuf>) {
    let Some(request) = read_request(&mut stream) else {
        return;
    };

    match request.websocket_key() {
        Some(key) if request.path == "/ws" => {
            if handshake(&mut stream, key).is_ok() {
                serve_websocket(stream, world);
            }
        }
        _ => serve_file(&mut stream, &request.path, site_root.as_path()),
    }
}

struct Request {
    path: String,
    headers: Vec<(String, String)>,
}

impl Request {
    fn header(&self, wanted: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(wanted))
            .map(|(_, value)| value.as_str())
    }

    fn websocket_key(&self) -> Option<&str> {
        let upgrade = self.header("upgrade")?;
        let connection = self.header("connection")?;
        let version = self.header("sec-websocket-version")?;
        let key = self.header("sec-websocket-key")?;
        let connection_upgrades = connection
            .split(',')
            .any(|token| token.trim().eq_ignore_ascii_case("upgrade"));

        (upgrade.eq_ignore_ascii_case("websocket") && connection_upgrades && version == "13")
            .then_some(key)
    }
}

fn read_request(stream: &mut impl Read) -> Option<Request> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    let mut consumed = read_bounded_line(&mut reader, &mut line, MAX_HEADER_BYTES)?;
    if consumed == 0 {
        return None;
    }

    let mut parts = line.split_whitespace();
    let method = parts.next()?;
    let path = parts.next()?.to_owned();
    let version = parts.next()?;
    if method != "GET" || !version.starts_with("HTTP/") || parts.next().is_some() {
        return None;
    }

    let mut headers = Vec::new();
    loop {
        let mut header = String::new();
        let remaining = MAX_HEADER_BYTES.checked_sub(consumed)?;
        let read = read_bounded_line(&mut reader, &mut header, remaining)?;
        if read == 0 {
            break;
        }
        consumed += read;
        let trimmed = header.trim_end();
        if trimmed.is_empty() {
            break;
        }
        if let Some(separator) = trimmed.find(':') {
            headers.push((
                trimmed[..separator].trim().to_owned(),
                trimmed[separator + 1..].trim().to_owned(),
            ));
        }
    }

    Some(Request { path, headers })
}

fn read_bounded_line(
    reader: &mut impl BufRead,
    line: &mut String,
    remaining: usize,
) -> Option<usize> {
    let read = reader
        .take((remaining.saturating_add(1)) as u64)
        .read_line(line)
        .ok()?;
    (read <= remaining).then_some(read)
}

fn handshake(stream: &mut impl Write, key: &str) -> std::io::Result<()> {
    let response = format!(
        "HTTP/1.1 101 Switching Protocols\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Accept: {}\r\n\r\n",
        websocket::accept_key(key)
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()
}

fn serve_websocket(stream: TcpStream, world: Arc<Mutex<World>>) {
    let id = {
        let mut state = world.lock().unwrap();
        let id = state.next_id;
        state.next_id += 1;

        let welcome = ServerMessage::Welcome {
            id,
            seed: state.seed,
            edits: state
                .edits
                .iter()
                .map(|(position, block)| Edit {
                    position: *position,
                    block: *block,
                })
                .collect(),
            players: state
                .clients
                .iter()
                .map(|(id, client)| (*id, client.pose))
                .collect(),
        };

        let Ok(mut writer) = stream.try_clone() else {
            return;
        };
        if websocket::write_text(&mut writer, &welcome.encode()).is_err() {
            return;
        }

        let pose = Pose {
            position: [0.0, 0.0, 0.0],
            yaw: 0.0,
            pitch: 0.0,
        };
        state.clients.insert(
            id,
            Client {
                stream: writer,
                pose,
            },
        );
        state.broadcast(id, &ServerMessage::PlayerJoined { id, pose });
        println!("Player {id} joined ({} online)", state.clients.len());
        id
    };

    let mut reader = stream;
    loop {
        let Some(frame) = websocket::read_frame(&mut reader) else {
            break;
        };
        match frame {
            Frame::Text(text) => {
                let Some(message) = ClientMessage::decode(&text) else {
                    continue;
                };
                let mut state = world.lock().unwrap();
                if !state.clients.contains_key(&id) {
                    break;
                }
                match message {
                    ClientMessage::Move { pose } => {
                        state.clients.get_mut(&id).expect("registered client").pose = pose;
                        state.broadcast(id, &ServerMessage::PlayerMoved { id, pose });
                    }
                    ClientMessage::SetBlock { edit } => {
                        state.edits.insert(edit.position, edit.block);
                        state.broadcast(id, &ServerMessage::BlockChanged { edit });
                    }
                }
            }
            Frame::Ping(payload) => {
                let mut state = world.lock().unwrap();
                let Some(client) = state.clients.get_mut(&id) else {
                    break;
                };
                if websocket::write_pong(&mut client.stream, &payload).is_err() {
                    break;
                }
            }
            Frame::Close => break,
            Frame::Other => {}
        }
    }

    let mut state = world.lock().unwrap();
    if let Some(mut client) = state.clients.remove(&id) {
        let _ = websocket::write_close(&mut client.stream);
    }
    state.broadcast(id, &ServerMessage::PlayerLeft { id });
    println!("Player {id} left ({} online)", state.clients.len());
}

fn serve_file(stream: &mut impl Write, path: &str, site_root: &Path) {
    let requested = path
        .split('?')
        .next()
        .unwrap_or("/")
        .trim_start_matches('/');
    let requested = if requested.is_empty() {
        "index.html"
    } else {
        requested
    };

    if requested
        .split('/')
        .any(|part| part == ".." || part.is_empty())
        || Path::new(requested).is_absolute()
    {
        let _ = respond(stream, "403 Forbidden", "text/plain", b"Forbidden");
        return;
    }

    match std::fs::File::open(site_root.join(requested)) {
        Ok(mut file) => {
            let mut bytes = Vec::new();
            if file.read_to_end(&mut bytes).is_err() {
                let _ = respond(stream, "500 Internal Server Error", "text/plain", b"Error");
                return;
            }
            if requested.ends_with(".html") {
                if let Ok(text) = std::str::from_utf8(&bytes) {
                    let patched =
                        text.replace(r#"data-multiplayer="0""#, r#"data-multiplayer="1""#);
                    let _ = respond(
                        stream,
                        "200 OK",
                        content_type(requested),
                        patched.as_bytes(),
                    );
                    return;
                }
            }
            let _ = respond(stream, "200 OK", content_type(requested), &bytes);
        }
        Err(_) => {
            let _ = respond(stream, "404 Not Found", "text/plain", b"Not found");
        }
    }
}

fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("wasm") => "application/wasm",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("json") => "application/json",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn respond(
    stream: &mut impl Write,
    status: &str,
    content_type: &str,
    body: &[u8],
) -> std::io::Result<()> {
    let header = format!(
        "HTTP/1.1 {status}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-cache\r\n\
         Connection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

fn site_root() -> PathBuf {
    let current = std::env::current_dir().unwrap_or_default();
    if current.join("index.html").is_file() {
        return current;
    }
    if let Some(parent) = current.parent() {
        if parent.join("index.html").is_file() {
            return parent.to_path_buf();
        }
    }

    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")))
        .to_path_buf()
}

fn local_address() -> Option<std::net::IpAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    let address = socket.local_addr().ok()?.ip();
    (!address.is_loopback() && !address.is_unspecified()).then_some(address)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn parses_a_websocket_upgrade() {
        let request = b"GET /ws HTTP/1.1\r\n\
            Host: localhost\r\n\
            Upgrade: websocket\r\n\
            Connection: keep-alive, Upgrade\r\n\
            Sec-WebSocket-Version: 13\r\n\
            Sec-WebSocket-Key: test-key\r\n\r\n";
        let mut input = Cursor::new(request);
        let request = read_request(&mut input).expect("valid request");

        assert_eq!(request.path, "/ws");
        assert_eq!(request.websocket_key(), Some("test-key"));
    }

    #[test]
    fn rejects_oversized_headers() {
        let request = format!(
            "GET / HTTP/1.1\r\nX-Fill: {}\r\n\r\n",
            "x".repeat(MAX_HEADER_BYTES)
        );
        assert!(read_request(&mut Cursor::new(request)).is_none());
    }

    #[test]
    fn finds_the_site_when_started_from_the_server_directory() {
        assert!(site_root().join("index.html").is_file());
    }

    #[test]
    fn serves_the_home_page_with_multiplayer_enabled() {
        let mut response = Vec::new();
        serve_file(&mut response, "/", &site_root());
        let response = String::from_utf8(response).expect("text response");

        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(response.contains(r#"data-multiplayer="1""#));
    }
}
