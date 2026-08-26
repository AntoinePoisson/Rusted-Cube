//! Minimal RFC 6455 handshake and frame codec. Extensions and fragmented
//! messages are intentionally unsupported.

use std::io::{Read, Write};

use crate::sha1;

const HANDSHAKE_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

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

pub fn accept_key(client_key: &str) -> String {
    base64(&sha1::digest(
        format!("{client_key}{HANDSHAKE_GUID}").as_bytes(),
    ))
}

pub enum Frame {
    Text(String),
    Close,
    Ping(Vec<u8>),
    Other,
}

pub fn read_frame(stream: &mut impl Read) -> Option<Frame> {
    let mut header = [0_u8; 2];
    stream.read_exact(&mut header).ok()?;

    let finished = header[0] & 0x80 != 0;
    let reserved = header[0] & 0x70;
    let opcode = header[0] & 0x0F;
    let masked = header[1] & 0x80 != 0;
    let mut length = (header[1] & 0x7F) as u64;

    if !finished || reserved != 0 || !masked || !matches!(opcode, 0x1 | 0x2 | 0x8 | 0x9 | 0xA) {
        return None;
    }

    if length == 126 {
        let mut extended = [0_u8; 2];
        stream.read_exact(&mut extended).ok()?;
        length = u16::from_be_bytes(extended) as u64;
    } else if length == 127 {
        let mut extended = [0_u8; 8];
        stream.read_exact(&mut extended).ok()?;
        length = u64::from_be_bytes(extended);
    }

    let control_frame = opcode & 0x08 != 0;
    if length > MAX_FRAME || (control_frame && length > 125) || (opcode == 0x8 && length == 1) {
        return None;
    }

    let mut mask = [0_u8; 4];
    stream.read_exact(&mut mask).ok()?;

    let mut payload = vec![0_u8; length as usize];
    stream.read_exact(&mut payload).ok()?;
    for (index, byte) in payload.iter_mut().enumerate() {
        *byte ^= mask[index % 4];
    }

    match opcode {
        0x1 => String::from_utf8(payload).ok().map(Frame::Text),
        0x8 => Some(Frame::Close),
        0x9 => Some(Frame::Ping(payload)),
        _ => Some(Frame::Other),
    }
}

fn write_frame(stream: &mut impl Write, opcode: u8, payload: &[u8]) -> std::io::Result<()> {
    let mut frame = vec![0x80 | opcode];
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

pub fn write_text(stream: &mut impl Write, text: &str) -> std::io::Result<()> {
    write_frame(stream, 0x1, text.as_bytes())
}

pub fn write_pong(stream: &mut impl Write, payload: &[u8]) -> std::io::Result<()> {
    write_frame(stream, 0xA, payload)
}

pub fn write_close(stream: &mut impl Write) -> std::io::Result<()> {
    write_frame(stream, 0x8, &[])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn client_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
        assert!(payload.len() < 126);
        let mask = [0x12, 0x34, 0x56, 0x78];
        let mut frame = vec![0x80 | opcode, 0x80 | payload.len() as u8];
        frame.extend_from_slice(&mask);
        frame.extend(
            payload
                .iter()
                .enumerate()
                .map(|(index, byte)| byte ^ mask[index % mask.len()]),
        );
        frame
    }

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

    #[test]
    fn accept_key_matches_the_rfc_example() {
        assert_eq!(
            accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    #[test]
    fn reads_a_masked_text_frame() {
        let mut frame = Cursor::new(client_frame(0x1, b"hello"));
        match read_frame(&mut frame) {
            Some(Frame::Text(text)) => assert_eq!(text, "hello"),
            _ => panic!("expected a text frame"),
        }
    }

    #[test]
    fn rejects_unmasked_or_fragmented_client_frames() {
        let mut unmasked = Cursor::new(vec![0x88, 0x00]);
        assert!(read_frame(&mut unmasked).is_none());

        let mut fragmented = client_frame(0x1, b"hello");
        fragmented[0] &= !0x80;
        assert!(read_frame(&mut Cursor::new(fragmented)).is_none());
    }

    #[test]
    fn server_frames_are_finished_and_unmasked() {
        let mut bytes = Vec::new();
        write_text(&mut bytes, "hello").expect("write frame");
        assert_eq!(bytes, b"\x81\x05hello");
    }
}
