//! WebSocket client for LAN play.
//!
//! Connecting is optional by design: when the page is served by anything other
//! than the bundled server the socket simply fails to open and the game carries
//! on single-player. Multiplayer is a bonus, never a prerequisite.

use std::{cell::RefCell, collections::VecDeque, rc::Rc};

use wasm_bindgen::{closure::Closure, JsCast};
use web_sys::{ErrorEvent, Event, MessageEvent, WebSocket};

use crate::protocol::{ClientMessage, Edit, PlayerId, Pose, ServerMessage, MOVE_INTERVAL_MS};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
    Connecting,
    Open,
    Closed,
}

pub struct Network {
    socket: WebSocket,
    inbox: Rc<RefCell<VecDeque<ServerMessage>>>,
    status: Rc<RefCell<Status>>,
    id: Option<PlayerId>,
    last_move_sent: f64,
}

impl Network {
    /// Starts connecting to the server that served this page. Returns `None`
    /// only when the URL cannot be built at all; a refused connection is
    /// reported later through `is_connected`.
    pub fn connect() -> Option<Self> {
        let window = web_sys::window()?;

        // bootstrap.js probes the host first. Opening a socket that cannot
        // succeed would make the browser log a failed handshake on every static
        // host, so absence of the flag means single-player, quietly.
        let serves_multiplayer = window
            .document()
            .and_then(|document| document.document_element())
            .and_then(|root| root.get_attribute("data-multiplayer"))
            .map_or(false, |value| value == "1");
        if !serves_multiplayer {
            return None;
        }

        let location = window.location();
        let host = location.host().ok()?;
        let scheme = match location.protocol().ok()?.as_str() {
            "https:" => "wss",
            _ => "ws",
        };

        let socket = WebSocket::new(&format!("{scheme}://{host}/ws")).ok()?;
        let inbox = Rc::new(RefCell::new(VecDeque::new()));
        let status = Rc::new(RefCell::new(Status::Connecting));

        let message_inbox = Rc::clone(&inbox);
        let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
            let Some(text) = event.data().as_string() else {
                return;
            };
            if let Some(message) = ServerMessage::decode(&text) {
                message_inbox.borrow_mut().push_back(message);
            }
        });
        socket.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        on_message.forget();

        let open_status = Rc::clone(&status);
        let on_open = Closure::<dyn FnMut(Event)>::new(move |_| {
            *open_status.borrow_mut() = Status::Open;
        });
        socket.set_onopen(Some(on_open.as_ref().unchecked_ref()));
        on_open.forget();

        // No server on this address: stay single-player, and do not shout about
        // it in the console.
        let close_status = Rc::clone(&status);
        let on_close = Closure::<dyn FnMut(Event)>::new(move |_| {
            *close_status.borrow_mut() = Status::Closed;
        });
        socket.set_onclose(Some(on_close.as_ref().unchecked_ref()));
        on_close.forget();

        let error_status = Rc::clone(&status);
        let on_error = Closure::<dyn FnMut(ErrorEvent)>::new(move |_| {
            *error_status.borrow_mut() = Status::Closed;
        });
        socket.set_onerror(Some(on_error.as_ref().unchecked_ref()));
        on_error.forget();

        Some(Self {
            socket,
            inbox,
            status,
            id: None,
            last_move_sent: 0.0,
        })
    }

    pub fn is_connected(&self) -> bool {
        *self.status.borrow() == Status::Open
    }

    /// The server never echoes a client's own messages back, so the id is kept
    /// only for display purposes.
    pub fn set_id(&mut self, id: PlayerId) {
        self.id = Some(id);
    }

    pub fn id(&self) -> Option<PlayerId> {
        self.id
    }

    /// Takes every message received since the last call.
    pub fn drain(&self) -> Vec<ServerMessage> {
        self.inbox.borrow_mut().drain(..).collect()
    }

    /// Reports the local pose, rate-limited: sending every frame would be far
    /// more traffic than the remote side can use.
    pub fn send_pose(&mut self, pose: Pose, now: f64) {
        if !self.is_connected() || now - self.last_move_sent < MOVE_INTERVAL_MS {
            return;
        }
        self.last_move_sent = now;
        self.send(&ClientMessage::Move { pose });
    }

    pub fn send_block(&self, edit: Edit) {
        if !self.is_connected() {
            return;
        }
        self.send(&ClientMessage::SetBlock { edit });
    }

    fn send(&self, message: &ClientMessage) {
        let _ = self.socket.send_with_str(&message.encode());
    }
}
