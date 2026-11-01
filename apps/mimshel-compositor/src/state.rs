//! Wayland protocol state, window mapping, and seat focus.

use smithay::{
    backend::renderer::utils::on_commit_buffer_handler,
    desktop::{PopupKind, PopupManager, Space, Window, WindowSurfaceType},
    input::{Seat, SeatHandler, SeatState},
    reexports::wayland_server::{
        Client, DisplayHandle,
        backend::{ClientData, ClientId, DisconnectReason},
        protocol::{wl_buffer::WlBuffer, wl_surface::WlSurface},
    },
    utils::{Logical, Point},
    wayland::{
        buffer::BufferHandler,
        compositor::{
            CompositorClientState, CompositorHandler, CompositorState, get_parent,
            is_sync_subsurface,
        },
        output::{OutputHandler, OutputManagerState},
        shell::xdg::{
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
        },
        shm::{ShmHandler, ShmState},
    },
};

/// Active pointer move or resize operation.
#[derive(Clone)]
pub struct Drag {
    /// Requested resize edges, or a move.
    pub edges:
        Option<smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge>,
    /// Window size when the drag began.
    pub size: (i32, i32),
    /// Window being dragged.
    pub window: Window,
    /// Pointer position when the drag began.
    pub start: Point<f64, Logical>,
    /// Window origin when the drag began.
    pub origin: Point<i32, Logical>,
}

/// Shared protocol, input, and window policy state.
pub struct State {
    /// Protocol-independent placement and focus policy.
    pub model: crate::model::Model,
    /// Shell protocol resources and policy.
    pub protocols: crate::protocols::Protocols,
    /// Mapped windows in stack or creation order.
    pub windows: Vec<(crate::model::WindowId, Window)>,
    pending: Vec<Window>,
    /// Whether an event requires a frame.
    pub dirty: bool,
    /// Physical pixels per logical point on the nested output.
    pub output_scale: f64,
    /// Wayland display handle.
    pub dh: DisplayHandle,
    /// DMA buffer protocol globals.
    pub dmabuf: smithay::wayland::dmabuf::DmabufState,
    /// Pending DMA buffer import requests.
    pub imports: Vec<(
        smithay::backend::allocator::dmabuf::Dmabuf,
        smithay::wayland::dmabuf::ImportNotifier,
    )>,
    compositor: CompositorState,
    shm: ShmState,
    xdg: XdgShellState,
    _decoration: smithay::wayland::shell::xdg::decoration::XdgDecorationState,
    requested_modes: Vec<(ToplevelSurface, crate::model::Mode)>,
    /// Window whose button group is hovered.
    pub hovered: Option<crate::model::WindowId>,
    /// Whether the current left-button press belongs to a compositor control.
    pub decoration_pressed: bool,
    /// Host cursor selected by frame hit-testing.
    pub cursor: smithay::reexports::winit::window::CursorIcon,
    /// Last title click for double-click detection.
    pub title_click: Option<(crate::model::WindowId, u32)>,
    seats: SeatState<Self>,
    /// Mapped desktop windows and outputs.
    pub space: Space<Window>,
    /// Tracked popup surfaces and grabs.
    pub popups: PopupManager,
    /// Client-visible input seat.
    pub seat: Seat<Self>,
    /// Persistent keyboard handle.
    pub keyboard: smithay::input::keyboard::KeyboardHandle<Self>,
    /// Persistent pointer handle.
    pub pointer_handle: smithay::input::pointer::PointerHandle<Self>,
    /// Current logical pointer position.
    pub pointer: Point<f64, Logical>,
    /// Whether the pointer is inside the host window.
    pub pointer_inside: bool,
    /// Current interactive operation.
    pub drag: Option<Drag>,
}

impl State {
    /// Create initialized state.
    pub fn new(
        dh: DisplayHandle,
        handle: smithay::reexports::calloop::LoopHandle<'static, Self>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let protocols = crate::protocols::Protocols::new(&dh, handle);
        let compositor = CompositorState::new::<Self>(&dh);
        let shm = ShmState::new::<Self>(&dh, vec![]);
        let xdg = XdgShellState::new::<Self>(&dh);
        let decoration =
            smithay::wayland::shell::xdg::decoration::XdgDecorationState::new::<Self>(&dh);
        OutputManagerState::new_with_xdg_output::<Self>(&dh);
        let mut seats = SeatState::new();
        let mut seat = seats.new_wl_seat(&dh, "mimshel");
        let keyboard = seat.add_keyboard(
            smithay::input::keyboard::XkbConfig {
                layout: "us,ru",
                options: Some("grp:alt_shift_toggle".into()),
                ..Default::default()
            },
            600,
            25,
        )?;
        let pointer_handle = seat.add_pointer();
        Ok(Self {
            model: crate::model::Model::new(1280, 800),
            windows: Vec::new(),
            pending: Vec::new(),
            protocols,
            dirty: true,
            output_scale: 1.0,
            dh,
            dmabuf: smithay::wayland::dmabuf::DmabufState::new(),
            imports: Vec::new(),
            compositor,
            shm,
            xdg,
            _decoration: decoration,
            requested_modes: Vec::new(),
            hovered: None,
            title_click: None,
            decoration_pressed: false,
            cursor: smithay::reexports::winit::window::CursorIcon::Default,
            seats,
            space: Space::default(),
            popups: PopupManager::default(),
            seat,
            keyboard,
            pointer_handle,
            pointer: (0.0, 0.0).into(),
            pointer_inside: false,
            drag: None,
        })
    }

    /// Publish policy activation and keyboard focus to clients.
    pub fn sync_focus(&mut self) {
        self.protocols.toplevels_dirty = true;
        if self.protocols.locked {
            self.focus_lock();
            return;
        }
        for (id, window) in &self.windows {
            window.set_activated(self.model.focus == Some(*id));
            if let Some(top) = window.toplevel() {
                top.send_pending_configure();
            }
        }
        let focus = self
            .exclusive_layer_focus()
            .or_else(|| self.current_layer_focus())
            .or_else(|| {
                self.windows
                    .iter()
                    .find(|(id, _)| Some(*id) == self.model.focus)
                    .and_then(|(_, w)| w.toplevel().map(|top| top.wl_surface().clone()))
            });
        self.keyboard
            .clone()
            .set_focus(self, focus, smithay::utils::SERIAL_COUNTER.next_serial());
        self.dirty = true;
    }

    /// Remove dead windows and clean protocol resources.
    pub fn refresh(&mut self) {
        use smithay::utils::IsAlive;
        let dead: Vec<_> = self
            .windows
            .iter()
            .filter(|(_, w)| !w.alive())
            .map(|(id, _)| *id)
            .collect();
        for id in &dead {
            self.model.close(*id);
        }
        self.windows.retain(|(id, _)| !dead.contains(id));
        self.pending.retain(|w| w.alive());
        self.refresh_protocols();
        self.space.refresh();
        self.popups.cleanup();
        if !dead.is_empty() {
            self.sync_focus();
        }
    }

    /// Apply the policy stack and positions to the desktop space.
    pub fn sync_positions(&mut self) {
        self.protocols.toplevels_dirty = true;
        // map_element also raises; replay the model stack, not creation order.
        for record in &self.model.windows {
            if let Some((_, window)) = self.windows.iter().find(|(id, _)| *id == record.id) {
                if record.mode == crate::model::Mode::Minimized {
                    self.space.unmap_elem(window);
                } else {
                    self.space.map_element(
                        window.clone(),
                        (
                            record.rect.x,
                            record.rect.y + self.model.title_height(record.id),
                        ),
                        false,
                    );
                }
            }
        }
        self.update_frame_pointer();
        self.dirty = true;
    }

    /// Apply a presentation request, including requests made before mapping.
    pub fn request_mode(&mut self, surface: &ToplevelSurface, mode: crate::model::Mode) {
        use crate::model::Mode;
        use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State as XdgState;
        let id = self
            .windows
            .iter()
            .find(|(_, w)| w.toplevel() == Some(surface))
            .map(|(id, _)| *id);
        let size = if let Some(id) = id {
            self.model.set_mode(id, mode);
            self.model.rect(id).map(|r| (r.width, r.height))
        } else {
            self.requested_modes.retain(|(t, _)| t != surface);
            self.requested_modes.push((surface.clone(), mode));
            let area = self.model.area;
            match mode {
                Mode::Maximized => Some((
                    area.width,
                    (area.height
                        - if server_decorated(surface) {
                            crate::model::TITLE_HEIGHT
                        } else {
                            0
                        })
                    .max(1),
                )),
                Mode::Fullscreen => Some((self.model.output.width, self.model.output.height)),
                Mode::Normal => Some((640.min(area.width), 400.min(area.height))),
                Mode::Minimized => None,
            }
        };
        surface.with_pending_state(|s| {
            if let Some(size) = size {
                s.size = Some(size.into());
            }
            s.states.unset(XdgState::Maximized);
            s.states.unset(XdgState::Fullscreen);
            if mode == Mode::Maximized {
                s.states.set(XdgState::Maximized);
            }
            if mode == Mode::Fullscreen {
                s.states.set(XdgState::Fullscreen);
            }
        });
        surface.send_pending_configure();
        self.sync_positions();
        self.sync_focus();
    }

    /// Publish geometry and state after output resize or restoring a hidden window.
    pub fn configure_windows(&mut self) {
        self.configure_locks();
        for (id, window) in &self.windows {
            let Some(record) = self.model.windows.iter().find(|w| w.id == *id) else {
                continue;
            };
            if let Some(top) = window.toplevel() {
                use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State as XdgState;
                top.with_pending_state(|s| {
                    s.size = Some((record.rect.width, record.rect.height).into());
                    s.states.unset(XdgState::Maximized);
                    s.states.unset(XdgState::Fullscreen);
                    if record.mode == crate::model::Mode::Maximized {
                        s.states.set(XdgState::Maximized);
                    }
                    if record.mode == crate::model::Mode::Fullscreen {
                        s.states.set(XdgState::Fullscreen);
                    }
                });
                top.send_pending_configure();
            }
        }
        self.sync_positions();
        self.sync_focus();
    }

    /// Locate a compositor frame control in front-to-back order.
    pub fn decoration_under(&self) -> Option<(crate::model::WindowId, crate::decoration::Hit)> {
        if self.protocols.locked || self.layer_under(true).is_some() {
            return None;
        }
        for w in self.model.windows.iter().rev() {
            if w.mode == crate::model::Mode::Minimized {
                continue;
            }
            // Popups are above both the client's content and our frame.
            if self
                .windows
                .iter()
                .find(|(id, _)| *id == w.id)
                .is_some_and(|(_, window)| {
                    self.space.element_location(window).is_some_and(|origin| {
                        window
                            .surface_under(
                                self.pointer - origin.to_f64(),
                                WindowSurfaceType::POPUP | WindowSurfaceType::SUBSURFACE,
                            )
                            .is_some()
                    })
                })
            {
                return None;
            }
            let x = self.pointer.x - f64::from(w.rect.x);
            let y = self.pointer.y - f64::from(w.rect.y);
            if w.decorated
                && w.mode != crate::model::Mode::Fullscreen
                && let Some(hit) = crate::decoration::hit_test(x, y, w.rect.width, w.rect.height)
            {
                return Some((w.id, hit));
            }
            let title = self.model.title_height(w.id);
            if x >= 0.0
                && x < f64::from(w.rect.width)
                && y >= f64::from(title)
                && y < f64::from(w.rect.height + title)
            {
                return None;
            }
        }
        None
    }

    /// Constrain a popup to the logical output bounds.
    pub(crate) fn popup_geometry(
        &self,
        surface: &PopupSurface,
        positioner: PositionerState,
    ) -> smithay::utils::Rectangle<i32, Logical> {
        use smithay::desktop::{find_popup_root_surface, get_popup_toplevel_coords};
        let kind = PopupKind::Xdg(surface.clone());
        let root = find_popup_root_surface(&kind).ok();
        let origin = self
            .windows
            .iter()
            .find(|(_, w)| w.toplevel().map(|top| top.wl_surface()) == root.as_ref())
            .and_then(|(id, _)| {
                self.model
                    .rect(*id)
                    .map(|r| Point::from((r.x, r.y + self.model.title_height(*id))))
            })
            .or_else(|| {
                let root = root.as_ref()?;
                let output = self.space.outputs().next()?;
                let map = smithay::desktop::layer_map_for_output(output);
                let layer = map.layer_for_surface(root, WindowSurfaceType::ALL)?;
                map.layer_geometry(layer).map(|g| g.loc)
            })
            .unwrap_or_default();
        let offset = origin + get_popup_toplevel_coords(&kind);
        let area = self.model.area;
        positioner.get_unconstrained_geometry(smithay::utils::Rectangle::new(
            (area.x - offset.x, area.y - offset.y).into(),
            (area.width, area.height).into(),
        ))
    }

    /// Find the surface and origin under the pointer.
    pub fn surface_under(&self) -> Option<(WlSurface, Point<f64, Logical>)> {
        if self.protocols.locked {
            return self.lock_under();
        }
        if let Some(hit) = self.layer_under(true) {
            return Some(hit);
        }
        if self.decoration_under().is_some() {
            return None;
        }
        self.space
            .element_under(self.pointer)
            .and_then(|(w, origin)| {
                w.surface_under(self.pointer - origin.to_f64(), WindowSurfaceType::ALL)
                    .map(|(surface, local)| (surface, (local + origin).to_f64()))
            })
            .or_else(|| self.layer_under(false))
    }
}

/// Per-client Wayland compositor protocol data.
#[derive(Default)]
pub struct ClientState {
    compositor: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _: ClientId) {}
    fn disconnected(&self, _: ClientId, _: DisconnectReason) {}
}

impl CompositorHandler for State {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        // Foreign client data still needs a valid compositor state reference.
        static FALLBACK: std::sync::OnceLock<CompositorClientState> = std::sync::OnceLock::new();
        client
            .get_data::<ClientState>()
            .map(|data| &data.compositor)
            .unwrap_or_else(|| FALLBACK.get_or_init(CompositorClientState::default))
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        self.dirty = true;
        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            let buffered =
                smithay::backend::renderer::utils::with_renderer_surface_state(&root, |s| {
                    s.buffer().is_some()
                })
                .unwrap_or(false);
            if let Some(index) = self
                .pending
                .iter()
                .position(|w| w.toplevel().is_some_and(|top| top.wl_surface() == &root))
            {
                let window = self.pending[index].clone();
                window.on_commit();
                let Some(top) = window.toplevel() else {
                    return;
                };
                if !top.is_initial_configure_sent() {
                    top.send_configure();
                }
                if buffered {
                    self.pending.remove(index);
                    let size = window.geometry().size;
                    let id = self.model.open(size.w, size.h);
                    let decorated = server_decorated(top);
                    if let Some(record) = self.model.windows.iter_mut().find(|w| w.id == id) {
                        record.decorated = decorated;
                    }
                    let requested = self
                        .requested_modes
                        .iter()
                        .find(|(t, _)| t == top)
                        .map(|(_, m)| *m);
                    self.requested_modes.retain(|(t, _)| t != top);
                    self.windows.push((id, window));
                    if let Some(mode) = requested {
                        self.model.set_mode(id, mode);
                    }
                    self.sync_positions();
                    self.sync_focus();
                }
            } else if let Some(index) = self
                .windows
                .iter()
                .position(|(_, w)| w.toplevel().is_some_and(|top| top.wl_surface() == &root))
            {
                let (id, window) = self.windows[index].clone();
                window.on_commit();
                if buffered {
                    let size = window.geometry().size;
                    let Some(mut rect) = self.model.rect(id) else {
                        return;
                    };
                    rect.width = size.w;
                    rect.height = size.h;
                    if self
                        .model
                        .windows
                        .iter()
                        .any(|w| w.id == id && w.mode == crate::model::Mode::Normal)
                    {
                        self.model.geometry(id, rect);
                    }
                } else {
                    self.windows.remove(index);
                    self.space.unmap_elem(&window);
                    self.model.close(id);
                    self.pending.push(window);
                    self.sync_focus();
                }
            }
        }
        self.commit_layers(surface);
        if self.protocols.locked {
            self.focus_lock();
        }
        self.popups.commit(surface);
        if let Some(PopupKind::Xdg(popup)) = self.popups.find_popup(surface)
            && !popup.is_initial_configure_sent()
            && let Err(error) = popup.send_configure()
        {
            eprintln!("popup configure: {error}");
        }
    }
}

impl BufferHandler for State {
    fn buffer_destroyed(&mut self, _: &WlBuffer) {}
}

impl ShmHandler for State {
    fn shm_state(&self) -> &ShmState {
        &self.shm
    }
}

impl XdgShellHandler for State {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let area = self.model.area;
        surface.with_pending_state(|s| {
            s.size = Some(
                (
                    640.min(area.width),
                    400.min((area.height - crate::model::TITLE_HEIGHT).max(1)),
                )
                    .into(),
            )
        });
        self.pending.push(Window::new_wayland_window(surface));
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        self.pending.retain(|w| w.toplevel() != Some(&surface));
        self.requested_modes.retain(|(t, _)| t != &surface);
        if let Some(index) = self
            .windows
            .iter()
            .position(|(_, w)| w.toplevel() == Some(&surface))
        {
            let (id, window) = self.windows.remove(index);
            self.space.unmap_elem(&window);
            self.model.close(id);
            self.sync_focus();
        }
    }

    fn move_request(
        &mut self,
        surface: ToplevelSurface,
        seat: smithay::reexports::wayland_server::protocol::wl_seat::WlSeat,
        serial: smithay::utils::Serial,
    ) {
        self.client_drag(surface, seat, serial, None);
    }

    fn resize_request(
        &mut self,
        surface: ToplevelSurface,
        seat: smithay::reexports::wayland_server::protocol::wl_seat::WlSeat,
        serial: smithay::utils::Serial,
        edges: smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge,
    ) {
        self.client_drag(surface, seat, serial, Some(edges));
    }

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        self.request_mode(&surface, crate::model::Mode::Maximized);
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        self.request_mode(&surface, crate::model::Mode::Normal);
    }

    fn fullscreen_request(
        &mut self,
        surface: ToplevelSurface,
        _: Option<smithay::reexports::wayland_server::protocol::wl_output::WlOutput>,
    ) {
        self.request_mode(&surface, crate::model::Mode::Fullscreen);
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        self.request_mode(&surface, crate::model::Mode::Normal);
    }

    fn minimize_request(&mut self, surface: ToplevelSurface) {
        self.request_mode(&surface, crate::model::Mode::Minimized);
    }

    fn app_id_changed(&mut self, _: ToplevelSurface) {
        self.protocols.toplevels_dirty = true;
        self.publish_toplevels();
    }

    fn title_changed(&mut self, _: ToplevelSurface) {
        self.protocols.toplevels_dirty = true;
        self.publish_toplevels();
        self.dirty = true;
    }

    fn new_popup(&mut self, surface: PopupSurface, positioner: PositionerState) {
        let geometry = self.popup_geometry(&surface, positioner);
        surface.with_pending_state(|s| {
            s.geometry = geometry;
            s.positioner = positioner;
        });
        if let Err(error) = self.popups.track_popup(PopupKind::Xdg(surface)) {
            eprintln!("popup: {error}");
        }
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        let geometry = self.popup_geometry(&surface, positioner);
        surface.with_pending_state(|s| {
            s.geometry = geometry;
            s.positioner = positioner;
        });
        surface.send_repositioned(token);
    }

    fn grab(
        &mut self,
        surface: PopupSurface,
        seat: smithay::reexports::wayland_server::protocol::wl_seat::WlSeat,
        serial: smithay::utils::Serial,
    ) {
        if self.protocols.locked {
            return;
        }
        use smithay::desktop::{PopupKeyboardGrab, PopupPointerGrab, find_popup_root_surface};
        if Seat::from_resource(&seat).as_ref() != Some(&self.seat) {
            return;
        }
        let kind = PopupKind::Xdg(surface);
        let Ok(root) = find_popup_root_surface(&kind) else {
            return;
        };
        let pointer = self.pointer_handle.clone();
        let keyboard = self.keyboard.clone();
        if !pointer.has_grab(serial) && !keyboard.has_grab(serial) {
            return;
        }
        if let Ok(grab) = self.popups.grab_popup(root, kind, &self.seat, serial) {
            keyboard.set_focus(self, grab.current_grab(), serial);
            keyboard.set_grab(self, PopupKeyboardGrab::new(&grab), serial);
            pointer.set_grab(
                self,
                PopupPointerGrab::new(&grab),
                serial,
                smithay::input::pointer::Focus::Keep,
            );
        }
    }
}

impl SeatHandler for State {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;
    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seats
    }

    fn cursor_image(&mut self, _: &Seat<Self>, _: smithay::input::pointer::CursorImageStatus) {}
}

impl OutputHandler for State {}
smithay::delegate_compositor!(State);
smithay::delegate_shm!(State);
smithay::delegate_xdg_shell!(State);
smithay::delegate_seat!(State);
smithay::delegate_output!(State);

impl smithay::wayland::dmabuf::DmabufHandler for State {
    fn dmabuf_state(&mut self) -> &mut smithay::wayland::dmabuf::DmabufState {
        &mut self.dmabuf
    }

    fn dmabuf_imported(
        &mut self,
        _: &smithay::wayland::dmabuf::DmabufGlobal,
        buffer: smithay::backend::allocator::dmabuf::Dmabuf,
        notifier: smithay::wayland::dmabuf::ImportNotifier,
    ) {
        self.imports.push((buffer, notifier));
    }
}
smithay::delegate_dmabuf!(State);

/// Whether the client has accepted compositor decorations (also the default without the protocol).
pub fn server_decorated(top: &ToplevelSurface) -> bool {
    use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode;
    top.with_pending_state(|s| s.decoration_mode != Some(Mode::ClientSide))
}

impl smithay::wayland::shell::xdg::decoration::XdgDecorationHandler for State {
    fn new_decoration(&mut self, top: ToplevelSurface) {
        self.decoration_mode(top, false);
    }
    fn request_mode(
        &mut self,
        top: ToplevelSurface,
        mode: smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode,
    ) {
        use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode;
        self.decoration_mode(top, mode == Mode::ClientSide);
    }
    fn unset_mode(&mut self, top: ToplevelSurface) {
        self.decoration_mode(top, false);
    }
}

impl State {
    fn decoration_mode(&mut self, top: ToplevelSurface, client: bool) {
        use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode;
        top.with_pending_state(|s| {
            s.decoration_mode = Some(if client {
                Mode::ClientSide
            } else {
                Mode::ServerSide
            })
        });
        top.send_pending_configure();
        if let Some((id, _)) = self
            .windows
            .iter()
            .find(|(_, w)| w.toplevel() == Some(&top))
        {
            if let Some(w) = self.model.windows.iter_mut().find(|w| w.id == *id) {
                w.decorated = !client;
            }
            self.model.set_work_area(self.model.area);
            self.configure_windows();
        } else if let Some(mode) = self
            .requested_modes
            .iter()
            .find(|(t, _)| t == &top)
            .map(|(_, m)| *m)
        {
            self.request_mode(&top, mode);
        }
    }
}
smithay::delegate_xdg_decoration!(State);
