use glam::{IVec3, Vec3};

use crate::{
    input::Input,
    world::{World, WORLD_HEIGHT},
};

const PLAYER_RADIUS: f32 = 0.3;
const PLAYER_HEIGHT: f32 = 1.8;
// Kept above the collision box on purpose; `eye_position` handles low ceilings.
const EYE_HEIGHT: f32 = 2.0;
const MIN_EYE_HEIGHT: f32 = 1.4;
const GRAVITY: f32 = 24.0;
const JUMP_SPEED: f32 = 8.5;
const VOID_LEVEL: f32 = -10.0;

pub struct Player {
    pub position: Vec3,
    velocity: Vec3,
    yaw: f32,
    pitch: f32,
    on_ground: bool,
}

impl Player {
    pub fn new(position: Vec3) -> Self {
        Self {
            position,
            velocity: Vec3::ZERO,
            yaw: -std::f32::consts::FRAC_PI_2,
            pitch: -0.12,
            on_ground: false,
        }
    }

    pub fn update(&mut self, input: &mut Input, world: &World, delta: f32) {
        let (mouse_dx, mouse_dy) = input.take_mouse_motion();
        self.yaw += mouse_dx * 0.0022;
        self.pitch = (self.pitch - mouse_dy * 0.0022).clamp(-1.5, 1.5);

        let forward = Vec3::new(self.yaw.cos(), 0.0, self.yaw.sin());
        let right = Vec3::new(-forward.z, 0.0, forward.x);
        let mut movement = Vec3::ZERO;

        if input.is_pressed("KeyW") || input.is_pressed("ArrowUp") {
            movement += forward;
        }
        if input.is_pressed("KeyS") || input.is_pressed("ArrowDown") {
            movement -= forward;
        }
        if input.is_pressed("KeyD") || input.is_pressed("ArrowRight") {
            movement += right;
        }
        if input.is_pressed("KeyA") || input.is_pressed("ArrowLeft") {
            movement -= right;
        }

        let speed = if input.is_pressed("ShiftLeft") || input.is_pressed("ShiftRight") {
            7.5
        } else {
            4.8
        };
        let horizontal = movement.normalize_or_zero() * speed;
        self.velocity.x = horizontal.x;
        self.velocity.z = horizontal.z;

        if input.is_pressed("Space") && self.on_ground {
            self.velocity.y = JUMP_SPEED;
            self.on_ground = false;
        }
        self.velocity.y -= GRAVITY * delta;

        self.move_axis(world, Vec3::new(self.velocity.x * delta, 0.0, 0.0), 0);
        self.move_axis(world, Vec3::new(0.0, self.velocity.y * delta, 0.0), 1);
        self.move_axis(world, Vec3::new(0.0, 0.0, self.velocity.z * delta), 2);

        if self.position.y < VOID_LEVEL {
            self.respawn_above_ground(world);
        }
    }

    fn respawn_above_ground(&mut self, world: &World) {
        let x = self.position.x.floor() as i32;
        let z = self.position.z.floor() as i32;
        let ground = world.highest_block(x, z).unwrap_or(WORLD_HEIGHT - 1);
        self.position = Vec3::new(x as f32 + 0.5, ground as f32 + 1.01, z as f32 + 0.5);
        self.velocity = Vec3::ZERO;
    }

    /// Lowers the camera when the normal eye position is inside a block.
    pub fn eye_position(&self, world: &World) -> Vec3 {
        let mut height = EYE_HEIGHT;
        while height > MIN_EYE_HEIGHT {
            let eye = self.position + Vec3::Y * height;
            if !world.is_solid(eye.floor().as_ivec3()) {
                return eye;
            }
            height -= 0.1;
        }
        self.position + Vec3::Y * MIN_EYE_HEIGHT
    }

    pub fn look_direction(&self) -> Vec3 {
        Vec3::new(
            self.yaw.cos() * self.pitch.cos(),
            self.pitch.sin(),
            self.yaw.sin() * self.pitch.cos(),
        )
        .normalize()
    }

    fn move_axis(&mut self, world: &World, displacement: Vec3, axis: usize) {
        if displacement.length_squared() == 0.0 {
            return;
        }

        self.position += displacement;
        if self.collides(world) {
            self.position -= displacement;
            if axis == 1 {
                self.on_ground = self.velocity.y < 0.0;
            }
            self.velocity[axis] = 0.0;
        } else if axis == 1 {
            self.on_ground = false;
        }
    }

    pub fn bounds(&self) -> (Vec3, Vec3) {
        (
            Vec3::new(
                self.position.x - PLAYER_RADIUS,
                self.position.y,
                self.position.z - PLAYER_RADIUS,
            ),
            Vec3::new(
                self.position.x + PLAYER_RADIUS,
                self.position.y + PLAYER_HEIGHT,
                self.position.z + PLAYER_RADIUS,
            ),
        )
    }

    pub fn yaw(&self) -> f32 {
        self.yaw
    }

    pub fn pitch(&self) -> f32 {
        self.pitch
    }

    #[cfg(test)]
    pub fn set_position(&mut self, position: Vec3) {
        self.position = position;
    }

    fn collides(&self, world: &World) -> bool {
        let (min, max) = self.bounds();

        for y in min.y.floor() as i32..=max.y.floor() as i32 {
            for z in min.z.floor() as i32..=max.z.floor() as i32 {
                for x in min.x.floor() as i32..=max.x.floor() as i32 {
                    if world.is_solid(IVec3::new(x, y, z)) {
                        return true;
                    }
                }
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::{Player, EYE_HEIGHT, MIN_EYE_HEIGHT, VOID_LEVEL};
    use crate::input::Input;
    use crate::world::{Block, World};
    use glam::{IVec3, Vec3};

    #[test]
    fn eye_sits_at_full_height_when_nothing_is_above() {
        let mut world = World::generate(41);
        for y in 25..32 {
            world.set(IVec3::new(4, y, 4), Block::Air);
        }
        let mut player = Player::new(Vec3::new(4.5, 25.0, 4.5));
        player.set_position(Vec3::new(4.5, 25.0, 4.5));

        let eye = player.eye_position(&world);
        assert!((eye.y - (25.0 + EYE_HEIGHT)).abs() < 1e-5);
    }

    #[test]
    fn falling_out_of_the_world_puts_the_player_back_on_their_own_column() {
        let world = World::generate(43);
        let column = Vec3::new(9.5, VOID_LEVEL - 5.0, 6.5);
        let mut player = Player::new(column);
        player.set_position(column);

        player.update(&mut Input::default(), &world, 0.016);

        let ground = world
            .highest_block(9, 6)
            .expect("column should have ground");
        assert_eq!(player.position.x, 9.5, "must stay on the same column");
        assert_eq!(player.position.z, 6.5, "must stay on the same column");
        assert!(
            player.position.y > ground as f32,
            "must come back above the ground, not inside it"
        );
    }

    #[test]
    fn eye_ducks_under_a_low_ceiling() {
        let mut world = World::generate(42);
        for y in 25..32 {
            world.set(IVec3::new(4, y, 4), Block::Air);
        }
        // ceiling two blocks up: body (1.8) fits, eyes (2.0) don't
        world.set(IVec3::new(4, 27, 4), Block::Stone);

        let mut player = Player::new(Vec3::new(4.5, 25.0, 4.5));
        player.set_position(Vec3::new(4.5, 25.0, 4.5));

        let eye = player.eye_position(&world);
        assert!(
            !world.is_solid(eye.floor().as_ivec3()),
            "camera must never end up inside a block"
        );
        assert!(eye.y >= 25.0 + MIN_EYE_HEIGHT);
        assert!(eye.y < 25.0 + EYE_HEIGHT, "the camera should have ducked");
    }
}
