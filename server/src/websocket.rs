//! The slice of RFC 6455 this server needs: the opening handshake and text
//! frames. No extensions, no fragmentation, no binary payloads.

use std::io::{Read, Write};
use std::net::TcpStream;

use crate::sha1;

/// Fixed GUID the handshake concatenates with the client key (RFC 6455 §1.3).
const HANDSHAKE_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// Refuse oversized frames rather than allocating whatever a client claims.
const MAX_FRAME: u64 = 1 << 20;

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64(input: &[u8]) -> String {
    let mut out = String::new();
    for group in input.chunks(3) {
        let b0 = group[0] as u32;
        let b1 = *group.get(1).unwrap_or(&0) as u32;
        let b2 = *group.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;

        out.push(ALPHABET[(triple >> 18) as usize & 63] as char);
        out.push(ALPHABET[(triple >> 12) as usize & 63] as char);
        out.push(if group.len() > 1 {
            ALPHABET[(triple >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if group.len() > 2 {
            ALPHABET[triple as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// The value a client's `Sec-WebSocket-Key` must be answered with.
pub fn accept_key(client_key: &str) -> String {
    base64(&sha1::digest(format!("{client_key}{HANDSHAKE_GUID}").as_bytes()))
}

pub enum Frame {
    Text(String),
    Close,
    Ping(Vec<u8>),
    /// Something we do not handle; the caller just carries on.
    Other,
}

/// Reads one frame. Returns `None` when the peer is gone or misbehaving.
pub fn read_frame(stream: &mut TcpStream) -> Option<Frame> {
    let mut header = [0_u8; 2];
    stream.read_exact(&mut header).ok()?;

    let opcode = header[0] & 0x0F;
    let masked = header[1] & 0x80 != 0;
    let mut length = (header[1] & 0x7F) as u64;

    if length == 126 {
        let mut extended = [0_u8; 2];
        stream.read_exact(&mut extended).ok()?;
        length = u16::from_be_bytes(extended) as u64;
    } else if length == 127 {
        let mut extended = [0_u8; 8];
        stream.read_exact(&mut extended).ok()?;
        length = u64::from_be_bytes(extended);
    }

    if length > MAX_FRAME {
        return None;
    }

    // Clients must mask; an unmasked client frame is a protocol error.
    let mut mask = [0_u8; 4];
    if masked {
        stream.read_exact(&mut mask).ok()?;
    } else if opcode != 0x8 {
        return None;
    }

    let mut payload = vec![0_u8; length as usize];
    stream.read_exact(&mut payload).ok()?;
    if masked {
        for (index, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[index % 4];
        }
    }

    match opcode {
        0x1 => String::from_utf8(payload).ok().map(Frame::Text),
        0x8 => Some(Frame::Close),
        0x9 => Some(Frame::Ping(payload)),
        _ => Some(Frame::Other),
    }
}

fn write_frame(stream: &mut TcpStream, opcode: u8, payload: &[u8]) -> std::io::Result<()> {
    let mut frame = vec![0x80 | opcode];
    // Server frames are never masked.
    match payload.len() {
        length if length < 126 => frame.push(length as u8),
        length if length <= u16::MAX as usize => {
            frame.push(126);
            frame.extend_from_slice(&(length as u16).to_be_bytes());
        }
        length => {
            frame.push(127);
            frame.extend_from_slice(&(length as u64).to_be_bytes());
        }
    }
    frame.extend_from_slice(payload);
    stream.write_all(&frame)?;
    stream.flush()
}

pub fn write_text(stream: &mut TcpStream, text: &str) -> std::io::Result<()> {
    write_frame(stream, 0x1, text.as_bytes())
}

pub fn write_pong(stream: &mut TcpStream, payload: &[u8]) -> std::io::Result<()> {
    write_frame(stream, 0xA, payload)
}

pub fn write_close(stream: &mut TcpStream) -> std::io::Result<()> {
    write_frame(stream, 0x8, &[])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_known_values() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    /// The handshake example given in RFC 6455 section 1.3.
    #[test]
    fn accept_key_matches_the_rfc_example() {
        assert_eq!(
            accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }
}
