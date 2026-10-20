//! Pointer/wheel interaction as a pure state machine.
//!
//! Hosts forward DOM event data (CSS pixel offsets relative to the canvas,
//! timestamps in milliseconds) as [`InputEvent`]s; [`Input::handle`] updates a
//! [`View`] and reports what happened. No DOM listeners live here, so the
//! logic is unit-testable natively.

use crate::view::View;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InputEvent {
    PointerDown {
        id: i32,
        x: f64,
        y: f64,
        button: i16,
        time_ms: f64,
    },
    PointerMove {
        id: i32,
        x: f64,
        y: f64,
    },
    PointerUp {
        id: i32,
        x: f64,
        y: f64,
        button: i16,
        time_ms: f64,
    },
    PointerCancel {
        id: i32,
    },
    /// `delta_mode` follows `WheelEvent.deltaMode`: 0 pixels, 1 lines, 2 pages.
    Wheel {
        x: f64,
        y: f64,
        dx: f64,
        dy: f64,
        delta_mode: u32,
        ctrl: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InputOutcome {
    /// Nothing visible changed.
    None,
    /// The view transform changed; redraw.
    ViewChanged,
    /// A short, nearly stationary press: select whatever is at `(x, y)`.
    Tap { x: f64, y: f64, button: i16 },
    /// Right-click: the host should refit (or otherwise reset) the view.
    Reset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WheelMode {
    /// Every wheel event zooms about the cursor.
    Zoom,
    /// Plain wheel/trackpad scroll pans; ctrl+wheel (and trackpad pinch) zooms.
    PanUnlessCtrl,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InputConfig {
    pub wheel: WheelMode,
    /// A press travelling less than this many CSS pixels counts as a tap...
    pub tap_max_travel: f64,
    /// ...if released within this many milliseconds.
    pub tap_max_ms: f64,
    /// Emit [`InputOutcome::Reset`] when the right button is released.
    pub reset_on_right_click: bool,
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            wheel: WheelMode::Zoom,
            tap_max_travel: 10.0,
            tap_max_ms: 500.0,
            reset_on_right_click: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Pointer {
    id: i32,
    x: f64,
    y: f64,
    travelled: f64,
    down_ms: f64,
}

#[derive(Debug, Clone, Default)]
pub struct Input {
    pub config: InputConfig,
    pointers: Vec<Pointer>,
}

/// Zoom factor for a wheel delta, matching the iBOM viewer's feel.
pub fn wheel_zoom_factor(dy: f64, delta_mode: u32) -> f64 {
    let px = match delta_mode {
        1 => dy * 30.0,
        2 => dy * 300.0,
        _ => dy,
    };
    1.1f64.powf(-px / 40.0).clamp(0.5, 2.0)
}

impl Input {
    pub fn new(config: InputConfig) -> Self {
        Self {
            config,
            pointers: Vec::new(),
        }
    }

    /// Pointers currently held down.
    pub fn active_pointers(&self) -> usize {
        self.pointers.len()
    }

    /// True while a drag or pinch is in progress.
    pub fn is_dragging(&self) -> bool {
        self.pointers
            .iter()
            .any(|p| p.travelled >= self.config.tap_max_travel)
    }

    pub fn handle(&mut self, ev: &InputEvent, view: &mut View) -> InputOutcome {
        match *ev {
            InputEvent::PointerDown {
                id, x, y, time_ms, ..
            } => {
                self.pointers.retain(|p| p.id != id);
                self.pointers.push(Pointer {
                    id,
                    x,
                    y,
                    travelled: 0.0,
                    down_ms: time_ms,
                });
                InputOutcome::None
            }
            InputEvent::PointerMove { id, x, y } => self.on_move(id, x, y, view),
            InputEvent::PointerUp {
                id,
                x,
                y,
                button,
                time_ms,
            } => {
                let Some(idx) = self.pointers.iter().position(|p| p.id == id) else {
                    return InputOutcome::None;
                };
                let p = self.pointers.remove(idx);
                if button == 2 && self.config.reset_on_right_click {
                    return InputOutcome::Reset;
                }
                let travelled = p.travelled + ((x - p.x).powi(2) + (y - p.y).powi(2)).sqrt();
                let quick = time_ms - p.down_ms <= self.config.tap_max_ms;
                // A finger lifted from a pinch is never a tap.
                if travelled < self.config.tap_max_travel && quick && self.pointers.is_empty() {
                    InputOutcome::Tap { x, y, button }
                } else {
                    InputOutcome::None
                }
            }
            InputEvent::PointerCancel { id } => {
                self.pointers.retain(|p| p.id != id);
                InputOutcome::None
            }
            InputEvent::Wheel {
                x,
                y,
                dx,
                dy,
                delta_mode,
                ctrl,
            } => {
                if self.config.wheel == WheelMode::Zoom || ctrl {
                    view.zoom_at(x, y, wheel_zoom_factor(dy, delta_mode));
                } else {
                    let k = match delta_mode {
                        1 => 30.0,
                        2 => 300.0,
                        _ => 1.0,
                    };
                    view.pan(-dx * k, -dy * k);
                }
                InputOutcome::ViewChanged
            }
        }
    }

    fn on_move(&mut self, id: i32, x: f64, y: f64, view: &mut View) -> InputOutcome {
        let Some(idx) = self.pointers.iter().position(|p| p.id == id) else {
            return InputOutcome::None;
        };
        let cur = self.pointers[idx];
        let (dx, dy) = (x - cur.x, y - cur.y);
        let moved = (dx * dx + dy * dy).sqrt();

        let outcome = match self.pointers.len() {
            1 => {
                view.pan(dx, dy);
                InputOutcome::ViewChanged
            }
            2 => {
                let other = self.pointers[1 - idx];
                let old_mid = ((cur.x + other.x) / 2.0, (cur.y + other.y) / 2.0);
                let new_mid = ((x + other.x) / 2.0, (y + other.y) / 2.0);
                let old_dist = ((cur.x - other.x).powi(2) + (cur.y - other.y).powi(2)).sqrt();
                let new_dist = ((x - other.x).powi(2) + (y - other.y).powi(2)).sqrt();
                view.pan(new_mid.0 - old_mid.0, new_mid.1 - old_mid.1);
                if old_dist > 1e-6 && new_dist > 1e-6 {
                    view.zoom_at(new_mid.0, new_mid.1, new_dist / old_dist);
                }
                InputOutcome::ViewChanged
            }
            // Three or more fingers: ignore movement but keep tracking.
            _ => InputOutcome::None,
        };

        let p = &mut self.pointers[idx];
        p.x = x;
        p.y = y;
        p.travelled += moved;
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn down(id: i32, x: f64, y: f64, t: f64) -> InputEvent {
        InputEvent::PointerDown {
            id,
            x,
            y,
            button: 0,
            time_ms: t,
        }
    }

    fn up(id: i32, x: f64, y: f64, t: f64) -> InputEvent {
        InputEvent::PointerUp {
            id,
            x,
            y,
            button: 0,
            time_ms: t,
        }
    }

    fn mv(id: i32, x: f64, y: f64) -> InputEvent {
        InputEvent::PointerMove { id, x, y }
    }

    #[test]
    fn short_press_is_a_tap() {
        let mut input = Input::default();
        let mut v = View::default();
        assert_eq!(
            input.handle(&down(1, 10.0, 10.0, 0.0), &mut v),
            InputOutcome::None
        );
        input.handle(&mv(1, 12.0, 11.0), &mut v);
        assert_eq!(
            input.handle(&up(1, 12.0, 11.0, 100.0), &mut v),
            InputOutcome::Tap {
                x: 12.0,
                y: 11.0,
                button: 0
            }
        );
        assert_eq!(input.active_pointers(), 0);
    }

    #[test]
    fn slow_or_long_press_is_not_a_tap() {
        let mut input = Input::default();
        let mut v = View::default();
        input.handle(&down(1, 0.0, 0.0, 0.0), &mut v);
        assert_eq!(
            input.handle(&up(1, 0.0, 0.0, 900.0), &mut v),
            InputOutcome::None
        );
        input.handle(&down(1, 0.0, 0.0, 0.0), &mut v);
        input.handle(&mv(1, 30.0, 0.0), &mut v);
        input.handle(&mv(1, 0.0, 0.0), &mut v);
        assert_eq!(
            input.handle(&up(1, 0.0, 0.0, 50.0), &mut v),
            InputOutcome::None
        );
    }

    #[test]
    fn drag_pans() {
        let mut input = Input::default();
        let mut v = View::default();
        input.handle(&down(1, 0.0, 0.0, 0.0), &mut v);
        assert_eq!(
            input.handle(&mv(1, 15.0, -5.0), &mut v),
            InputOutcome::ViewChanged
        );
        assert!(input.is_dragging());
        assert_eq!((v.tx, v.ty), (15.0, -5.0));
        // Moves without a pressed pointer are hover, not drag.
        input.handle(&up(1, 15.0, -5.0, 10.0), &mut v);
        assert_eq!(input.handle(&mv(1, 50.0, 50.0), &mut v), InputOutcome::None);
        assert_eq!((v.tx, v.ty), (15.0, -5.0));
    }

    #[test]
    fn pinch_zooms_about_midpoint() {
        let mut input = Input::default();
        let mut v = View::default();
        input.handle(&down(1, 100.0, 100.0, 0.0), &mut v);
        input.handle(&down(2, 200.0, 100.0, 0.0), &mut v);
        let fixed = v.to_world([200.0, 100.0]);
        // Spread finger 1 outward: distance 100 -> 200, midpoint 150 -> 100.
        input.handle(&mv(1, 0.0, 100.0), &mut v);
        assert!((v.scale - 2.0).abs() < 1e-9);
        // Finger 2 did not move, so the point under it is unchanged.
        let now = v.to_world([200.0, 100.0]);
        assert!((now[0] - fixed[0]).abs() < 1e-9 && (now[1] - fixed[1]).abs() < 1e-9);
        // Lifting one finger of a pinch is not a tap.
        assert_eq!(
            input.handle(&up(2, 200.0, 100.0, 50.0), &mut v),
            InputOutcome::None
        );
    }

    #[test]
    fn wheel_zooms_or_pans_by_mode() {
        let mut input = Input::default();
        let mut v = View::default();
        let wheel = |ctrl| InputEvent::Wheel {
            x: 50.0,
            y: 50.0,
            dx: 0.0,
            dy: -40.0,
            delta_mode: 0,
            ctrl,
        };
        input.handle(&wheel(false), &mut v);
        assert!((v.scale - 1.1).abs() < 1e-9);

        input.config.wheel = WheelMode::PanUnlessCtrl;
        let before = v.scale;
        input.handle(&wheel(false), &mut v);
        assert_eq!(v.scale, before);
        assert!(v.ty > 0.0);
        input.handle(&wheel(true), &mut v);
        assert!(v.scale > before);
    }

    #[test]
    fn wheel_factor_is_clamped_and_mode_scaled() {
        assert_eq!(wheel_zoom_factor(-10_000.0, 0), 2.0);
        assert_eq!(wheel_zoom_factor(10_000.0, 0), 0.5);
        assert!((wheel_zoom_factor(1.0, 1) - wheel_zoom_factor(30.0, 0)).abs() < 1e-12);
    }

    #[test]
    fn right_click_resets_and_cancel_forgets() {
        let mut input = Input::default();
        let mut v = View::default();
        input.handle(
            &InputEvent::PointerDown {
                id: 3,
                x: 0.0,
                y: 0.0,
                button: 2,
                time_ms: 0.0,
            },
            &mut v,
        );
        let out = input.handle(
            &InputEvent::PointerUp {
                id: 3,
                x: 0.0,
                y: 0.0,
                button: 2,
                time_ms: 10.0,
            },
            &mut v,
        );
        assert_eq!(out, InputOutcome::Reset);
        input.handle(&down(4, 0.0, 0.0, 0.0), &mut v);
        input.handle(&InputEvent::PointerCancel { id: 4 }, &mut v);
        assert_eq!(input.active_pointers(), 0);
    }
}
