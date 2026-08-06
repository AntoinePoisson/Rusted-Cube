//! Rusted Cube LAN server.
//!
//! Serves the static files and the WebSocket from one process, so everyone on
//! the network only needs `http://<host>:8080`.
//!
//! Written against `std` alone: one thread per connection, no async runtime.
//! A handful of players on a local network never justifies the dependency
//! tree — and the game itself builds on a compiler those crates no longer
//! support.
//!
//! The server owns the world seed and the list of block edits. Clients generate
//! terrain locally from that seed, so only player poses and edits travel.

// The wire format is shared with the client by including the same source file
// rather than depending on the game crate: that would drag wasm-bindgen and
// web-sys into a build that has no use for them.
#[path = "../../src/protocol.rs"]
// The client uses parts of the protocol the server does not, and vice versa.
#[allow(dead_code)]
mod protocol;
mod sha1;
mod websocket;

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::{Arc, Mutex};

use protocol::{ClientMessage, Edit, PlayerId, Pose, ServerMessage, DEFAULT_PORT};

use websocket::Frame;

/// Refuse absurd request headers rather than buffering them.
const MAX_HEADER_BYTES: usize = 8 * 1024;

struct Client {
    stream: TcpStream,
    pose: Pose,
}

struct World {
    seed: u32,
    /// Keyed by position so repeated edits of one cell collapse.
    edits: HashMap<[i32; 3], u8>,
    clients: HashMap<PlayerId, Client>,
    next_id: PlayerId,
}

impl World {
    /// Sends to every client except `origin`, dropping those that error out.
    fn broadcast(&mut self, origin: PlayerId, message: &ServerMessage) {
        let text = message.encode();
        let mut dead = Vec::new();
        for (id, client) in self.clients.iter_mut() {
            if *id == origin {
                continue;
            }
            if websocket::write_text(&mut client.stream, &text).is_err() {
                dead.push(*id);
            }
        }
        for id in dead {
            self.clients.remove(&id);
        }
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

    let listener = match TcpListener::bind(("0.0.0.0", DEFAULT_PORT)) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("Could not bind port {DEFAULT_PORT}: {error}");
            return;
        }
    };

    println!("Rusted Cube server listening on port {DEFAULT_PORT} (seed {seed})");
    println!("  http://localhost:{DEFAULT_PORT}");
    for address in local_addresses() {
        println!("  http://{address}:{DEFAULT_PORT}   <- share this on your network");
    }

    for incoming in listener.incoming() {
        let Ok(stream) = incoming else { continue };
        let world = Arc::clone(&world);
        std::thread::spawn(move || handle_connection(stream, world));
    }
}

fn handle_connection(mut stream: TcpStream, world: Arc<Mutex<World>>) {
    let Some(request) = read_request(&mut stream) else {
        return;
    };

    let websocket_key = request.headers.iter().find_map(|(name, value)| {
        if name.eq_ignore_ascii_case("sec-websocket-key") {
            Some(value.clone())
        } else {
            None
        }
    });

    match websocket_key {
        Some(key) if request.path == "/ws" => {
            if handshake(&mut stream, &key).is_ok() {
                serve_websocket(stream, world);
            }
        }
        _ => serve_file(&mut stream, &request.path),
    }
}

struct Request {
    path: String,
    headers: Vec<(String, String)>,
}

fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;

    let mut parts = line.split_whitespace();
    let method = parts.next()?;
    let path = parts.next()?.to_owned();
    if method != "GET" {
        return None;
    }

    let mut headers = Vec::new();
    let mut consumed = line.len();
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).ok()? == 0 {
            break;
        }
        consumed += header.len();
        if consumed > MAX_HEADER_BYTES {
            return None;
        }
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

fn handshake(stream: &mut TcpStream, key: &str) -> std::io::Result<()> {
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
    // Register the newcomer and hand it the world under a single lock.
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
        // Existing players would otherwise not learn about this one until it
        // first moved.
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
                match message {
                    ClientMessage::Move { pose } => {
                        if let Some(client) = state.clients.get_mut(&id) {
                            client.pose = pose;
                        }
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
                if let Some(client) = state.clients.get_mut(&id) {
                    let _ = websocket::write_pong(&mut client.stream, &payload);
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

fn serve_file(stream: &mut TcpStream, path: &str) {
    let requested = path.split('?').next().unwrap_or("/").trim_start_matches('/');
    let requested = if requested.is_empty() {
        "index.html"
    } else {
        requested
    };

    // Only ever serve from the working directory.
    if requested.split('/').any(|part| part == ".." || part.is_empty())
        || Path::new(requested).is_absolute()
    {
        let _ = respond(stream, "403 Forbidden", "text/plain", b"Forbidden");
        return;
    }

    match std::fs::File::open(requested) {
        Ok(mut file) => {
            let mut bytes = Vec::new();
            if file.read_to_end(&mut bytes).is_err() {
                let _ = respond(stream, "500 Internal Server Error", "text/plain", b"Error");
                return;
            }
            // Announce multiplayer in the page itself. Letting the client probe
            // for it instead would cost a request whose failure the browser
            // logs as a console error on every static host.
            if requested.ends_with(".html") {
                if let Ok(text) = std::str::from_utf8(&bytes) {
                    let patched = text.replace(r#"data-multiplayer="0""#, r#"data-multiplayer="1""#);
                    let _ = respond(stream, "200 OK", content_type(requested), patched.as_bytes());
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
        // Browsers refuse to stream-compile a module served as anything else.
        Some("wasm") => "application/wasm",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("json") => "application/json",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn respond(
    stream: &mut TcpStream,
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

/// Best-effort LAN address, printed so players know where to point a browser.
fn local_addresses() -> Vec<String> {
    let Ok(output) = std::process::Command::new("ipconfig")
        .arg("getifaddr")
        .arg("en0")
        .output()
    else {
        return Vec::new();
    };
    let address = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if address.is_empty() {
        Vec::new()
    } else {
        vec![address]
    }
}
