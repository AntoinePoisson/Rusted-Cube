use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    rc::Rc,
};

use glam::{IVec3, Vec3};
use wasm_bindgen::{closure::Closure, JsCast, JsValue};
use web_sys::{
    Document, Event, HtmlCanvasElement, HtmlElement, KeyboardEvent, MouseEvent, Performance,
    PointerEvent, VisibilityState, Window,
};

use crate::{
    input::Input,
    net::Network,
    player::Player,
    protocol::{Edit, PlayerId, Pose, ServerMessage, MOVE_INTERVAL_MS},
    renderer::{Avatar, Renderer, SkyState},
    world::{Block, ChunkPosition, World},
};

const DAY_LENGTH: f32 = 240.0;
const REACH: f32 = 6.0;
const ACTION_COOLDOWN_MS: f64 = 250.0;
// Keep in sync with the CSS hand swing.
const SWING_MS: f64 = 260.0;
const MESH_BUDGET_MS: f64 = 4.0;
const HUD_INTERVAL_MS: f64 = 250.0;
// Fraction of the ring radius the knob travels, leaving it inside the ring.
const KNOB_TRAVEL: f64 = 0.62;
const TOUCH_STICK_IDS: [&str; 2] = ["touch-move", "touch-camera"];

type AnimationCallback = Rc<RefCell<Option<Closure<dyn FnMut(f64)>>>>;

struct Hud {
    seed: HtmlElement,
    chunks: HtmlElement,
    fps: HtmlElement,
    mesh: HtmlElement,
    vertices: HtmlElement,
    held: HtmlElement,
    players: HtmlElement,
}

struct RemotePlayer {
    from: Pose,
    to: Pose,
    blend: f32,
    color: Vec3,
}

impl RemotePlayer {
    fn new(pose: Pose, id: PlayerId) -> Self {
        Self {
            from: pose,
            to: pose,
            blend: 1.0,
            color: color_for(id),
        }
    }

    fn push(&mut self, pose: Pose) {
        self.from = self.interpolated();
        self.to = pose;
        self.blend = 0.0;
    }

    fn advance(&mut self, delta: f32) {
        let step = delta / (MOVE_INTERVAL_MS as f32 / 1_000.0);
        self.blend = (self.blend + step).min(1.0);
    }

    fn interpolated(&self) -> Pose {
        let lerp = |a: f32, b: f32| a + (b - a) * self.blend;
        Pose {
            position: [
                lerp(self.from.position[0], self.to.position[0]),
                lerp(self.from.position[1], self.to.position[1]),
                lerp(self.from.position[2], self.to.position[2]),
            ],
            yaw: self.from.yaw + wrap_angle(self.to.yaw - self.from.yaw) * self.blend,
            pitch: lerp(self.from.pitch, self.to.pitch),
        }
    }
}

fn wrap_angle(angle: f32) -> f32 {
    (angle + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI
}

fn color_for(id: PlayerId) -> Vec3 {
    let hue = (id.wrapping_mul(2_654_435_761) >> 8) as f32 / 16_777_216.0;
    let shift = |offset: f32| ((hue + offset) * std::f32::consts::TAU).sin() * 0.5 + 0.5;
    Vec3::new(shift(0.0), shift(0.33), shift(0.66)) * 0.65 + Vec3::splat(0.25)
}

fn block_name(block: Block) -> &'static str {
    match block {
        Block::Air => "AIR",
        Block::Grass => "GRASS",
        Block::Dirt => "DIRT",
        Block::Stone => "STONE",
        Block::Sand => "SAND",
        Block::Wood => "WOOD",
        Block::Leaves => "LEAVES",
        Block::Snow => "SNOW",
    }
}

struct Game {
    world: World,
    player: Player,
    input: Input,
    renderer: Renderer,
    seed: u32,
    last_frame: f64,
    time_of_day: f32,
    pending_meshes: VecDeque<ChunkPosition>,
    last_mesh_ms: f64,
    frame_accumulator: f64,
    frame_count: u32,
    fps: f64,
    last_hud_update: f64,
    hud: Hud,
    hand: HtmlElement,
    hand_swing_until: f64,
    // One slot for now; this can become a hotbar later.
    held_block: Option<Block>,
    next_action_at: f64,
    network: Option<Network>,
    remote_players: HashMap<PlayerId, RemotePlayer>,
    loader: Option<HtmlElement>,
    loader_hidden: bool,
    performance: Performance,
    playing: bool,
    visible: bool,
}

impl Game {
    fn new(
        canvas: HtmlCanvasElement,
        hud: Hud,
        hand: HtmlElement,
        loader: Option<HtmlElement>,
        performance: Performance,
    ) -> Result<Self, JsValue> {
        let now = performance.now();
        let time_of_day = 0.18;

        let mut renderer = Renderer::new(canvas)?;
        renderer.present_sky(&SkyState::at(time_of_day));

        let seed = initial_seed();
        let mut world = World::generate(seed);
        let player = spawn_player(&world);
        let pending_meshes = world.take_dirty().into_iter().collect();

        hud.seed.set_inner_text(&format!("SEED {seed}"));

        let mut game = Self {
            world,
            player,
            input: Input::default(),
            renderer,
            seed,
            last_frame: now,
            time_of_day,
            pending_meshes,
            last_mesh_ms: 0.0,
            frame_accumulator: 0.0,
            frame_count: 0,
            fps: 0.0,
            last_hud_update: now,
            hud,
            hand,
            hand_swing_until: 0.0,
            held_block: None,
            next_action_at: 0.0,
            network: Network::connect(),
            remote_players: HashMap::new(),
            loader,
            loader_hidden: false,
            performance,
            playing: false,
            visible: true,
        };
        game.process_pending_meshes();
        Ok(game)
    }

    fn frame(&mut self, now: f64) {
        if !self.visible {
            self.last_frame = now;
            return;
        }
        if !self.playing && now - self.last_frame < 1_000.0 / 30.0 {
            return;
        }

        let delta = ((now - self.last_frame) as f32 / 1_000.0).clamp(0.0, 0.05);
        self.last_frame = now;

        self.time_of_day = (self.time_of_day + delta / DAY_LENGTH).fract();

        self.process_network(now, delta);

        if self.input.take_regeneration_request() && !self.is_online() {
            self.regenerate();
        }

        self.player.update(&mut self.input, &self.world, delta);
        self.world.load_chunks_around(self.player.position);

        self.handle_block_actions(now);

        self.queue_dirty_meshes();
        self.process_pending_meshes();
        self.update_loader();
        let world = &self.world;
        self.renderer
            .retain_chunks(|position| world.is_loaded(position));

        let hand_class = if now < self.hand_swing_until {
            "hand hand--swing"
        } else if self.input.is_moving() {
            "hand hand--moving"
        } else {
            "hand"
        };
        self.hand.set_class_name(hand_class);

        let sky = SkyState::at(self.time_of_day);
        let eye = self.player.eye_position(&self.world);
        let direction = self.player.look_direction();
        self.renderer.render(eye, direction, &sky);
        self.renderer.render_avatars(&self.avatars(), &sky);

        if let Some((cell, _)) = self.world.raycast(eye, direction, REACH) {
            self.renderer.render_block_highlight(cell, &sky);
        }

        self.update_hud(now, delta);
    }

    fn set_playing(&mut self, playing: bool) {
        self.playing = playing;
        self.input.clear();
        self.last_frame = self.performance.now();
    }

    fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
        self.input.clear();
        self.last_frame = self.performance.now();
    }

    fn is_online(&self) -> bool {
        self.network
            .as_ref()
            .map_or(false, |network| network.is_connected())
    }

    fn process_network(&mut self, now: f64, delta: f32) {
        let Some(network) = self.network.as_mut() else {
            return;
        };

        if network.is_closed() {
            self.remote_players.clear();
            return;
        }

        for message in network.drain() {
            match message {
                ServerMessage::Welcome {
                    id,
                    seed,
                    edits,
                    players,
                } => {
                    network.set_id(id);
                    self.seed = seed;
                    self.world = World::generate(seed);
                    self.player = spawn_player(&self.world);
                    for edit in edits {
                        apply_edit(&mut self.world, edit);
                    }
                    self.pending_meshes.clear();
                    self.renderer.clear_chunks();
                    self.remote_players = players
                        .into_iter()
                        .map(|(id, pose)| (id, RemotePlayer::new(pose, id)))
                        .collect();
                    self.hud.seed.set_inner_text(&format!("SEED {seed}"));
                }
                ServerMessage::PlayerJoined { id, pose } => {
                    self.remote_players.insert(id, RemotePlayer::new(pose, id));
                }
                ServerMessage::PlayerMoved { id, pose } => {
                    self.remote_players
                        .entry(id)
                        .or_insert_with(|| RemotePlayer::new(pose, id))
                        .push(pose);
                }
                ServerMessage::PlayerLeft { id } => {
                    self.remote_players.remove(&id);
                }
                ServerMessage::BlockChanged { edit } => {
                    apply_edit(&mut self.world, edit);
                }
            }
        }

        for remote in self.remote_players.values_mut() {
            remote.advance(delta);
        }

        network.send_pose(
            Pose {
                position: self.player.position.to_array(),
                yaw: self.player.yaw(),
                pitch: self.player.pitch(),
            },
            now,
        );
    }

    fn broadcast_edit(&self, position: IVec3, block: Block) {
        if let Some(network) = self.network.as_ref() {
            network.send_block(Edit {
                position: position.to_array(),
                block: block as u8,
            });
        }
    }

    fn avatars(&self) -> Vec<Avatar> {
        self.remote_players
            .values()
            .map(|remote| {
                let pose = remote.interpolated();
                Avatar {
                    position: Vec3::from_array(pose.position),
                    yaw: pose.yaw,
                    color: remote.color,
                }
            })
            .collect()
    }

    fn handle_block_actions(&mut self, now: f64) {
        let wants_break = self.input.take_break_request();
        let wants_place = self.input.take_place_request();
        if !wants_break && !wants_place {
            return;
        }
        if now < self.next_action_at {
            return;
        }

        let eye = self.player.eye_position(&self.world);
        let direction = self.player.look_direction();

        if wants_break {
            if let Some((position, block)) = self.world.break_block(eye, direction, REACH) {
                self.held_block = Some(block);
                self.update_held_label();
                self.broadcast_edit(position, Block::Air);
            }
            self.start_swing(now);
        } else if let Some(block) = self.held_block {
            let (min, max) = self.player.bounds();
            if let Some(position) = self
                .world
                .place_block(eye, direction, REACH, block, min, max)
            {
                self.broadcast_edit(position, block);
            }
            self.start_swing(now);
        }
    }

    fn start_swing(&mut self, now: f64) {
        self.hand_swing_until = now + SWING_MS;
        self.next_action_at = now + ACTION_COOLDOWN_MS;
    }

    fn update_held_label(&self) {
        let label = match self.held_block {
            Some(block) => format!("HOLDING {}", block_name(block)),
            None => "HOLDING -".to_owned(),
        };
        self.hud.held.set_inner_text(&label);
    }

    fn regenerate(&mut self) {
        self.seed = random_seed();
        self.world = World::generate(self.seed);
        self.player = spawn_player(&self.world);
        self.pending_meshes.clear();
        self.renderer.clear_chunks();
        self.queue_dirty_meshes();
        self.hud.seed.set_inner_text(&format!("SEED {}", self.seed));
    }

    fn queue_dirty_meshes(&mut self) {
        for position in self.world.take_dirty() {
            if !self.pending_meshes.contains(&position) {
                self.pending_meshes.push_back(position);
            }
        }
    }

    fn process_pending_meshes(&mut self) {
        if self.pending_meshes.is_empty() {
            return;
        }

        let started = self.performance.now();
        let deadline = started + MESH_BUDGET_MS;
        while let Some(position) = self.pending_meshes.pop_front() {
            let mesh = self.world.build_chunk_mesh_greedy(position);
            self.renderer
                .upload_chunk(position, &mesh.vertices, mesh.quad_count);

            if self.performance.now() >= deadline {
                break;
            }
        }
        self.last_mesh_ms = self.performance.now() - started;
    }

    fn update_loader(&mut self) {
        if self.loader_hidden || !self.pending_meshes.is_empty() {
            return;
        }
        self.loader_hidden = true;
        if let Some(loader) = self.loader.take() {
            loader.set_class_name("loader loader--hidden");
        }
    }

    fn update_hud(&mut self, now: f64, delta: f32) {
        self.frame_accumulator += delta as f64;
        self.frame_count += 1;

        if now - self.last_hud_update < HUD_INTERVAL_MS {
            return;
        }
        if self.frame_accumulator > 0.0 {
            self.fps = self.frame_count as f64 / self.frame_accumulator;
        }
        self.last_hud_update = now;
        self.frame_accumulator = 0.0;
        self.frame_count = 0;

        self.hud.fps.set_inner_text(&format!("{:.0} FPS", self.fps));
        self.hud.chunks.set_inner_text(&format!(
            "CHUNKS {}/{}",
            self.renderer.visible_chunks(),
            self.world.loaded_chunk_count()
        ));
        self.hud
            .mesh
            .set_inner_text(&format!("MESH {:.2} MS", self.last_mesh_ms));
        self.hud.vertices.set_inner_text(&format!(
            "TRIS {}K",
            self.renderer.drawn_triangles() / 1_000
        ));
        self.hud
            .players
            .set_inner_text(&match self.network.as_ref() {
                Some(network) if network.is_connected() => format!(
                    "P{} ONLINE {}",
                    network.id().unwrap_or(0),
                    self.remote_players.len() + 1
                ),
                _ => "SOLO".to_owned(),
            });
    }
}

pub fn start() -> Result<(), JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("Window is unavailable"))?;
    let document = window
        .document()
        .ok_or_else(|| JsValue::from_str("Document is unavailable"))?;
    let canvas = element::<HtmlCanvasElement>(&document, "game-canvas")?;
    let hud = Hud {
        seed: element(&document, "seed")?,
        chunks: element(&document, "chunks")?,
        fps: element(&document, "fps")?,
        mesh: element(&document, "mesh")?,
        vertices: element(&document, "vertices")?,
        held: element(&document, "held")?,
        players: element(&document, "players")?,
    };
    let hand = element::<HtmlElement>(&document, "hand")?;
    let performance = window
        .performance()
        .ok_or_else(|| JsValue::from_str("Performance timer is unavailable"))?;
    let loader = document
        .get_element_by_id("loader")
        .and_then(|element| element.dyn_into::<HtmlElement>().ok());
    let game = Rc::new(RefCell::new(Game::new(
        canvas.clone(),
        hud,
        hand,
        loader,
        performance,
    )?));

    register_start_ui(&document, &canvas, Rc::clone(&game))?;
    register_keyboard(&window, Rc::clone(&game))?;
    register_mouse(&document, &canvas, Rc::clone(&game))?;
    register_touch_controls(&document, Rc::clone(&game))?;
    register_pointer_lock_ui(&document, Rc::clone(&game))?;
    register_visibility(&document, Rc::clone(&game))?;
    start_animation_loop(&window, game)?;
    Ok(())
}

fn register_start_ui(
    document: &Document,
    canvas: &HtmlCanvasElement,
    game: Rc<RefCell<Game>>,
) -> Result<(), JsValue> {
    let button = element::<HtmlElement>(document, "play-button")?;
    let start_document = document.clone();
    let start_canvas = canvas.clone();
    let callback = Closure::<dyn FnMut(Event)>::new(move |_| {
        if uses_touch(&start_document) {
            apply_playing_ui(&start_document, true);
            game.borrow_mut().set_playing(true);
            let _ = start_canvas.focus();
        } else {
            start_canvas.request_pointer_lock();
        }
    });
    button.add_event_listener_with_callback("click", callback.as_ref().unchecked_ref())?;
    callback.forget();
    Ok(())
}

fn register_keyboard(window: &Window, game: Rc<RefCell<Game>>) -> Result<(), JsValue> {
    let keydown_game = Rc::clone(&game);
    let keydown = Closure::<dyn FnMut(KeyboardEvent)>::new(move |event: KeyboardEvent| {
        if matches!(
            event.code().as_str(),
            "Space" | "ArrowUp" | "ArrowDown" | "ArrowLeft" | "ArrowRight"
        ) {
            event.prevent_default();
        }
        if event.code() == "KeyR" && !event.repeat() {
            keydown_game.borrow_mut().input.request_regeneration();
        }
        keydown_game.borrow_mut().input.set_key(event.code(), true);
    });
    window.add_event_listener_with_callback("keydown", keydown.as_ref().unchecked_ref())?;
    keydown.forget();

    let keyup_game = Rc::clone(&game);
    let keyup = Closure::<dyn FnMut(KeyboardEvent)>::new(move |event: KeyboardEvent| {
        keyup_game.borrow_mut().input.set_key(event.code(), false);
    });
    window.add_event_listener_with_callback("keyup", keyup.as_ref().unchecked_ref())?;
    keyup.forget();

    let blur = Closure::<dyn FnMut()>::new(move || {
        game.borrow_mut().input.clear();
    });
    window.add_event_listener_with_callback("blur", blur.as_ref().unchecked_ref())?;
    blur.forget();
    Ok(())
}

fn register_mouse(
    document: &Document,
    canvas: &HtmlCanvasElement,
    game: Rc<RefCell<Game>>,
) -> Result<(), JsValue> {
    let lock_document = document.clone();
    let lock_canvas = canvas.clone();
    let click_game = Rc::clone(&game);
    let mousedown = Closure::<dyn FnMut(MouseEvent)>::new(move |event: MouseEvent| {
        if lock_document.pointer_lock_element().is_none() {
            lock_canvas.request_pointer_lock();
            return;
        }
        match event.button() {
            0 => click_game.borrow_mut().input.request_break(),
            2 => click_game.borrow_mut().input.request_place(),
            _ => {}
        }
    });
    canvas.add_event_listener_with_callback("mousedown", mousedown.as_ref().unchecked_ref())?;
    mousedown.forget();

    let context_menu = Closure::<dyn FnMut(MouseEvent)>::new(move |event: MouseEvent| {
        event.prevent_default();
    });
    canvas
        .add_event_listener_with_callback("contextmenu", context_menu.as_ref().unchecked_ref())?;
    context_menu.forget();

    let motion_document = document.clone();
    let mousemove = Closure::<dyn FnMut(MouseEvent)>::new(move |event: MouseEvent| {
        if motion_document.pointer_lock_element().is_some() {
            game.borrow_mut()
                .input
                .add_mouse_motion(event.movement_x() as f32, event.movement_y() as f32);
        }
    });
    document.add_event_listener_with_callback("mousemove", mousemove.as_ref().unchecked_ref())?;
    mousemove.forget();
    Ok(())
}

fn register_pointer_lock_ui(document: &Document, game: Rc<RefCell<Game>>) -> Result<(), JsValue> {
    let pointer_document = document.clone();
    let callback = Closure::<dyn FnMut()>::new(move || {
        let playing = pointer_document.pointer_lock_element().is_some();
        apply_playing_ui(&pointer_document, playing);
        game.borrow_mut().set_playing(playing);
    });
    document
        .add_event_listener_with_callback("pointerlockchange", callback.as_ref().unchecked_ref())?;
    callback.forget();

    let note = element::<HtmlElement>(document, "device-note")?;
    let error = Closure::<dyn FnMut(Event)>::new(move |_| {
        note.set_inner_text("Cursor capture failed — check browser permissions");
        note.set_class_name("intro__device intro__device--error");
    });
    document
        .add_event_listener_with_callback("pointerlockerror", error.as_ref().unchecked_ref())?;
    error.forget();
    Ok(())
}

fn register_visibility(document: &Document, game: Rc<RefCell<Game>>) -> Result<(), JsValue> {
    let visibility_document = document.clone();
    let callback = Closure::<dyn FnMut(Event)>::new(move |_| {
        let visible = visibility_document.visibility_state() != VisibilityState::Hidden;
        game.borrow_mut().set_visible(visible);
    });
    document
        .add_event_listener_with_callback("visibilitychange", callback.as_ref().unchecked_ref())?;
    callback.forget();
    Ok(())
}

fn register_touch_controls(document: &Document, game: Rc<RefCell<Game>>) -> Result<(), JsValue> {
    // Sticks report screen-space vectors: forward is up, looking down is down.
    register_touch_stick(
        document,
        TOUCH_STICK_IDS[0],
        |input, x, y| input.set_move_axis(x, -y),
        Rc::clone(&game),
    )?;
    register_touch_stick(
        document,
        TOUCH_STICK_IDS[1],
        |input, x, y| input.set_look_axis(x, y),
        Rc::clone(&game),
    )?;
    register_touch_key(document, "touch-jump", "Space", Rc::clone(&game))?;
    register_touch_action(
        document,
        "touch-break",
        Input::request_break,
        Rc::clone(&game),
    )?;
    register_touch_action(
        document,
        "touch-place",
        Input::request_place,
        Rc::clone(&game),
    )?;
    register_touch_look(document, Rc::clone(&game))?;

    let pause = element::<HtmlElement>(document, "touch-pause")?;
    let pause_document = document.clone();
    let pause_game = Rc::clone(&game);
    let callback = Closure::<dyn FnMut(Event)>::new(move |event: Event| {
        event.prevent_default();
        apply_playing_ui(&pause_document, false);
        pause_game.borrow_mut().set_playing(false);
        if let Ok(play) = element::<HtmlElement>(&pause_document, "play-button") {
            let _ = play.focus();
        }
    });
    pause.add_event_listener_with_callback("click", callback.as_ref().unchecked_ref())?;
    callback.forget();
    Ok(())
}

fn register_touch_key(
    document: &Document,
    id: &str,
    code: &'static str,
    game: Rc<RefCell<Game>>,
) -> Result<(), JsValue> {
    let control = element::<HtmlElement>(document, id)?;
    let down_control = control.clone();
    let down_game = Rc::clone(&game);
    let down = Closure::<dyn FnMut(PointerEvent)>::new(move |event: PointerEvent| {
        event.prevent_default();
        let _ = down_control.set_pointer_capture(event.pointer_id());
        down_control.set_class_name("is-pressed");
        down_game.borrow_mut().input.set_key(code.to_owned(), true);
    });
    control.add_event_listener_with_callback("pointerdown", down.as_ref().unchecked_ref())?;
    down.forget();

    let up_control = control.clone();
    let up = Closure::<dyn FnMut(PointerEvent)>::new(move |event: PointerEvent| {
        event.prevent_default();
        up_control.set_class_name("");
        game.borrow_mut().input.set_key(code.to_owned(), false);
    });
    control.add_event_listener_with_callback("pointerup", up.as_ref().unchecked_ref())?;
    control.add_event_listener_with_callback("pointercancel", up.as_ref().unchecked_ref())?;
    up.forget();
    Ok(())
}

fn register_touch_action(
    document: &Document,
    id: &str,
    action: fn(&mut Input),
    game: Rc<RefCell<Game>>,
) -> Result<(), JsValue> {
    let control = element::<HtmlElement>(document, id)?;
    let down_control = control.clone();
    let down = Closure::<dyn FnMut(PointerEvent)>::new(move |event: PointerEvent| {
        event.prevent_default();
        let _ = down_control.set_pointer_capture(event.pointer_id());
        down_control.set_class_name("is-pressed");
        action(&mut game.borrow_mut().input);
    });
    control.add_event_listener_with_callback("pointerdown", down.as_ref().unchecked_ref())?;
    down.forget();

    let up_control = control.clone();
    let up = Closure::<dyn FnMut(PointerEvent)>::new(move |event: PointerEvent| {
        event.prevent_default();
        up_control.set_class_name("");
    });
    control.add_event_listener_with_callback("pointerup", up.as_ref().unchecked_ref())?;
    control.add_event_listener_with_callback("pointercancel", up.as_ref().unchecked_ref())?;
    up.forget();
    Ok(())
}

/// Wires an analog on-screen stick: the knob follows the finger inside the ring and
/// `apply` receives the resulting unit-circle vector in screen space (y grows downwards).
fn register_touch_stick(
    document: &Document,
    id: &str,
    apply: fn(&mut Input, f32, f32),
    game: Rc<RefCell<Game>>,
) -> Result<(), JsValue> {
    let stick = element::<HtmlElement>(document, id)?;
    let knob = stick
        .query_selector(".touch-stick__knob")?
        .and_then(|element| element.dyn_into::<HtmlElement>().ok())
        .ok_or_else(|| JsValue::from_str(&format!("#{id} has no knob element")))?;
    let pointer = Rc::new(RefCell::new(None::<i32>));

    let down_stick = stick.clone();
    let down_knob = knob.clone();
    let down_pointer = Rc::clone(&pointer);
    let down_game = Rc::clone(&game);
    let down = Closure::<dyn FnMut(PointerEvent)>::new(move |event: PointerEvent| {
        event.prevent_default();
        let _ = down_stick.set_pointer_capture(event.pointer_id());
        *down_pointer.borrow_mut() = Some(event.pointer_id());
        let _ = down_stick.class_list().add_1("is-active");
        drag_stick(&down_stick, &down_knob, &event, apply, &down_game);
    });
    stick.add_event_listener_with_callback("pointerdown", down.as_ref().unchecked_ref())?;
    down.forget();

    let move_stick = stick.clone();
    let move_knob = knob.clone();
    let move_pointer = Rc::clone(&pointer);
    let move_game = Rc::clone(&game);
    let movement = Closure::<dyn FnMut(PointerEvent)>::new(move |event: PointerEvent| {
        if *move_pointer.borrow() != Some(event.pointer_id()) {
            return;
        }
        event.prevent_default();
        drag_stick(&move_stick, &move_knob, &event, apply, &move_game);
    });
    stick.add_event_listener_with_callback("pointermove", movement.as_ref().unchecked_ref())?;
    movement.forget();

    let up_stick = stick.clone();
    let up = Closure::<dyn FnMut(PointerEvent)>::new(move |event: PointerEvent| {
        if *pointer.borrow() != Some(event.pointer_id()) {
            return;
        }
        event.prevent_default();
        pointer.borrow_mut().take();
        let _ = up_stick.class_list().remove_1("is-active");
        let _ = knob
            .style()
            .set_property("transform", "translate3d(0, 0, 0)");
        apply(&mut game.borrow_mut().input, 0.0, 0.0);
    });
    stick.add_event_listener_with_callback("pointerup", up.as_ref().unchecked_ref())?;
    stick.add_event_listener_with_callback("pointercancel", up.as_ref().unchecked_ref())?;
    up.forget();
    Ok(())
}

fn drag_stick(
    stick: &HtmlElement,
    knob: &HtmlElement,
    event: &PointerEvent,
    apply: fn(&mut Input, f32, f32),
    game: &Rc<RefCell<Game>>,
) {
    let rect = stick.get_bounding_client_rect();
    let radius = (rect.width() * 0.5).max(1.0);
    let mut x = (event.client_x() as f64 - (rect.left() + rect.width() * 0.5)) / radius;
    let mut y = (event.client_y() as f64 - (rect.top() + rect.height() * 0.5)) / radius;
    let length = (x * x + y * y).sqrt();
    if length > 1.0 {
        x /= length;
        y /= length;
    }

    let travel = radius * KNOB_TRAVEL;
    let _ = knob.style().set_property(
        "transform",
        &format!("translate3d({}px, {}px, 0)", x * travel, y * travel),
    );
    apply(&mut game.borrow_mut().input, x as f32, y as f32);
}

fn register_touch_look(document: &Document, game: Rc<RefCell<Game>>) -> Result<(), JsValue> {
    let area = element::<HtmlElement>(document, "touch-look")?;
    let pointer = Rc::new(RefCell::new(None::<(i32, i32, i32)>));

    let down_area = area.clone();
    let down_pointer = Rc::clone(&pointer);
    let down = Closure::<dyn FnMut(PointerEvent)>::new(move |event: PointerEvent| {
        event.prevent_default();
        let _ = down_area.set_pointer_capture(event.pointer_id());
        *down_pointer.borrow_mut() = Some((event.pointer_id(), event.client_x(), event.client_y()));
    });
    area.add_event_listener_with_callback("pointerdown", down.as_ref().unchecked_ref())?;
    down.forget();

    let move_pointer = Rc::clone(&pointer);
    let move_game = Rc::clone(&game);
    let movement = Closure::<dyn FnMut(PointerEvent)>::new(move |event: PointerEvent| {
        event.prevent_default();
        let previous = *move_pointer.borrow();
        let Some((pointer_id, x, y)) = previous else {
            return;
        };
        if pointer_id != event.pointer_id() {
            return;
        }
        let dx = event.client_x() - x;
        let dy = event.client_y() - y;
        *move_pointer.borrow_mut() = Some((pointer_id, event.client_x(), event.client_y()));
        move_game
            .borrow_mut()
            .input
            .add_mouse_motion(dx as f32 * 0.85, dy as f32 * 0.85);
    });
    area.add_event_listener_with_callback("pointermove", movement.as_ref().unchecked_ref())?;
    movement.forget();

    let up = Closure::<dyn FnMut(PointerEvent)>::new(move |event: PointerEvent| {
        event.prevent_default();
        if pointer
            .borrow()
            .map(|(pointer_id, _, _)| pointer_id == event.pointer_id())
            .unwrap_or(false)
        {
            pointer.borrow_mut().take();
        }
    });
    area.add_event_listener_with_callback("pointerup", up.as_ref().unchecked_ref())?;
    area.add_event_listener_with_callback("pointercancel", up.as_ref().unchecked_ref())?;
    up.forget();
    Ok(())
}

fn uses_touch(document: &Document) -> bool {
    document
        .document_element()
        .and_then(|root| root.get_attribute("data-input"))
        .as_deref()
        == Some("touch")
}

fn apply_playing_ui(document: &Document, playing: bool) {
    if let Some(body) = document.body() {
        body.set_class_name(if playing { "playing" } else { "" });
    }
    if let Some(intro) = document
        .get_element_by_id("game-instructions")
        .and_then(|element| element.parent_element())
    {
        if playing {
            let _ = intro.set_attribute("aria-hidden", "true");
        } else {
            let _ = intro.remove_attribute("aria-hidden");
        }
    }
    if uses_touch(document) {
        if let Some(controls) = document.get_element_by_id("touch-controls") {
            let _ = controls.set_attribute("aria-hidden", if playing { "false" } else { "true" });
        }
        recentre_touch_sticks(document);
    }
}

/// Clears the stick visuals so a knob left off-centre by a lost pointer does not stay stuck.
fn recentre_touch_sticks(document: &Document) {
    for id in TOUCH_STICK_IDS {
        let Ok(stick) = element::<HtmlElement>(document, id) else {
            continue;
        };
        let _ = stick.class_list().remove_1("is-active");
        if let Some(knob) = stick
            .query_selector(".touch-stick__knob")
            .ok()
            .flatten()
            .and_then(|element| element.dyn_into::<HtmlElement>().ok())
        {
            let _ = knob
                .style()
                .set_property("transform", "translate3d(0, 0, 0)");
        }
    }
}

fn start_animation_loop(window: &Window, game: Rc<RefCell<Game>>) -> Result<(), JsValue> {
    let animation: AnimationCallback = Rc::new(RefCell::new(None));
    let next_animation = Rc::clone(&animation);
    let animation_window = window.clone();

    *animation.borrow_mut() = Some(Closure::new(move |time: f64| {
        game.borrow_mut().frame(time);
        if let Some(callback) = next_animation.borrow().as_ref() {
            let _ = animation_window.request_animation_frame(callback.as_ref().unchecked_ref());
        }
    }));

    let callback = animation.borrow();
    window.request_animation_frame(
        callback
            .as_ref()
            .ok_or_else(|| JsValue::from_str("Animation callback is unavailable"))?
            .as_ref()
            .unchecked_ref(),
    )?;
    Ok(())
}

fn apply_edit(world: &mut World, edit: Edit) {
    let Some(block) = Block::from_u8(edit.block) else {
        return;
    };
    world.set(IVec3::from_array(edit.position), block);
}

/// Honours `?seed=` so a world can be shared, and rolls a random one otherwise.
fn initial_seed() -> u32 {
    seed_from_query().unwrap_or_else(random_seed)
}

fn seed_from_query() -> Option<u32> {
    let search = web_sys::window()?.location().search().ok()?;
    search
        .trim_start_matches('?')
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(key, _)| *key == "seed")
        .and_then(|(_, value)| value.parse::<u32>().ok())
}

fn random_seed() -> u32 {
    // `Math::random` carries 53 bits of mantissa, enough to fill the whole range.
    (js_sys::Math::random() * f64::from(u32::MAX)) as u32
}

fn spawn_player(world: &World) -> Player {
    let x = 0;
    let z = 0;
    let y = world.highest_block(x, z).unwrap_or(20) as f32 + 1.01;
    Player::new(Vec3::new(x as f32 + 0.5, y, z as f32 + 0.5))
}

fn element<T: JsCast>(document: &Document, id: &str) -> Result<T, JsValue> {
    document
        .get_element_by_id(id)
        .ok_or_else(|| JsValue::from_str(&format!("Missing #{id} element")))?
        .dyn_into::<T>()
        .map_err(|_| JsValue::from_str(&format!("#{id} has an unexpected element type")))
}
