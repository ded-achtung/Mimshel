use crate::state::{Drag, State};
use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent,
        KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
    },
    input::{
        keyboard::FilterResult,
        pointer::{AxisFrame, ButtonEvent, MotionEvent},
    },
    utils::{Logical, SERIAL_COUNTER, Size},
};

impl State {
    pub fn input<I: InputBackend>(
        &mut self,
        event: InputEvent<I>,
        size: Size<i32, Logical>,
        dock: bool,
    ) {
        let pointer = self.seat.get_pointer().unwrap();
        let keyboard = self.seat.get_keyboard().unwrap();
        match event {
            InputEvent::Keyboard { event, .. } => {
                keyboard.input::<(), _>(
                    self,
                    event.key_code(),
                    event.state(),
                    SERIAL_COUNTER.next_serial(),
                    event.time_msec(),
                    |_, _, _| FilterResult::Forward,
                );
            }
            InputEvent::PointerMotionAbsolute { event, .. } => {
                self.pointer = event.position_transformed(size);
                self.pointer_inside = true;
                if let Some(Drag {
                    window,
                    start,
                    origin,
                }) = self.drag.clone()
                {
                    let delta = (self.pointer - start).to_i32_round();
                    self.space.map_element(window, origin + delta, true);
                }
                let under = if self.drag.is_some() || self.over_dock(size, dock) {
                    None
                } else {
                    self.surface_under()
                };
                pointer.motion(
                    self,
                    under,
                    &MotionEvent {
                        location: self.pointer,
                        serial: SERIAL_COUNTER.next_serial(),
                        time: event.time_msec(),
                    },
                );
                pointer.frame(self);
            }
            InputEvent::PointerButton { event, .. } => {
                let serial = SERIAL_COUNTER.next_serial();
                if event.button_code() == 0x110
                    && event.state() == ButtonState::Released
                    && self.drag.take().is_some()
                {
                    return;
                }
                if event.state() == ButtonState::Pressed && !pointer.is_grabbed() {
                    let hit = if self.over_dock(size, dock) {
                        None
                    } else {
                        self.space
                            .element_under(self.pointer)
                            .map(|(w, p)| (w.clone(), p))
                    };
                    let focus = hit
                        .as_ref()
                        .and_then(|(w, _)| w.toplevel().map(|t| t.wl_surface().clone()));
                    if let Some((window, origin)) = hit {
                        self.space.raise_element(&window, true);
                        if event.button_code() == 0x110 && keyboard.modifier_state().logo {
                            self.drag = Some(Drag {
                                window,
                                start: self.pointer,
                                origin,
                            });
                        }
                    } else {
                        for w in self.space.elements() {
                            w.set_activated(false);
                        }
                    }
                    keyboard.set_focus(self, focus, serial);
                    for w in self.space.elements() {
                        w.toplevel().unwrap().send_pending_configure();
                    }
                    if self.drag.is_some() {
                        return;
                    }
                }
                pointer.button(
                    self,
                    &ButtonEvent {
                        button: event.button_code(),
                        state: event.state(),
                        serial,
                        time: event.time_msec(),
                    },
                );
                pointer.frame(self);
            }
            InputEvent::PointerAxis { event, .. } => {
                let mut frame = AxisFrame::new(event.time_msec()).source(event.source());
                for axis in [Axis::Horizontal, Axis::Vertical] {
                    let discrete = event.amount_v120(axis);
                    let amount = event
                        .amount(axis)
                        .unwrap_or_else(|| discrete.unwrap_or(0.0) * 15.0 / 120.0);
                    if amount != 0.0 {
                        frame = frame.value(axis, amount);
                    }
                    if let Some(steps) = discrete {
                        frame = frame.v120(axis, steps as i32);
                    }
                    if event.source() == AxisSource::Finger && event.amount(axis) == Some(0.0) {
                        frame = frame.stop(axis);
                    }
                }
                pointer.axis(self, frame);
                pointer.frame(self);
            }
            _ => {}
        }
    }
    fn over_dock(&self, size: Size<i32, Logical>, enabled: bool) -> bool {
        enabled
            && self.pointer.y > f64::from(size.h) - 125.0
            && (self.pointer.x - f64::from(size.w) / 2.0).abs() < 400.0
    }
}
