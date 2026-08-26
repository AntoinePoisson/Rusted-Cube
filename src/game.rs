use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    rc::Rc,
};

use glam::{IVec3, Vec3};
use wasm_bindgen::{closure::Closure, JsCast, JsValue};
use web_sys::{
    Document, HtmlCanvasElement, HtmlElement, KeyboardEvent, MouseEvent, Performance, Window,
};

use crate::{
    input::Input,
    net::Network,
    player::Player,
    protocol::{Edit, PlayerId, Pose, ServerMessage, MOVE_INTERVAL_MS},
    renderer::{Avatar, Renderer, SkyState},
    world::{Block, ChunkPosition, World},
};

const DEFAULT_SEED: u32 = 1_337;
const DAY_LENGTH: f32 = 240.0;
const REACH: f32 = 6.0;
const ACTION_COOLDOWN_MS: f64 = 250.0;
// Keep in sync with the CSS hand swing.
const SWING_MS: f64 = 260.0;
const MESH_BUDGET_MS: f64 = 4.0;
const HUD_INTERVAL_MS: f64 = 250.0;

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

        let mut world = World::generate(DEFAULT_SEED);
        let player = spawn_player(&world);
        let pending_meshes = world.take_dirty().into_iter().collect();

        hud.seed.set_inner_text(&format!("SEED {DEFAULT_SEED}"));

        let mut game = Self {
            world,
            player,
            input: Input::default(),
            renderer,
            seed: DEFAULT_SEED,
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
        };
        game.process_pending_meshes();
        Ok(game)
    }

    fn frame(&mut self, now: f64) {
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
        self.seed = self
            .seed
            .wrapping_mul(1_664_525)
            .wrapping_add(1_013_904_223);
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

    register_keyboard(&window, Rc::clone(&game))?;
    register_mouse(&document, &canvas, Rc::clone(&game))?;
    register_pointer_lock_ui(&document)?;
    start_animation_loop(&window, game)?;
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

fn register_pointer_lock_ui(document: &Document) -> Result<(), JsValue> {
    let pointer_document = document.clone();
    let callback = Closure::<dyn FnMut()>::new(move || {
        if let Some(body) = pointer_document.body() {
            body.set_class_name(if pointer_document.pointer_lock_element().is_some() {
                "playing"
            } else {
                ""
            });
        }
    });
    document
        .add_event_listener_with_callback("pointerlockchange", callback.as_ref().unchecked_ref())?;
    callback.forget();
    Ok(())
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
