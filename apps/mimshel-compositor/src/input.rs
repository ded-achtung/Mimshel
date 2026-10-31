use crate::state::{Drag, State};
use smithay::reexports::wayland_server::Resource;
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
    pub fn input<I: InputBackend>(&mut self, event: InputEvent<I>, size: Size<i32, Logical>) {
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
                if let Some(drag) = self.drag.clone() {
                    self.update_drag(drag);
                }
                let under = if self.drag.is_some() {
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
                    && let Some(drag) = self.drag.take()
                {
                    if drag.edges.is_some() {
                        let top = drag.window.toplevel().unwrap();
                        top.with_pending_state(|s| { s.states.unset(smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Resizing); });
                        top.send_pending_configure();
                    }
                    if !pointer.is_grabbed() {
                        return;
                    }
                }
                if event.state() == ButtonState::Pressed && !pointer.is_grabbed() {
                    let hit = self
                        .space
                        .element_under(self.pointer)
                        .map(|(w, p)| (w.clone(), p));
                    if let Some((window, _)) = hit {
                        if let Some((id, _)) = self.windows.iter().find(|(_, w)| w == &window) {
                            self.model.raise(*id);
                        }
                        self.space.raise_element(&window, true);
                        if event.button_code() == 0x110 && keyboard.modifier_state().logo {
                            let origin = self.space.element_location(&window).unwrap();
                            self.drag = Some(Drag {
                                size: (window.geometry().size.w, window.geometry().size.h),
                                edges: None,
                                window,
                                start: self.pointer,
                                origin,
                            });
                        }
                    }
                    self.sync_focus();
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
    pub fn host_focus_lost(&mut self) {
        // Smithay's winit adapter discards synthetic key releases on focus loss.
        let keyboard = self.seat.get_keyboard().unwrap();
        for key in keyboard.pressed_keys() {
            keyboard.input::<(), _>(
                self,
                key,
                smithay::backend::input::KeyState::Released,
                SERIAL_COUNTER.next_serial(),
                0,
                |_, _, _| FilterResult::Forward,
            );
        }
        self.drag = None;
        self.pointer_inside = false;
    }
    pub fn client_drag(
        &mut self,
        surface: smithay::wayland::shell::xdg::ToplevelSurface,
        seat: smithay::reexports::wayland_server::protocol::wl_seat::WlSeat,
        serial: smithay::utils::Serial,
        edges: Option<
            smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge,
        >,
    ) {
        if smithay::input::Seat::from_resource(&seat).as_ref() != Some(&self.seat) {
            return;
        }
        let pointer = self.seat.get_pointer().unwrap();
        if !pointer.has_grab(serial) {
            return;
        }
        let Some(start) = pointer.grab_start_data() else {
            return;
        };
        if !start
            .focus
            .as_ref()
            .is_some_and(|(s, _)| s.id().same_client_as(&surface.wl_surface().id()))
        {
            return;
        }
        if let Some((_, window)) = self
            .windows
            .iter()
            .find(|(_, w)| w.toplevel() == Some(&surface))
        {
            self.drag = Some(Drag {
                window: window.clone(),
                origin: self.space.element_location(window).unwrap(),
                start: start.location,
                size: (window.geometry().size.w, window.geometry().size.h),
                edges,
            });
        }
    }
    fn update_drag(&mut self, drag: Drag) {
        use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::{
            ResizeEdge, State as XdgState,
        };
        let Some(id) = self
            .windows
            .iter()
            .find(|(_, w)| w == &drag.window)
            .map(|(id, _)| *id)
        else {
            self.drag = None;
            return;
        };
        let delta: smithay::utils::Point<i32, Logical> = (self.pointer - drag.start).to_i32_round();
        let mut r = crate::model::Rect {
            x: drag.origin.x,
            y: drag.origin.y,
            width: drag.size.0,
            height: drag.size.1,
        };
        if let Some(edge) = drag.edges {
            let left = matches!(
                edge,
                ResizeEdge::Left | ResizeEdge::TopLeft | ResizeEdge::BottomLeft
            );
            let top = matches!(
                edge,
                ResizeEdge::Top | ResizeEdge::TopLeft | ResizeEdge::TopRight
            );
            let right = matches!(
                edge,
                ResizeEdge::Right | ResizeEdge::TopRight | ResizeEdge::BottomRight
            );
            let bottom = matches!(
                edge,
                ResizeEdge::Bottom | ResizeEdge::BottomLeft | ResizeEdge::BottomRight
            );
            if left {
                r.width = (r.width - delta.x).max(1);
                r.x += drag.size.0 - r.width;
            }
            if right {
                r.width = (r.width + delta.x).max(1);
            }
            if top {
                let dy = delta.y.max(-r.y).min(r.height - 1);
                r.y += dy;
                r.height -= dy;
            }
            if bottom {
                r.height = (r.height + delta.y).max(1);
            }
            drag.window.toplevel().unwrap().with_pending_state(|s| {
                s.size = Some((r.width, r.height).into());
                s.states.set(XdgState::Resizing);
            });
            drag.window.toplevel().unwrap().send_pending_configure();
        } else {
            r.x += delta.x;
            r.y += delta.y;
        }
        self.model.geometry(id, r);
        self.sync_positions();
    }
}
