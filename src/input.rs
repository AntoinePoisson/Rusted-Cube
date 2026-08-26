use std::collections::HashSet;

#[derive(Default)]
pub struct Input {
    pressed: HashSet<String>,
    mouse_dx: f32,
    mouse_dy: f32,
    break_requested: bool,
    place_requested: bool,
    regenerate_requested: bool,
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
        self.break_requested = false;
        self.place_requested = false;
        self.regenerate_requested = false;
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
        input.add_mouse_motion(3.0, -2.0);
        input.request_break();
        input.request_place();
        input.request_regeneration();

        input.clear();

        assert!(!input.is_pressed("KeyW"));
        assert_eq!(input.take_mouse_motion(), (0.0, 0.0));
        assert!(!input.take_break_request());
        assert!(!input.take_place_request());
        assert!(!input.take_regeneration_request());
    }
}
