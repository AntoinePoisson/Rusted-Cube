//! Wire format shared by the client and the LAN server.
//!
//! JSON stays hand-written here to keep serde's proc-macro stack out of the
//! WebAssembly build and remain compatible with the project's Rust toolchain.

pub type PlayerId = u32;

pub const DEFAULT_PORT: u16 = 8118;

pub const MOVE_INTERVAL_MS: f64 = 50.0;

pub const MAX_BLOCK_ID: u8 = 7;

pub const WORLD_HEIGHT: i32 = 48;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub position: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
}

/// Stored by the server and replayed to newcomers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Edit {
    pub position: [i32; 3],
    /// `Block` discriminant. Kept as an integer to avoid coupling this module to `world`.
    pub block: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ClientMessage {
    Move { pose: Pose },
    SetBlock { edit: Edit },
}

#[derive(Clone, Debug, PartialEq)]
pub enum ServerMessage {
    /// Sent once on connect, everything needed to rebuild the server's world.
    Welcome {
        id: PlayerId,
        seed: u32,
        edits: Vec<Edit>,
        players: Vec<(PlayerId, Pose)>,
    },
    PlayerJoined {
        id: PlayerId,
        pose: Pose,
    },
    PlayerLeft {
        id: PlayerId,
    },
    PlayerMoved {
        id: PlayerId,
        pose: Pose,
    },
    BlockChanged {
        edit: Edit,
    },
}

fn write_pose(out: &mut String, pose: &Pose) {
    out.push_str(&format!(
        r#"{{"position":[{},{},{}],"yaw":{},"pitch":{}}}"#,
        pose.position[0], pose.position[1], pose.position[2], pose.yaw, pose.pitch
    ));
}

fn write_edit(out: &mut String, edit: &Edit) {
    out.push_str(&format!(
        r#"{{"position":[{},{},{}],"block":{}}}"#,
        edit.position[0], edit.position[1], edit.position[2], edit.block
    ));
}

impl ClientMessage {
    pub fn encode(&self) -> String {
        let mut out = String::new();
        match self {
            ClientMessage::Move { pose } => {
                out.push_str(r#"{"type":"Move","pose":"#);
                write_pose(&mut out, pose);
                out.push('}');
            }
            ClientMessage::SetBlock { edit } => {
                out.push_str(r#"{"type":"SetBlock","edit":"#);
                write_edit(&mut out, edit);
                out.push('}');
            }
        }
        out
    }

    pub fn decode(text: &str) -> Option<Self> {
        let value = Json::parse(text)?;
        match value.get("type")?.as_str()? {
            "Move" => Some(ClientMessage::Move {
                pose: value.get("pose")?.as_pose()?,
            }),
            "SetBlock" => Some(ClientMessage::SetBlock {
                edit: value.get("edit")?.as_edit()?,
            }),
            _ => None,
        }
    }
}

impl ServerMessage {
    pub fn encode(&self) -> String {
        let mut out = String::new();
        match self {
            ServerMessage::Welcome {
                id,
                seed,
                edits,
                players,
            } => {
                out.push_str(&format!(
                    r#"{{"type":"Welcome","id":{id},"seed":{seed},"edits":["#
                ));
                for (index, edit) in edits.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    write_edit(&mut out, edit);
                }
                out.push_str(r#"],"players":["#);
                for (index, (player, pose)) in players.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    out.push_str(&format!(r#"{{"id":{player},"pose":"#));
                    write_pose(&mut out, pose);
                    out.push('}');
                }
                out.push_str("]}");
            }
            ServerMessage::PlayerJoined { id, pose } => {
                out.push_str(&format!(r#"{{"type":"PlayerJoined","id":{id},"pose":"#));
                write_pose(&mut out, pose);
                out.push('}');
            }
            ServerMessage::PlayerLeft { id } => {
                out.push_str(&format!(r#"{{"type":"PlayerLeft","id":{id}}}"#));
            }
            ServerMessage::PlayerMoved { id, pose } => {
                out.push_str(&format!(r#"{{"type":"PlayerMoved","id":{id},"pose":"#));
                write_pose(&mut out, pose);
                out.push('}');
            }
            ServerMessage::BlockChanged { edit } => {
                out.push_str(r#"{"type":"BlockChanged","edit":"#);
                write_edit(&mut out, edit);
                out.push('}');
            }
        }
        out
    }

    pub fn decode(text: &str) -> Option<Self> {
        let value = Json::parse(text)?;
        match value.get("type")?.as_str()? {
            "Welcome" => {
                let mut edits = Vec::new();
                for entry in value.get("edits")?.as_array()? {
                    edits.push(entry.as_edit()?);
                }
                let mut players = Vec::new();
                for entry in value.get("players")?.as_array()? {
                    players.push((entry.get("id")?.as_u32()?, entry.get("pose")?.as_pose()?));
                }
                Some(ServerMessage::Welcome {
                    id: value.get("id")?.as_u32()?,
                    seed: value.get("seed")?.as_u32()?,
                    edits,
                    players,
                })
            }
            "PlayerJoined" => Some(ServerMessage::PlayerJoined {
                id: value.get("id")?.as_u32()?,
                pose: value.get("pose")?.as_pose()?,
            }),
            "PlayerLeft" => Some(ServerMessage::PlayerLeft {
                id: value.get("id")?.as_u32()?,
            }),
            "PlayerMoved" => Some(ServerMessage::PlayerMoved {
                id: value.get("id")?.as_u32()?,
                pose: value.get("pose")?.as_pose()?,
            }),
            "BlockChanged" => Some(ServerMessage::BlockChanged {
                edit: value.get("edit")?.as_edit()?,
            }),
            _ => None,
        }
    }
}

/// Small JSON subset used by the protocol. This is deliberately not a general parser.
#[derive(Clone, Debug, PartialEq)]
enum Json {
    Number(f64),
    Text(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
    Bool(bool),
    Null,
}

impl Json {
    fn parse(text: &str) -> Option<Self> {
        let bytes: Vec<char> = text.chars().collect();
        let mut cursor = 0;
        let value = parse_value(&bytes, &mut cursor)?;
        skip_whitespace(&bytes, &mut cursor);
        if cursor == bytes.len() {
            Some(value)
        } else {
            None
        }
    }

    fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(entries) => entries
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value),
            _ => None,
        }
    }

    fn as_str(&self) -> Option<&str> {
        match self {
            Json::Text(text) => Some(text),
            _ => None,
        }
    }

    fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Number(number) => Some(*number),
            _ => None,
        }
    }

    fn as_u32(&self) -> Option<u32> {
        let number = self.as_f64()?;
        if number.is_finite() && number.fract() == 0.0 && (0.0..=u32::MAX as f64).contains(&number)
        {
            Some(number as u32)
        } else {
            None
        }
    }

    fn as_i32(&self) -> Option<i32> {
        let number = self.as_f64()?;
        if number.is_finite()
            && number.fract() == 0.0
            && (i32::MIN as f64..=i32::MAX as f64).contains(&number)
        {
            Some(number as i32)
        } else {
            None
        }
    }

    fn as_u8(&self) -> Option<u8> {
        let number = self.as_f64()?;
        if number.is_finite() && number.fract() == 0.0 && (0.0..=u8::MAX as f64).contains(&number) {
            Some(number as u8)
        } else {
            None
        }
    }

    fn as_f32(&self) -> Option<f32> {
        let value = self.as_f64()? as f32;
        value.is_finite().then_some(value)
    }

    fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Array(items) => Some(items.as_slice()),
            _ => None,
        }
    }

    fn as_pose(&self) -> Option<Pose> {
        let position = self.get("position")?.as_array()?;
        if position.len() != 3 {
            return None;
        }
        Some(Pose {
            position: [
                position[0].as_f32()?,
                position[1].as_f32()?,
                position[2].as_f32()?,
            ],
            yaw: self.get("yaw")?.as_f32()?,
            pitch: self.get("pitch")?.as_f32()?,
        })
    }

    fn as_edit(&self) -> Option<Edit> {
        let position = self.get("position")?.as_array()?;
        if position.len() != 3 {
            return None;
        }
        let position = [
            position[0].as_i32()?,
            position[1].as_i32()?,
            position[2].as_i32()?,
        ];
        let block = self.get("block")?.as_u8()?;
        if !(0..WORLD_HEIGHT).contains(&position[1]) || block > MAX_BLOCK_ID {
            return None;
        }
        Some(Edit { position, block })
    }
}

fn skip_whitespace(bytes: &[char], cursor: &mut usize) {
    while *cursor < bytes.len() && bytes[*cursor].is_ascii_whitespace() {
        *cursor += 1;
    }
}

fn parse_value(bytes: &[char], cursor: &mut usize) -> Option<Json> {
    skip_whitespace(bytes, cursor);
    match bytes.get(*cursor)? {
        '{' => parse_object(bytes, cursor),
        '[' => parse_array(bytes, cursor),
        '"' => parse_string(bytes, cursor).map(Json::Text),
        't' => parse_literal(bytes, cursor, "true", Json::Bool(true)),
        'f' => parse_literal(bytes, cursor, "false", Json::Bool(false)),
        'n' => parse_literal(bytes, cursor, "null", Json::Null),
        _ => parse_number(bytes, cursor),
    }
}

fn parse_literal(bytes: &[char], cursor: &mut usize, word: &str, value: Json) -> Option<Json> {
    for expected in word.chars() {
        if *bytes.get(*cursor)? != expected {
            return None;
        }
        *cursor += 1;
    }
    Some(value)
}

fn parse_object(bytes: &[char], cursor: &mut usize) -> Option<Json> {
    *cursor += 1;
    let mut entries = Vec::new();
    skip_whitespace(bytes, cursor);
    if *bytes.get(*cursor)? == '}' {
        *cursor += 1;
        return Some(Json::Object(entries));
    }

    loop {
        skip_whitespace(bytes, cursor);
        let key = parse_string(bytes, cursor)?;
        skip_whitespace(bytes, cursor);
        if *bytes.get(*cursor)? != ':' {
            return None;
        }
        *cursor += 1;
        entries.push((key, parse_value(bytes, cursor)?));
        skip_whitespace(bytes, cursor);
        match bytes.get(*cursor)? {
            ',' => *cursor += 1,
            '}' => {
                *cursor += 1;
                return Some(Json::Object(entries));
            }
            _ => return None,
        }
    }
}

fn parse_array(bytes: &[char], cursor: &mut usize) -> Option<Json> {
    *cursor += 1;
    let mut items = Vec::new();
    skip_whitespace(bytes, cursor);
    if *bytes.get(*cursor)? == ']' {
        *cursor += 1;
        return Some(Json::Array(items));
    }

    loop {
        items.push(parse_value(bytes, cursor)?);
        skip_whitespace(bytes, cursor);
        match bytes.get(*cursor)? {
            ',' => *cursor += 1,
            ']' => {
                *cursor += 1;
                return Some(Json::Array(items));
            }
            _ => return None,
        }
    }
}

fn parse_string(bytes: &[char], cursor: &mut usize) -> Option<String> {
    if *bytes.get(*cursor)? != '"' {
        return None;
    }
    *cursor += 1;
    let mut text = String::new();
    loop {
        let ch = *bytes.get(*cursor)?;
        *cursor += 1;
        match ch {
            '"' => return Some(text),
            '\\' => {
                let escaped = *bytes.get(*cursor)?;
                *cursor += 1;
                text.push(match escaped {
                    'n' => '\n',
                    't' => '\t',
                    'r' => '\r',
                    'b' => '\u{8}',
                    'f' => '\u{c}',
                    '"' => '"',
                    '\\' => '\\',
                    '/' => '/',
                    _ => return None,
                });
            }
            '\0'..='\u{1f}' => return None,
            _ => text.push(ch),
        }
    }
}

fn parse_number(bytes: &[char], cursor: &mut usize) -> Option<Json> {
    let start = *cursor;
    if matches!(bytes.get(*cursor), Some('-')) {
        *cursor += 1;
    }

    match bytes.get(*cursor)? {
        '0' => *cursor += 1,
        '1'..='9' => {
            *cursor += 1;
            while matches!(bytes.get(*cursor), Some(c) if c.is_ascii_digit()) {
                *cursor += 1;
            }
        }
        _ => return None,
    }

    if matches!(bytes.get(*cursor), Some('.')) {
        *cursor += 1;
        let decimals = *cursor;
        while matches!(bytes.get(*cursor), Some(c) if c.is_ascii_digit()) {
            *cursor += 1;
        }
        if decimals == *cursor {
            return None;
        }
    }

    if matches!(bytes.get(*cursor), Some('e') | Some('E')) {
        *cursor += 1;
        if matches!(bytes.get(*cursor), Some('-') | Some('+')) {
            *cursor += 1;
        }
        let exponent = *cursor;
        while matches!(bytes.get(*cursor), Some(c) if c.is_ascii_digit()) {
            *cursor += 1;
        }
        if exponent == *cursor {
            return None;
        }
    }

    let text: String = bytes[start..*cursor].iter().collect();
    text.parse::<f64>().ok().map(Json::Number)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pose() -> Pose {
        Pose {
            position: [1.5, -2.25, 3.0],
            yaw: 0.75,
            pitch: -0.5,
        }
    }

    #[test]
    fn server_messages_survive_a_round_trip() {
        let messages = [
            ServerMessage::Welcome {
                id: 7,
                seed: 1_337,
                edits: vec![
                    Edit {
                        position: [1, 2, 3],
                        block: 4,
                    },
                    Edit {
                        position: [-5, 6, -7],
                        block: 0,
                    },
                ],
                players: vec![(9, pose())],
            },
            ServerMessage::PlayerJoined {
                id: 4,
                pose: pose(),
            },
            ServerMessage::PlayerLeft { id: 3 },
            ServerMessage::PlayerMoved {
                id: 3,
                pose: pose(),
            },
            ServerMessage::BlockChanged {
                edit: Edit {
                    position: [-4, 5, -6],
                    block: 2,
                },
            },
        ];

        for message in messages {
            let encoded = message.encode();
            let decoded = ServerMessage::decode(&encoded)
                .unwrap_or_else(|| panic!("could not decode {encoded}"));
            assert_eq!(decoded, message);
        }
    }

    #[test]
    fn client_messages_survive_a_round_trip() {
        for message in [
            ClientMessage::Move { pose: pose() },
            ClientMessage::SetBlock {
                edit: Edit {
                    position: [7, 8, 9],
                    block: 2,
                },
            },
        ] {
            let encoded = message.encode();
            let decoded = ClientMessage::decode(&encoded)
                .unwrap_or_else(|| panic!("could not decode {encoded}"));
            assert_eq!(decoded, message);
        }
    }

    #[test]
    fn welcome_with_no_players_or_edits_round_trips() {
        let message = ServerMessage::Welcome {
            id: 1,
            seed: 42,
            edits: Vec::new(),
            players: Vec::new(),
        };
        assert_eq!(ServerMessage::decode(&message.encode()), Some(message));
    }

    #[test]
    fn malformed_input_is_rejected() {
        for text in [
            "",
            "{",
            "[]",
            "null",
            r#"{"type":"Nope"}"#,
            r#"{"type":"PlayerLeft"}"#,
            r#"{"type":"PlayerLeft","id":"three"}"#,
            r#"{"type":"PlayerLeft","id":-1}"#,
            r#"{"type":"PlayerLeft","id":1.5}"#,
            r#"{"type":"BlockChanged","edit":{"position":[1,2],"block":1}}"#,
            r#"{"type":"BlockChanged","edit":{"position":[1,2,3],"block":900}}"#,
            r#"{"type":"BlockChanged","edit":{"position":[1,2,3],"block":8}}"#,
            r#"{"type":"BlockChanged","edit":{"position":[1,48,3],"block":1}}"#,
            r#"{"type":"BlockChanged","edit":{"position":[1.2,2,3],"block":1}}"#,
            r#"{"type":"BlockChanged","edit":{"position":[1,2,3],"block":1.5}}"#,
            r#"{"type":"PlayerMoved","id":1,"pose":{"position":[1,2,3],"yaw":0}}"#,
            r#"{"type":"PlayerMoved","id":1,"pose":{"position":[1e999,2,3],"yaw":0,"pitch":0}}"#,
            r#"{"type":"PlayerLeft","id":1} trailing"#,
        ] {
            assert_eq!(ServerMessage::decode(text), None, "should reject {text:?}");
        }

        assert_eq!(
            ClientMessage::decode(r#"{"type":"SetBlock","edit":{"position":[1,-1,3],"block":1}}"#),
            None
        );
    }

    #[test]
    fn parser_handles_whitespace_and_escapes() {
        let value = Json::parse(" { \"a\" : [ 1 , 2.5 ] , \"b\" : \"x\\ny\" } ").expect("parses");
        assert_eq!(
            value.get("a").and_then(Json::as_array).map(<[Json]>::len),
            Some(2)
        );
        assert_eq!(value.get("b").and_then(Json::as_str), Some("x\ny"));
    }

    #[test]
    fn parser_rejects_non_json_number_forms_and_control_characters() {
        for text in ["+1", ".5", "1.", "01", "1e", "\"line\nfeed\""] {
            assert_eq!(Json::parse(text), None, "should reject {text:?}");
        }
    }
}
