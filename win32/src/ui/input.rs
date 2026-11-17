//! Unified pointer input (`WM_POINTER*`), precision touchpad / wheel delta
//! accumulation, pointer capture tracking, and keyboard event mapping.

use super::tree::WidgetId;

/// Standard Windows `WHEEL_DELTA` per detent.
pub const WHEEL_DELTA: f32 = 120.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PointerKind {
    Mouse,
    Touch,
    Pen,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PointerButton {
    Primary,
    Secondary,
    Other,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[allow(dead_code)] // keys dispatched across widgets and screens
pub enum KeyCode {
    Tab,
    Escape,
    Enter,
    Space,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Other(u16),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[allow(dead_code)]
pub struct KeyEvent {
    pub key: KeyCode,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

/// Accumulates fractional `WM_MOUSEWHEEL` / `WM_MOUSEHWHEEL` (and `WM_POINTER*WHEEL`)
/// deltas so precision touchpads never lose sub-notch movement.
#[derive(Clone, Copy, Debug, Default)]
pub struct WheelAccumulator {
    rem_x: f32,
    rem_y: f32,
}

#[allow(dead_code)]
impl WheelAccumulator {
    /// Adds a vertical wheel `delta` (positive = wheel rotated forward / scroll up).
    /// Returns the number of discrete integer line/notch steps accumulated, keeping
    /// the fractional remainder for subsequent events.
    pub fn push_vertical_steps(&mut self, delta: i16) -> i32 {
        self.rem_y += delta as f32;
        let steps = (self.rem_y / WHEEL_DELTA).trunc() as i32;
        self.rem_y -= steps as f32 * WHEEL_DELTA;
        steps
    }

    /// Adds a horizontal wheel `delta` (positive = scroll right).
    pub fn push_horizontal_steps(&mut self, delta: i16) -> i32 {
        self.rem_x += delta as f32;
        let steps = (self.rem_x / WHEEL_DELTA).trunc() as i32;
        self.rem_x -= steps as f32 * WHEEL_DELTA;
        steps
    }

    /// Converts a vertical `delta` into smooth DIP scroll delta (positive = scroll up,
    /// so caller subtracts from `scroll_y` or adds `-dy`), retaining sub-0.1 DIP remainder.
    pub fn push_vertical_dip(&mut self, delta: i16, dip_per_notch: f32) -> f32 {
        self.rem_y += (delta as f32 / WHEEL_DELTA) * dip_per_notch;
        let whole = (self.rem_y * 10.0).trunc() / 10.0;
        self.rem_y -= whole;
        whole
    }

    /// Converts a horizontal `delta` into smooth DIP scroll delta (positive = scroll right).
    pub fn push_horizontal_dip(&mut self, delta: i16, dip_per_notch: f32) -> f32 {
        self.rem_x += (delta as f32 / WHEEL_DELTA) * dip_per_notch;
        let whole = (self.rem_x * 10.0).trunc() / 10.0;
        self.rem_x -= whole;
        whole
    }

    pub fn reset(&mut self) {
        self.rem_x = 0.0;
        self.rem_y = 0.0;
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct PointerOutcome {
    pub visual_changed: bool,
    pub clicked: Option<WidgetId>,
    pub request_capture: bool,
    pub release_capture: bool,
}

/// Client-area pointer interaction state.
#[derive(Debug, Default)]
pub struct PointerState {
    pub hovered: Option<WidgetId>,
    pub pressed: Option<WidgetId>,
    pub captured_pointer: Option<u32>,
    pub last_dip: Option<(f32, f32)>,
    pub wheel: WheelAccumulator,
}

#[allow(dead_code)]
impl PointerState {
    pub fn on_move(&mut self, hit: Option<WidgetId>, x_dip: f32, y_dip: f32) -> PointerOutcome {
        self.last_dip = Some((x_dip, y_dip));
        let next_hover = if self.pressed.is_some() {
            // While captured, hover tracks whether the pointer stays over the pressed target.
            if hit == self.pressed {
                hit
            } else {
                None
            }
        } else {
            hit
        };
        let visual_changed = self.hovered != next_hover;
        self.hovered = next_hover;
        PointerOutcome {
            visual_changed,
            ..Default::default()
        }
    }

    pub fn on_down(
        &mut self,
        pointer_id: u32,
        button: PointerButton,
        hit: Option<WidgetId>,
        x_dip: f32,
        y_dip: f32,
    ) -> PointerOutcome {
        self.last_dip = Some((x_dip, y_dip));
        if button != PointerButton::Primary {
            return PointerOutcome::default();
        }
        let changed = self.pressed != hit || self.hovered != hit;
        self.pressed = hit;
        self.hovered = hit;
        if hit.is_some() {
            self.captured_pointer = Some(pointer_id);
        }
        PointerOutcome {
            visual_changed: changed,
            request_capture: hit.is_some(),
            ..Default::default()
        }
    }

    pub fn on_up(
        &mut self,
        pointer_id: u32,
        button: PointerButton,
        hit: Option<WidgetId>,
        x_dip: f32,
        y_dip: f32,
    ) -> PointerOutcome {
        self.last_dip = Some((x_dip, y_dip));
        if button != PointerButton::Primary {
            return PointerOutcome::default();
        }
        let was_captured = self.captured_pointer == Some(pointer_id);
        let clicked = match self.pressed {
            Some(p) if hit == Some(p) => Some(p),
            _ => None,
        };
        let changed = self.pressed.is_some() || self.hovered != hit;
        self.pressed = None;
        self.captured_pointer = None;
        self.hovered = hit;
        PointerOutcome {
            visual_changed: changed,
            clicked,
            release_capture: was_captured,
            ..Default::default()
        }
    }

    pub fn on_leave(&mut self) -> PointerOutcome {
        if self.captured_pointer.is_some() {
            return PointerOutcome::default();
        }
        let changed = self.hovered.is_some();
        self.hovered = None;
        PointerOutcome {
            visual_changed: changed,
            ..Default::default()
        }
    }

    pub fn on_capture_lost(&mut self) -> PointerOutcome {
        let changed = self.pressed.is_some() || self.captured_pointer.is_some();
        self.pressed = None;
        self.captured_pointer = None;
        PointerOutcome {
            visual_changed: changed,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wheel_accumulator_preserves_fractional_precision_touchpad_deltas() {
        let mut w = WheelAccumulator::default();
        assert_eq!(w.push_vertical_steps(30), 0);
        assert_eq!(w.push_vertical_steps(30), 0);
        assert_eq!(w.push_vertical_steps(30), 0);
        assert_eq!(w.push_vertical_steps(30), 1);

        assert_eq!(w.push_horizontal_steps(-40), 0);
        assert_eq!(w.push_horizontal_steps(-80), -1);

        // Smooth DIP mode preserves sub-notch fractions cleanly.
        let mut s = 0.0_f32;
        for _ in 0..8 {
            s += w.push_vertical_dip(15, 48.0);
        }
        assert!((s - 48.0).abs() < 0.05);
    }

    #[test]
    fn pointer_state_clicks_only_on_matching_press_and_release_and_handles_capture_loss() {
        let mut ps = PointerState::default();
        let a = WidgetId(10);
        let b = WidgetId(11);

        let down = ps.on_down(1, PointerButton::Primary, Some(a), 10.0, 10.0);
        assert!(down.request_capture);
        assert!(down.visual_changed);

        // Dragging onto `b` and releasing does not click `a` or `b`.
        ps.on_move(Some(b), 50.0, 10.0);
        assert_eq!(ps.hovered, None);
        let up = ps.on_up(1, PointerButton::Primary, Some(b), 50.0, 10.0);
        assert_eq!(up.clicked, None);
        assert!(up.release_capture);

        // Press and release on `a` clicks `a`.
        ps.on_down(2, PointerButton::Primary, Some(a), 10.0, 10.0);
        let up_hit = ps.on_up(2, PointerButton::Primary, Some(a), 12.0, 12.0);
        assert_eq!(up_hit.clicked, Some(a));

        // Capture stolen cancels press without clicking.
        ps.on_down(3, PointerButton::Primary, Some(a), 10.0, 10.0);
        let lost = ps.on_capture_lost();
        assert!(lost.visual_changed);
        assert_eq!(ps.pressed, None);
        assert_eq!(ps.captured_pointer, None);
    }
}
