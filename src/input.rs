use std::collections::HashSet;

#[derive(Default)]
pub struct Input {
    pressed: HashSet<String>,
    mouse_dx: f32,
    mouse_dy: f32,
    /// Analog movement from the on-screen stick: x strafes right, y walks forward.
    move_axis: (f32, f32),
    /// Analog look rate from the on-screen stick: x turns right, y looks down.
    look_axis: (f32, f32),
    break_requested: bool,
    place_requested: bool,
    regenerate_requested: bool,
}

/// Keeps an analog stick inside the unit circle so diagonals are not faster.
fn clamp_to_unit_circle(x: f32, y: f32) -> (f32, f32) {
    let length = (x * x + y * y).sqrt();
    if length > 1.0 {
        (x / length, y / length)
    } else {
        (x, y)
    }
}

impl Input {
    pub fn set_key(&mut self, code: String, pressed: bool) {
        if pressed {
            self.pressed.insert(code);
        } else {
            self.pressed.remove(&code);
        }
    }

    pub fn is_pressed(&self, code: &str) -> bool {
        self.pressed.contains(code)
    }

    pub fn is_moving(&self) -> bool {
        if self.move_axis != (0.0, 0.0) {
            return true;
        }
        [
            "KeyW",
            "KeyA",
            "KeyS",
            "KeyD",
            "ArrowUp",
            "ArrowDown",
            "ArrowLeft",
            "ArrowRight",
        ]
        .iter()
        .any(|code| self.is_pressed(code))
    }

    pub fn clear(&mut self) {
        self.pressed.clear();
        self.mouse_dx = 0.0;
        self.mouse_dy = 0.0;
        self.move_axis = (0.0, 0.0);
        self.look_axis = (0.0, 0.0);
        self.break_requested = false;
        self.place_requested = false;
        self.regenerate_requested = false;
    }

    pub fn set_move_axis(&mut self, x: f32, y: f32) {
        self.move_axis = clamp_to_unit_circle(x, y);
    }

    pub fn move_axis(&self) -> (f32, f32) {
        self.move_axis
    }

    pub fn set_look_axis(&mut self, x: f32, y: f32) {
        self.look_axis = clamp_to_unit_circle(x, y);
    }

    pub fn look_axis(&self) -> (f32, f32) {
        self.look_axis
    }

    pub fn add_mouse_motion(&mut self, dx: f32, dy: f32) {
        self.mouse_dx += dx;
        self.mouse_dy += dy;
    }

    pub fn take_mouse_motion(&mut self) -> (f32, f32) {
        let motion = (self.mouse_dx, self.mouse_dy);
        self.mouse_dx = 0.0;
        self.mouse_dy = 0.0;
        motion
    }

    pub fn request_break(&mut self) {
        self.break_requested = true;
    }

    pub fn take_break_request(&mut self) -> bool {
        std::mem::take(&mut self.break_requested)
    }

    pub fn request_place(&mut self) {
        self.place_requested = true;
    }

    pub fn take_place_request(&mut self) -> bool {
        std::mem::take(&mut self.place_requested)
    }

    pub fn request_regeneration(&mut self) {
        self.regenerate_requested = true;
    }

    pub fn take_regeneration_request(&mut self) -> bool {
        std::mem::take(&mut self.regenerate_requested)
    }
}

#[cfg(test)]
mod tests {
    use super::Input;

    #[test]
    fn clear_releases_keys_and_pending_actions() {
        let mut input = Input::default();
        input.set_key("KeyW".to_owned(), true);
        input.set_move_axis(0.4, -0.6);
        input.set_look_axis(0.2, 0.1);
        input.add_mouse_motion(3.0, -2.0);
        input.request_break();
        input.request_place();
        input.request_regeneration();

        input.clear();

        assert!(!input.is_pressed("KeyW"));
        assert_eq!(input.move_axis(), (0.0, 0.0));
        assert_eq!(input.look_axis(), (0.0, 0.0));
        assert_eq!(input.take_mouse_motion(), (0.0, 0.0));
        assert!(!input.take_break_request());
        assert!(!input.take_place_request());
        assert!(!input.take_regeneration_request());
    }

    #[test]
    fn analog_sticks_stay_inside_the_unit_circle() {
        let mut input = Input::default();
        input.set_move_axis(3.0, 4.0);

        let (x, y) = input.move_axis();
        assert!(((x * x + y * y).sqrt() - 1.0).abs() < 1e-5);
        assert!((x - 0.6).abs() < 1e-5);
        assert!((y - 0.8).abs() < 1e-5);

        input.set_move_axis(0.3, -0.4);
        assert_eq!(input.move_axis(), (0.3, -0.4));
    }

    #[test]
    fn analog_movement_counts_as_moving() {
        let mut input = Input::default();
        assert!(!input.is_moving());

        input.set_move_axis(0.0, 0.5);
        assert!(input.is_moving());
    }
}
