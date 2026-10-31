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
    /// Mapped windows in stack or creation order.
    pub windows: Vec<(crate::model::WindowId, Window)>,
    pending: Vec<Window>,
    /// Whether an event requires a frame.
    pub dirty: bool,
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
    pub fn new(dh: DisplayHandle) -> Result<Self, Box<dyn std::error::Error>> {
        let compositor = CompositorState::new::<Self>(&dh);
        let shm = ShmState::new::<Self>(&dh, vec![]);
        let xdg = XdgShellState::new::<Self>(&dh);
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
            dirty: true,
            dh,
            dmabuf: smithay::wayland::dmabuf::DmabufState::new(),
            imports: Vec::new(),
            compositor,
            shm,
            xdg,
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
        for (id, window) in &self.windows {
            window.set_activated(self.model.focus == Some(*id));
            if let Some(top) = window.toplevel() {
                top.send_pending_configure();
            }
        }
        let focus = self
            .windows
            .iter()
            .find(|(id, _)| Some(*id) == self.model.focus)
            .and_then(|(_, w)| w.toplevel().map(|top| top.wl_surface().clone()));
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
        self.space.refresh();
        self.popups.cleanup();
        if !dead.is_empty() {
            self.sync_focus();
        }
    }

    /// Apply the policy stack and positions to the desktop space.
    pub fn sync_positions(&mut self) {
        // map_element also raises; replay the model stack, not creation order.
        for record in &self.model.windows {
            if let Some((_, window)) = self.windows.iter().find(|(id, _)| *id == record.id) {
                self.space
                    .map_element(window.clone(), (record.rect.x, record.rect.y), false);
            }
        }
        self.dirty = true;
    }

    /// Constrain a popup to the logical output bounds.
    fn popup_geometry(
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
            .and_then(|(id, _)| self.model.rect(*id))
            .map(|r| Point::from((r.x, r.y)))
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
        self.space
            .element_under(self.pointer)
            .and_then(|(w, origin)| {
                w.surface_under(self.pointer - origin.to_f64(), WindowSurfaceType::ALL)
                    .map(|(surface, local)| (surface, (local + origin).to_f64()))
            })
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
                    let Some(rect) = self.model.rect(id) else {
                        return;
                    };
                    self.space
                        .map_element(window.clone(), (rect.x, rect.y), false);
                    self.windows.push((id, window));
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
                    self.model.geometry(id, rect);
                } else {
                    self.windows.remove(index);
                    self.space.unmap_elem(&window);
                    self.model.close(id);
                    self.pending.push(window);
                    self.sync_focus();
                }
            }
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
            s.size = Some((640.min(area.width), 400.min(area.height)).into())
        });
        self.pending.push(Window::new_wayland_window(surface));
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        self.pending.retain(|w| w.toplevel() != Some(&surface));
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
