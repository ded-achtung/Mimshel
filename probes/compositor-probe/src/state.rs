use smithay::{
    backend::renderer::utils::on_commit_buffer_handler,
    desktop::{PopupKind, PopupManager, Space, Window, WindowSurfaceType},
    input::{Seat, SeatHandler, SeatState},
    reexports::wayland_server::{
        Client, DisplayHandle, Resource,
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
        selection::{
            SelectionHandler,
            data_device::{
                ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
                set_data_device_focus,
            },
        },
        shell::xdg::{
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
        },
        shm::{ShmHandler, ShmState},
    },
};

#[derive(Clone)]
pub struct Drag {
    pub window: Window,
    pub start: Point<f64, Logical>,
    pub origin: Point<i32, Logical>,
}

pub struct State {
    pub dh: DisplayHandle,
    pub dmabuf: smithay::wayland::dmabuf::DmabufState,
    pub imports: Vec<(
        smithay::backend::allocator::dmabuf::Dmabuf,
        smithay::wayland::dmabuf::ImportNotifier,
    )>,
    compositor: CompositorState,
    shm: ShmState,
    xdg: XdgShellState,
    seats: SeatState<Self>,
    data_device: DataDeviceState,
    pub space: Space<Window>,
    pub popups: PopupManager,
    pub seat: Seat<Self>,
    pub pointer: Point<f64, Logical>,
    pub pointer_inside: bool,
    pub drag: Option<Drag>,
}
impl State {
    pub fn new(dh: DisplayHandle) -> Result<Self, Box<dyn std::error::Error>> {
        let compositor = CompositorState::new::<Self>(&dh);
        let shm = ShmState::new::<Self>(&dh, vec![]);
        let xdg = XdgShellState::new::<Self>(&dh);
        OutputManagerState::new_with_xdg_output::<Self>(&dh);
        let mut seats = SeatState::new();
        let mut seat = seats.new_wl_seat(&dh, "probe");
        seat.add_keyboard(Default::default(), 200, 25)?;
        seat.add_pointer();
        let data_device = DataDeviceState::new::<Self>(&dh);
        Ok(Self {
            dh,
            dmabuf: smithay::wayland::dmabuf::DmabufState::new(),
            imports: Vec::new(),
            compositor,
            shm,
            xdg,
            seats,
            data_device,
            space: Space::default(),
            popups: PopupManager::default(),
            seat,
            pointer: (0.0, 0.0).into(),
            pointer_inside: false,
            drag: None,
        })
    }
    pub fn surface_under(&self) -> Option<(WlSurface, Point<f64, Logical>)> {
        self.space
            .element_under(self.pointer)
            .and_then(|(w, origin)| {
                w.surface_under(self.pointer - origin.to_f64(), WindowSurfaceType::ALL)
                    .map(|(surface, local)| (surface, (local + origin).to_f64()))
            })
    }
}

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
        &client
            .get_data::<ClientState>()
            .expect("probe client data")
            .compositor
    }
    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            if let Some(window) = self
                .space
                .elements()
                .find(|w| w.toplevel().is_some_and(|t| t.wl_surface() == &root))
            {
                window.on_commit();
                let top = window.toplevel().unwrap();
                if !top.is_initial_configure_sent() {
                    top.send_configure();
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
        let n = self.space.elements().count() as i32;
        surface.with_pending_state(|s| s.size = Some((640, 400).into()));
        self.space.map_element(
            Window::new_wayland_window(surface),
            (50 + n % 6 * 90, 45 + n % 6 * 65),
            false,
        );
    }
    fn new_popup(&mut self, surface: PopupSurface, positioner: PositionerState) {
        surface.with_pending_state(|s| s.geometry = positioner.get_geometry());
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
        surface.with_pending_state(|s| {
            s.geometry = positioner.get_geometry();
            s.positioner = positioner;
        });
        surface.send_repositioned(token);
    }
    fn grab(
        &mut self,
        _: PopupSurface,
        _: smithay::reexports::wayland_server::protocol::wl_seat::WlSeat,
        _: smithay::utils::Serial,
    ) {
        // Popup rendering is supported; modal popup grabs are outside this probe.
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
    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        let client = focused.and_then(|s| self.dh.get_client(s.id()).ok());
        set_data_device_focus(&self.dh, seat, client);
    }
}
impl SelectionHandler for State {
    type SelectionUserData = ();
}
impl DataDeviceHandler for State {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device
    }
}
impl ClientDndGrabHandler for State {}
impl ServerDndGrabHandler for State {}
impl OutputHandler for State {}
smithay::delegate_compositor!(State);
smithay::delegate_shm!(State);
smithay::delegate_xdg_shell!(State);
smithay::delegate_seat!(State);
smithay::delegate_data_device!(State);
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
