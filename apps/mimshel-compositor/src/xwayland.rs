//! XWayland lifecycle and X11 policy through Smithay's window manager.
use crate::{
    Error,
    model::{Mode, WindowId},
    state::{Drag, State},
};
use smithay::{
    desktop::Window,
    reexports::{calloop::LoopHandle, wayland_server::protocol::wl_surface::WlSurface},
    utils::{Logical, Rectangle},
    wayland::seat::WaylandFocus,
    wayland::xwayland_shell::{XWaylandShellHandler, XWaylandShellState},
    xwayland::{
        X11Surface, X11Wm, XWayland, XWaylandEvent, XwmHandler,
        xwm::{Reorder, ResizeEdge, WmWindowProperty, XwmId},
    },
};

/// Owned Wayland surface for either desktop window kind.
pub fn surface(window: &Window) -> Option<WlSurface> {
    window.wl_surface().map(|s| s.into_owned())
}

/// Own the event source so teardown breaks Smithay's event-loop references.
pub struct Instance {
    /// Reserved X11 display number.
    pub display: u32,
    handle: LoopHandle<'static, State>,
    token: smithay::reexports::calloop::RegistrationToken,
}
impl Drop for Instance {
    fn drop(&mut self) {
        self.handle.remove(self.token);
    }
}

/// Start eagerly and retain an explicit shutdown owner.
pub(crate) fn start(state: &State, handle: LoopHandle<'static, State>) -> Result<Instance, Error> {
    // Do not let Smithay reclaim stale locks or unlink pre-existing socket paths.
    // Its atomic lock acquisition and abstract socket bind settle live-server races.
    for number in 1..1024 {
        if std::fs::symlink_metadata(format!("/tmp/.X{number}-lock")).is_ok()
            || std::fs::symlink_metadata(format!("/tmp/.X11-unix/X{number}")).is_ok()
        {
            continue;
        }
        let result = XWayland::spawn(
            &state.dh,
            Some(number),
            std::iter::empty::<(&str, &str)>(),
            true,
            std::process::Stdio::null(),
            std::process::Stdio::inherit(),
            |_| {},
        );
        let (xwayland, client) = match result {
            Ok(result) => result,
            Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => continue,
            Err(error) => return Err(error.into()),
        };
        let wm_handle = handle.downgrade();
        let token = handle.insert_source(xwayland, move |event, _, state| match event {
            XWaylandEvent::Ready { x11_socket, .. } => {
                let Some(wm_handle) = wm_handle.upgrade() else {
                    return;
                };
                match X11Wm::start_wm(wm_handle, &state.dh, x11_socket, client.clone()) {
                    Ok(mut wm) => {
                        use smithay::wayland::selection::SelectionTarget;
                        for (index, target) in
                            [SelectionTarget::Clipboard, SelectionTarget::Primary]
                                .into_iter()
                                .enumerate()
                        {
                            if let Some(mimes) = state.wayland_selections[index].clone() {
                                let _ = wm.new_selection(target, Some(mimes));
                            }
                        }
                        state.xwms.push(wm);
                    }
                    Err(error) => eprintln!("XWayland WM: {error}"),
                }
            }
            XWaylandEvent::Error => eprintln!("XWayland exited during startup"),
        })?;
        return Ok(Instance {
            display: number,
            handle,
            token,
        });
    }
    Err("no unused X11 display number".into())
}

impl State {
    fn x11_id(&self, surface: &X11Surface) -> Option<WindowId> {
        self.windows
            .iter()
            .find(|(_, w)| w.x11_surface() == Some(surface))
            .map(|(id, _)| *id)
    }

    /// Apply a mode to either protocol's window.
    pub fn window_mode(&mut self, id: WindowId, mode: Mode) {
        if let Some(top) = self
            .windows
            .iter()
            .find(|(i, _)| *i == id)
            .and_then(|(_, w)| w.toplevel().cloned())
        {
            self.request_mode(&top, mode);
        } else {
            self.model.set_mode(id, mode);
            self.configure_windows();
        }
    }

    /// Ask either protocol's client to close.
    pub fn close_window(&self, id: WindowId) {
        if let Some((_, window)) = self.windows.iter().find(|(i, _)| *i == id) {
            if let Some(top) = window.toplevel() {
                top.send_close();
            }
            if let Some(x11) = window.x11_surface() {
                let _ = x11.close();
            }
        }
    }

    /// Synchronize X11 client geometry, EWMH states and stacking with the model.
    pub(crate) fn sync_x11_positions(&mut self) {
        for record in &self.model.windows {
            if let Some(x11) = self
                .windows
                .iter()
                .find(|(id, _)| *id == record.id)
                .and_then(|(_, w)| w.x11_surface())
            {
                let rect = Rectangle::new(
                    (
                        record.rect.x,
                        record.rect.y + self.model.title_height(record.id),
                    )
                        .into(),
                    (record.rect.width, record.rect.height).into(),
                );
                if x11.geometry() != rect {
                    let _ = x11.configure(rect);
                }
                if x11.is_maximized() != (record.mode == Mode::Maximized) {
                    let _ = x11.set_maximized(record.mode == Mode::Maximized);
                }
                if x11.is_fullscreen() != (record.mode == Mode::Fullscreen) {
                    let _ = x11.set_fullscreen(record.mode == Mode::Fullscreen);
                }
                if x11.is_hidden() != (record.mode == Mode::Minimized) {
                    let _ = x11.set_hidden(record.mode == Mode::Minimized);
                }
            }
        }
        if let Some(wm) = self.xwms.first_mut() {
            let _ =
                wm.update_stacking_order_upwards(self.model.windows.iter().filter_map(|record| {
                    self.windows
                        .iter()
                        .find(|(id, _)| *id == record.id)
                        .and_then(|(_, w)| w.x11_surface())
                }));
        }
        for window in &self.overrides {
            if let Some(x11) = window.x11_surface() {
                self.space
                    .map_element(window.clone(), x11.geometry().loc, false);
            }
        }
    }

    fn x11_properties(&mut self, x11: &X11Surface) {
        if let Some(id) = self.x11_id(x11) {
            if let Some(record) = self.model.windows.iter_mut().find(|w| w.id == id) {
                record.x11_title = x11.title();
                // A program that draws its own frame (Steam, Chrome,
                // Telegram say so through the Motif hints) gets no second
                // title bar from us.
                record.decorated = !x11.is_decorated();
                record.min_size = x11.min_size().map_or((1, 1), |s| (s.w, s.h));
                record.max_size = x11.max_size().map_or((0, 0), |s| (s.w, s.h));
                if record.mode != Mode::Fullscreen {
                    let rect = record.rect;
                    self.model.geometry(id, rect);
                }
            }
            self.protocols.toplevels_dirty = true;
            self.sync_positions();
        }
    }

    fn map_x11(&mut self, x11: X11Surface) {
        if self.x11_id(&x11).is_some()
            || self.overrides.iter().any(|w| w.x11_surface() == Some(&x11))
        {
            return;
        }
        let geometry = x11.geometry();
        let window = Window::new_x11_window(x11.clone());
        if x11.is_override_redirect() {
            self.space.map_element(window.clone(), geometry.loc, false);
            self.overrides.push(window);
        } else {
            let id = self.model.open(geometry.size.w, geometry.size.h);
            self.windows.push((id, window));
            let fullscreen = x11.is_fullscreen();
            let maximized = x11.is_maximized();
            self.x11_properties(&x11);
            if fullscreen {
                self.model.set_mode(id, Mode::Fullscreen);
            } else if maximized {
                self.model.set_mode(id, Mode::Maximized);
            }
            self.sync_positions();
            self.sync_focus();
        }
        self.dirty = true;
    }

    fn unmap_x11(&mut self, x11: &X11Surface) {
        if let Some(index) = self
            .windows
            .iter()
            .position(|(_, w)| w.x11_surface() == Some(x11))
        {
            let (id, window) = self.windows.remove(index);
            self.space.unmap_elem(&window);
            self.model.close(id);
            self.sync_focus();
        }
        self.overrides.retain(|window| {
            if window.x11_surface() == Some(x11) {
                self.space.unmap_elem(window);
                false
            } else {
                true
            }
        });
        self.dirty = true;
    }

    fn x11_mode(&mut self, x11: &X11Surface, mode: Mode) {
        if let Some(id) = self.x11_id(x11) {
            self.window_mode(id, mode);
        }
    }

    fn x11_drag(&mut self, x11: &X11Surface, edge: Option<ResizeEdge>) {
        if self.protocols.locked || !self.pointer_handle.is_grabbed() {
            return;
        }
        let Some(id) = self.x11_id(x11).filter(|id| self.model.focus == Some(*id)) else {
            return;
        };
        let Some(rect) = self.model.rect(id) else {
            return;
        };
        let Some((_, window)) = self.windows.iter().find(|(i, _)| *i == id) else {
            return;
        };
        use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge as E;
        let edges = edge.map(|edge| match edge {
            ResizeEdge::Top => E::Top,
            ResizeEdge::Bottom => E::Bottom,
            ResizeEdge::Left => E::Left,
            ResizeEdge::Right => E::Right,
            ResizeEdge::TopLeft => E::TopLeft,
            ResizeEdge::TopRight => E::TopRight,
            ResizeEdge::BottomLeft => E::BottomLeft,
            ResizeEdge::BottomRight => E::BottomRight,
        });
        self.drag = Some(Drag {
            window: window.clone(),
            origin: (rect.x, rect.y).into(),
            size: (rect.width, rect.height),
            start: self.pointer,
            edges,
        });
    }
}
impl XWaylandShellHandler for State {
    fn xwayland_shell_state(&mut self) -> &mut XWaylandShellState {
        &mut self.xwayland_shell
    }
    fn surface_associated(&mut self, _: XwmId, _: WlSurface, window: X11Surface) {
        if window.is_mapped() {
            self.map_x11(window);
        }
        self.sync_focus();
    }
}
smithay::delegate_xwayland_shell!(State);
impl XwmHandler for State {
    fn allow_selection_access(
        &mut self,
        _: XwmId,
        _: smithay::wayland::selection::SelectionTarget,
    ) -> bool {
        !self.protocols.locked
            && self
                .keyboard
                .current_focus()
                .is_some_and(|focus| focus.x11.is_some())
    }
    fn new_selection(
        &mut self,
        id: XwmId,
        target: smithay::wayland::selection::SelectionTarget,
        mimes: Vec<String>,
    ) {
        self.x11_selection(id, target, mimes);
    }
    fn cleared_selection(
        &mut self,
        id: XwmId,
        target: smithay::wayland::selection::SelectionTarget,
    ) {
        self.clear_x11_selection(id, target);
    }
    fn send_selection(
        &mut self,
        _: XwmId,
        target: smithay::wayland::selection::SelectionTarget,
        mime: String,
        fd: std::os::fd::OwnedFd,
    ) {
        self.send_wayland_selection(target, mime, fd);
    }

    fn disconnected(&mut self, id: XwmId) {
        self.clear_x11_selection(id, smithay::wayland::selection::SelectionTarget::Clipboard);
        self.clear_x11_selection(id, smithay::wayland::selection::SelectionTarget::Primary);
        let surfaces: Vec<_> = self
            .windows
            .iter()
            .map(|(_, w)| w)
            .chain(self.overrides.iter())
            .filter_map(|w| w.x11_surface().cloned())
            .collect();
        for surface in surfaces {
            self.unmap_x11(&surface);
        }
        // Not dropped: a selection transfer, a drag over an X11 window or a
        // queued surface request may still call `xwm_state` with this id.
        // Emptying the list made that call index an empty list, and an
        // XWayland crash took the whole session with it.
        let (gone, live): (Vec<_>, Vec<_>) = std::mem::take(&mut self.xwms)
            .into_iter()
            .partition(|wm| wm.id() == id);
        self.xwms = live;
        self.gone_xwms.extend(gone);
    }

    fn xwm_state(&mut self, id: XwmId) -> &mut X11Wm {
        // The event source is registered by start_wm, then stored before
        // dispatch resumes; a manager is never dropped while the compositor
        // runs, so the one asked for is in one of the two lists.
        let live = self.xwms.iter().position(|wm| wm.id() == id);
        let wm = match live {
            Some(i) => self.xwms.get_mut(i),
            None => self
                .gone_xwms
                .iter_mut()
                .find(|wm| wm.id() == id)
                .or(self.xwms.first_mut()),
        };
        #[allow(clippy::expect_used)]
        wm.expect("an X11 window manager for a live XwmId")
    }
    fn new_window(&mut self, _: XwmId, _: X11Surface) {}
    fn new_override_redirect_window(&mut self, _: XwmId, _: X11Surface) {}
    fn map_window_request(&mut self, _: XwmId, window: X11Surface) {
        if let Err(error) = window.set_mapped(true) {
            eprintln!("X11 map: {error}");
            return;
        }
        self.map_x11(window);
    }
    fn mapped_override_redirect_window(&mut self, _: XwmId, window: X11Surface) {
        self.map_x11(window);
    }
    fn unmapped_window(&mut self, _: XwmId, window: X11Surface) {
        self.unmap_x11(&window);
    }
    fn destroyed_window(&mut self, _: XwmId, window: X11Surface) {
        self.unmap_x11(&window);
    }
    fn configure_request(
        &mut self,
        _: XwmId,
        window: X11Surface,
        _: Option<i32>,
        _: Option<i32>,
        width: Option<u32>,
        height: Option<u32>,
        _: Option<Reorder>,
    ) {
        if let Some(id) = self.x11_id(&window) {
            if let Some(mut rect) = self.model.rect(id) {
                if self
                    .model
                    .windows
                    .iter()
                    .any(|w| w.id == id && w.mode == Mode::Normal)
                {
                    if let Some(w) = width {
                        rect.width = i32::try_from(w).unwrap_or(i32::MAX);
                    }
                    if let Some(h) = height {
                        rect.height = i32::try_from(h).unwrap_or(i32::MAX);
                    }
                    self.model.geometry(id, rect);
                }
                // Reply even when declining a client's geometry request.
                let _ = window.configure(None);
                self.sync_positions();
            }
        } else {
            let mut rect = window.geometry();
            if let Some(w) = width {
                rect.size.w = i32::try_from(w).unwrap_or(i32::MAX).max(1);
            }
            if let Some(h) = height {
                rect.size.h = i32::try_from(h).unwrap_or(i32::MAX).max(1);
            }
            let _ = window.configure(rect);
        }
    }
    fn configure_notify(
        &mut self,
        _: XwmId,
        window: X11Surface,
        geometry: Rectangle<i32, Logical>,
        _: Option<u32>,
    ) {
        if window.is_override_redirect() {
            if let Some(w) = self
                .overrides
                .iter()
                .find(|w| w.x11_surface() == Some(&window))
            {
                self.space.map_element(w.clone(), geometry.loc, false);
            }
            self.dirty = true;
        }
    }
    fn property_notify(&mut self, _: XwmId, window: X11Surface, _: WmWindowProperty) {
        self.x11_properties(&window);
    }
    fn maximize_request(&mut self, _: XwmId, window: X11Surface) {
        self.x11_mode(&window, Mode::Maximized);
    }
    fn unmaximize_request(&mut self, _: XwmId, window: X11Surface) {
        self.x11_mode(&window, Mode::Normal);
    }
    fn fullscreen_request(&mut self, _: XwmId, window: X11Surface) {
        self.x11_mode(&window, Mode::Fullscreen);
    }
    fn unfullscreen_request(&mut self, _: XwmId, window: X11Surface) {
        self.x11_mode(&window, Mode::Normal);
    }
    fn minimize_request(&mut self, _: XwmId, window: X11Surface) {
        self.x11_mode(&window, Mode::Minimized);
    }
    fn unminimize_request(&mut self, _: XwmId, window: X11Surface) {
        if let Some(id) = self.x11_id(&window) {
            self.model.restore(id);
            self.configure_windows();
        }
    }
    fn resize_request(&mut self, _: XwmId, window: X11Surface, _: u32, edge: ResizeEdge) {
        self.x11_drag(&window, Some(edge));
    }
    fn move_request(&mut self, _: XwmId, window: X11Surface, _: u32) {
        self.x11_drag(&window, None);
    }
}

/// Keyboard target retaining X11 ICCCM focus semantics and its Wayland transport.
#[derive(Clone, Debug, PartialEq)]
pub struct Focus {
    surface: WlSurface,
    x11: Option<X11Surface>,
}
impl Focus {
    /// Wayland resource identity, also for X11 keyboard transports.
    pub fn id(&self) -> smithay::reexports::wayland_server::backend::ObjectId {
        use smithay::reexports::wayland_server::Resource;
        self.surface.id()
    }
    pub(crate) fn window(window: &Window) -> Option<Self> {
        Some(Self {
            surface: surface(window)?,
            x11: window.x11_surface().cloned(),
        })
    }
}
impl From<WlSurface> for Focus {
    fn from(surface: WlSurface) -> Self {
        Self { surface, x11: None }
    }
}
impl From<smithay::desktop::PopupKind> for Focus {
    fn from(popup: smithay::desktop::PopupKind) -> Self {
        popup.wl_surface().clone().into()
    }
}
impl From<Focus> for WlSurface {
    fn from(focus: Focus) -> Self {
        focus.surface
    }
}
impl smithay::utils::IsAlive for Focus {
    fn alive(&self) -> bool {
        smithay::utils::IsAlive::alive(&self.surface)
    }
}
impl smithay::wayland::seat::WaylandFocus for Focus {
    fn wl_surface(&self) -> Option<std::borrow::Cow<'_, WlSurface>> {
        Some(std::borrow::Cow::Borrowed(&self.surface))
    }
}
use smithay::backend::input::KeyState;
use smithay::{
    input::{
        Seat,
        keyboard::{KeyboardTarget, KeysymHandle, ModifiersState},
    },
    utils::Serial,
};
impl KeyboardTarget<State> for Focus {
    fn enter(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        keys: Vec<KeysymHandle<'_>>,
        serial: Serial,
    ) {
        if let Some(x11) = &self.x11 {
            KeyboardTarget::enter(x11, seat, data, keys, serial);
        } else {
            KeyboardTarget::enter(&self.surface, seat, data, keys, serial);
        }
    }
    fn leave(&self, seat: &Seat<State>, data: &mut State, serial: Serial) {
        if let Some(x11) = &self.x11 {
            KeyboardTarget::leave(x11, seat, data, serial);
        } else {
            KeyboardTarget::leave(&self.surface, seat, data, serial);
        }
    }
    fn key(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        key: KeysymHandle<'_>,
        state: KeyState,
        serial: Serial,
        time: u32,
    ) {
        if let Some(x11) = &self.x11 {
            x11.key(seat, data, key, state, serial, time);
        } else {
            self.surface.key(seat, data, key, state, serial, time);
        }
    }
    fn modifiers(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        modifiers: ModifiersState,
        serial: Serial,
    ) {
        if let Some(x11) = &self.x11 {
            x11.modifiers(seat, data, modifiers, serial);
        } else {
            self.surface.modifiers(seat, data, modifiers, serial);
        }
    }
}

use smithay::input::pointer::*;
impl Focus {
    fn inner_pointer_target(&self) -> &dyn PointerTarget<State> {
        // X11Surface forwards regular input to its Wayland transport.
        match &self.x11 {
            Some(x11) => x11,
            None => &self.surface,
        }
    }
    /// Retain the X11 identity when selecting a pointer target.
    pub fn pointer(state: &State, surface: WlSurface) -> Self {
        let x11 = state
            .windows
            .iter()
            .map(|(_, w)| w)
            .chain(state.overrides.iter())
            .find(|w| crate::xwayland::surface(w).as_ref() == Some(&surface))
            .and_then(|w| w.x11_surface())
            .cloned();
        Self { surface, x11 }
    }
}
impl PointerTarget<State> for Focus {
    fn enter(&self, seat: &Seat<State>, data: &mut State, event: &MotionEvent) {
        self.inner_pointer_target().enter(seat, data, event)
    }
    fn motion(&self, seat: &Seat<State>, data: &mut State, event: &MotionEvent) {
        self.inner_pointer_target().motion(seat, data, event)
    }
    fn relative_motion(&self, seat: &Seat<State>, data: &mut State, event: &RelativeMotionEvent) {
        self.inner_pointer_target()
            .relative_motion(seat, data, event)
    }
    fn button(&self, seat: &Seat<State>, data: &mut State, event: &ButtonEvent) {
        self.inner_pointer_target().button(seat, data, event)
    }
    fn axis(&self, seat: &Seat<State>, data: &mut State, frame: AxisFrame) {
        self.inner_pointer_target().axis(seat, data, frame)
    }
    fn frame(&self, seat: &Seat<State>, data: &mut State) {
        self.inner_pointer_target().frame(seat, data)
    }
    fn leave(&self, seat: &Seat<State>, data: &mut State, serial: Serial, time: u32) {
        self.inner_pointer_target().leave(seat, data, serial, time)
    }
    fn gesture_swipe_begin(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &GestureSwipeBeginEvent,
    ) {
        self.inner_pointer_target()
            .gesture_swipe_begin(seat, data, event)
    }
    fn gesture_swipe_update(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &GestureSwipeUpdateEvent,
    ) {
        self.inner_pointer_target()
            .gesture_swipe_update(seat, data, event)
    }
    fn gesture_swipe_end(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &GestureSwipeEndEvent,
    ) {
        self.inner_pointer_target()
            .gesture_swipe_end(seat, data, event)
    }
    fn gesture_pinch_begin(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &GesturePinchBeginEvent,
    ) {
        self.inner_pointer_target()
            .gesture_pinch_begin(seat, data, event)
    }
    fn gesture_pinch_update(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &GesturePinchUpdateEvent,
    ) {
        self.inner_pointer_target()
            .gesture_pinch_update(seat, data, event)
    }
    fn gesture_pinch_end(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &GesturePinchEndEvent,
    ) {
        self.inner_pointer_target()
            .gesture_pinch_end(seat, data, event)
    }
    fn gesture_hold_begin(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &GestureHoldBeginEvent,
    ) {
        self.inner_pointer_target()
            .gesture_hold_begin(seat, data, event)
    }
    fn gesture_hold_end(&self, seat: &Seat<State>, data: &mut State, event: &GestureHoldEndEvent) {
        self.inner_pointer_target()
            .gesture_hold_end(seat, data, event)
    }
}

use smithay::{
    input::dnd::{DndFocus, OfferData, Source},
    reexports::wayland_server::DisplayHandle,
    utils::Point,
    wayland::selection::data_device::WlOfferData,
    xwayland::xwm::XwmOfferData,
};
use std::sync::Arc;
/// Offer transport selected by the destination surface.
pub enum DragOffer<S: Source> {
    /// Native Wayland data offer.
    Wayland(WlOfferData<S>),
    /// XDND offer managed by X11Wm.
    X11(XwmOfferData<S>),
}

impl<S: Source> OfferData for DragOffer<S> {
    fn disable(&self) {
        match self {
            DragOffer::Wayland(data) => data.disable(),
            DragOffer::X11(data) => data.disable(),
        }
    }

    fn drop(&self) {
        match self {
            DragOffer::Wayland(data) => data.drop(),
            DragOffer::X11(data) => data.drop(),
        }
    }

    fn validated(&self) -> bool {
        match self {
            DragOffer::Wayland(data) => data.validated(),
            DragOffer::X11(data) => data.validated(),
        }
    }
}

impl DndFocus<State> for Focus {
    type OfferData<S>
        = DragOffer<S>
    where
        S: Source;

    fn enter<S: Source>(
        &self,
        data: &mut State,
        dh: &DisplayHandle,
        source: Arc<S>,
        seat: &Seat<State>,
        location: Point<f64, Logical>,
        serial: &Serial,
    ) -> Option<DragOffer<S>> {
        match &self.x11 {
            None => {
                let surface = &self.surface;
                DndFocus::enter(surface, data, dh, source, seat, location, serial)
                    .map(DragOffer::Wayland)
            }
            Some(surface) => DndFocus::enter(surface, data, dh, source, seat, location, serial)
                .map(DragOffer::X11),
        }
    }

    fn motion<S: Source>(
        &self,
        data: &mut State,
        offer: Option<&mut DragOffer<S>>,
        seat: &Seat<State>,
        location: Point<f64, Logical>,
        time: u32,
    ) {
        match &self.x11 {
            None => {
                let surface = &self.surface;
                let offer = match offer {
                    Some(DragOffer::Wayland(offer)) => Some(offer),
                    None => None,
                    _ => return,
                };
                DndFocus::motion(surface, data, offer, seat, location, time)
            }
            Some(surface) => {
                let offer = match offer {
                    Some(DragOffer::X11(offer)) => Some(offer),
                    None => None,
                    _ => return,
                };
                DndFocus::motion(surface, data, offer, seat, location, time)
            }
        }
    }

    fn leave<S: Source>(
        &self,
        data: &mut State,
        offer: Option<&mut DragOffer<S>>,
        seat: &Seat<State>,
    ) {
        match &self.x11 {
            None => {
                let surface = &self.surface;
                let offer = match offer {
                    Some(DragOffer::Wayland(offer)) => Some(offer),
                    None => None,
                    _ => return,
                };
                DndFocus::leave(surface, data, offer, seat)
            }
            Some(surface) => {
                let offer = match offer {
                    Some(DragOffer::X11(offer)) => Some(offer),
                    None => None,
                    _ => return,
                };
                DndFocus::leave(surface, data, offer, seat)
            }
        }
    }

    fn drop<S: Source>(
        &self,
        data: &mut State,
        offer: Option<&mut DragOffer<S>>,
        seat: &Seat<State>,
    ) {
        match &self.x11 {
            None => {
                let surface = &self.surface;
                let offer = match offer {
                    Some(DragOffer::Wayland(offer)) => Some(offer),
                    None => None,
                    _ => return,
                };
                DndFocus::drop(surface, data, offer, seat)
            }
            Some(surface) => {
                let offer = match offer {
                    Some(DragOffer::X11(offer)) => Some(offer),
                    None => None,
                    _ => return,
                };
                DndFocus::drop(surface, data, offer, seat)
            }
        }
    }
}
