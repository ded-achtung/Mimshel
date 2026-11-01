//! Host input forwarding and validated client move and resize requests.

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
    /// Forward host input and update focus and interactive drags.
    pub fn input<I: InputBackend>(&mut self, event: InputEvent<I>, size: Size<i32, Logical>) {
        self.notify_activity();
        let pointer = self.pointer_handle.clone();
        let keyboard = self.keyboard.clone();
        match event {
            InputEvent::Keyboard { event, .. } => {
                keyboard.input::<(), _>(
                    self,
                    event.key_code(),
                    event.state(),
                    SERIAL_COUNTER.next_serial(),
                    event.time_msec(),
                    |state, modifiers, keys| {
                        if !state.protocols.locked
                            && state.exclusive_layer_focus().is_none()
                            && modifiers.logo
                            && keys.raw_latin_sym_or_raw_current_sym()
                                == Some(smithay::input::keyboard::Keysym::u)
                        {
                            if event.state() == smithay::backend::input::KeyState::Pressed {
                                state.model.restore_recent();
                                state.configure_windows();
                            }
                            FilterResult::Intercept(())
                        } else {
                            FilterResult::Forward
                        }
                    },
                );
            }
            InputEvent::PointerMotionAbsolute { event, .. } => {
                self.pointer = event.position_transformed(size);
                self.pointer_inside = true;
                if let Some(drag) = self.drag.clone() {
                    self.update_drag(drag);
                }
                self.update_frame_pointer();
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
                    if drag.edges.is_some()
                        && let Some(top) = drag.window.toplevel()
                    {
                        top.with_pending_state(|s| { s.states.unset(smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Resizing); });
                        top.send_pending_configure();
                    }
                    if self.decoration_pressed {
                        self.decoration_pressed = false;
                    }
                    if !pointer.is_grabbed() {
                        return;
                    }
                }
                if event.button_code() == 0x110
                    && event.state() == ButtonState::Released
                    && self.decoration_pressed
                {
                    self.decoration_pressed = false;
                    return;
                }
                if event.button_code() == 0x110
                    && event.state() == ButtonState::Pressed
                    && !pointer.is_grabbed()
                    && let Some((id, hit)) = self.decoration_under()
                {
                    self.leave_on_demand_layer();
                    self.decoration_pressed = true;
                    self.model.raise(id);
                    self.sync_positions();
                    self.sync_focus();
                    let Some((_, window)) = self.windows.iter().find(|(i, _)| *i == id) else {
                        return;
                    };
                    let window = window.clone();
                    use crate::decoration::Hit;
                    match hit {
                        Hit::Close => {
                            self.close_window(id);
                        }
                        Hit::Minimize => {
                            self.window_mode(id, crate::model::Mode::Minimized);
                        }
                        Hit::Maximize => {
                            self.toggle_maximize(id);
                        }
                        Hit::Title => {
                            let now = event.time_msec();
                            if self.title_click.is_some_and(|(last, time)| {
                                last == id && now.wrapping_sub(time) < 400
                            }) {
                                self.title_click = None;
                                self.toggle_maximize(id);
                            } else {
                                self.title_click = Some((id, now));
                                self.frame_drag(id, window, None);
                            }
                        }
                        Hit::Edge(x, y) => {
                            use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge as E;
                            let edge = match (x, y) {
                                (-1, -1) => E::TopLeft,
                                (0, -1) => E::Top,
                                (1, -1) => E::TopRight,
                                (-1, 0) => E::Left,
                                (1, 0) => E::Right,
                                (-1, 1) => E::BottomLeft,
                                (0, 1) => E::Bottom,
                                _ => E::BottomRight,
                            };
                            self.frame_drag(id, window, Some(edge));
                        }
                    }
                    return;
                }
                if event.state() == ButtonState::Pressed
                    && !pointer.is_grabbed()
                    && !self.focus_shell_click()
                {
                    self.leave_on_demand_layer();
                    let hit = self
                        .space
                        .element_under(self.pointer)
                        .map(|(w, p)| (w.clone(), p));
                    if let Some((window, _)) = hit {
                        if window
                            .x11_surface()
                            .is_some_and(|x| x.is_override_redirect())
                        {
                            pointer.button(
                                self,
                                &smithay::input::pointer::ButtonEvent {
                                    serial,
                                    time: event.time_msec(),
                                    button: event.button_code(),
                                    state: event.state(),
                                },
                            );
                            pointer.frame(self);
                            return;
                        }
                        if let Some((id, _)) = self.windows.iter().find(|(_, w)| w == &window) {
                            self.model.raise(*id);
                        }
                        self.sync_positions();
                        if event.button_code() == 0x110 && keyboard.modifier_state().logo {
                            let Some(origin) = self
                                .windows
                                .iter()
                                .find(|(_, w)| w == &window)
                                .and_then(|(id, _)| self.model.rect(*id))
                                .map(|r| (r.x, r.y).into())
                            else {
                                return;
                            };
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

    /// Toggle output-sized presentation for a frame control.
    fn toggle_maximize(&mut self, id: crate::model::WindowId) {
        let Some(w) = self.model.windows.iter().find(|w| w.id == id) else {
            return;
        };
        let mode = if w.mode == crate::model::Mode::Maximized {
            crate::model::Mode::Normal
        } else {
            crate::model::Mode::Maximized
        };
        self.window_mode(id, mode);
    }

    fn frame_drag(
        &mut self,
        id: crate::model::WindowId,
        window: smithay::desktop::Window,
        edges: Option<
            smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge,
        >,
    ) {
        let Some(record) = self.model.windows.iter().find(|w| w.id == id) else {
            return;
        };
        if record.mode != crate::model::Mode::Normal {
            return;
        }
        self.drag = Some(Drag {
            size: (record.rect.width, record.rect.height),
            origin: (record.rect.x, record.rect.y).into(),
            start: self.pointer,
            window,
            edges,
        });
    }

    /// Update hover only when the pointer enters or leaves a button group.
    pub fn update_frame_pointer(&mut self) {
        let hit = self.decoration_under();
        let hover = hit.and_then(|(id, _)| {
            self.model.rect(id).and_then(|r| {
                let (x, y) = (
                    self.pointer.x - f64::from(r.x),
                    self.pointer.y - f64::from(r.y),
                );
                ((12.0..=64.0).contains(&x) && (9.0..=21.0).contains(&y)).then_some(id)
            })
        });
        if hover != self.hovered {
            self.hovered = hover;
            self.dirty = true;
        }
        use smithay::reexports::winit::window::CursorIcon as C;
        self.cursor = match hit {
            Some((_, crate::decoration::Hit::Edge(x, y))) => match (x, y) {
                (-1, -1) | (1, 1) => C::NwseResize,
                (1, -1) | (-1, 1) => C::NeswResize,
                (0, _) => C::NsResize,
                _ => C::EwResize,
            },
            _ => C::Default,
        };
    }

    /// Release held keys and cancel interactive operations.
    pub fn host_focus_lost(&mut self) {
        // Smithay's winit adapter discards synthetic key releases on focus loss.
        let keyboard = self.keyboard.clone();
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
        self.decoration_pressed = false;
        if self.hovered.take().is_some() {
            self.dirty = true;
        }
    }

    /// Validate a client grab before beginning a move or resize.
    pub fn client_drag(
        &mut self,
        surface: smithay::wayland::shell::xdg::ToplevelSurface,
        seat: smithay::reexports::wayland_server::protocol::wl_seat::WlSeat,
        serial: smithay::utils::Serial,
        edges: Option<
            smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge,
        >,
    ) {
        if self.protocols.locked
            || smithay::input::Seat::from_resource(&seat).as_ref() != Some(&self.seat)
        {
            return;
        }
        let pointer = self.pointer_handle.clone();
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
            let Some(origin) = self
                .windows
                .iter()
                .find(|(_, w)| w == window)
                .and_then(|(id, _)| self.model.rect(*id))
                .map(|r| (r.x, r.y).into())
            else {
                return;
            };
            self.drag = Some(Drag {
                window: window.clone(),
                origin,
                start: start.location,
                size: (window.geometry().size.w, window.geometry().size.h),
                edges,
            });
        }
    }

    /// Apply pointer movement to the active window operation.
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
        if !self
            .model
            .windows
            .iter()
            .any(|w| w.id == id && w.mode == crate::model::Mode::Normal)
        {
            self.drag = None;
            return;
        }
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
            if let Some(top) = drag.window.toplevel() {
                top.with_pending_state(|s| {
                    s.size = Some((r.width, r.height).into());
                    s.states.set(XdgState::Resizing);
                });
                top.send_pending_configure();
            }
        } else {
            if delta.x.abs() > 3 || delta.y.abs() > 3 {
                self.title_click = None;
            }
            r.x += delta.x;
            r.y += delta.y;
        }
        self.model.geometry(id, r);
        self.sync_positions();
    }
}
