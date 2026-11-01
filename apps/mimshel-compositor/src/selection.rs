//! Seat selections and the asynchronous XWayland selection bridge.
use crate::state::State;
use smithay::{
    input::Seat,
    reexports::wayland_server::Resource,
    wayland::{
        seat::WaylandFocus,
        selection::{
            SelectionHandler, SelectionSource, SelectionTarget,
            data_device::{
                self, ClientDndGrabHandler, DataDeviceHandler, DataDeviceState,
                ServerDndGrabHandler,
            },
            primary_selection::{self, PrimarySelectionHandler, PrimarySelectionState},
        },
    },
    xwayland::xwm::XwmId,
};
use std::os::fd::OwnedFd;

impl DataDeviceHandler for State {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device
    }
}
impl PrimarySelectionHandler for State {
    fn primary_selection_state(&self) -> &PrimarySelectionState {
        &self.primary_selection
    }
}
impl ClientDndGrabHandler for State {}
impl ServerDndGrabHandler for State {}
impl SelectionHandler for State {
    type SelectionUserData = XwmId;
    fn new_selection(
        &mut self,
        target: SelectionTarget,
        source: Option<SelectionSource>,
        _: Seat<Self>,
    ) {
        let index = match target {
            SelectionTarget::Clipboard => 0,
            SelectionTarget::Primary => 1,
        };
        self.wayland_selections[index] = source.as_ref().map(SelectionSource::mime_types);
        for wm in &mut self.xwms {
            if let Err(error) = wm.new_selection(target, self.wayland_selections[index].clone()) {
                eprintln!("X11 selection: {error}");
            }
        }
    }
    fn send_selection(
        &mut self,
        target: SelectionTarget,
        mime: String,
        fd: OwnedFd,
        _: Seat<Self>,
        id: &XwmId,
    ) {
        if self.protocols.locked {
            return;
        }
        if let Some(wm) = self.xwms.iter_mut().find(|wm| wm.id() == *id)
            && let Err(error) = wm.send_selection(target, mime, fd, self.selection_loop.clone())
        {
            eprintln!("X11 selection transfer: {error}");
        }
    }
}
impl State {
    pub(crate) fn selection_focus(
        &mut self,
        seat: &Seat<Self>,
        focus: Option<&crate::xwayland::Focus>,
    ) {
        let client = focus
            .and_then(WaylandFocus::wl_surface)
            .and_then(|surface| surface.client());
        data_device::set_data_device_focus(&self.dh, seat, client.clone());
        primary_selection::set_primary_focus(&self.dh, seat, client);
    }
    pub(crate) fn x11_selection(&mut self, id: XwmId, target: SelectionTarget, mimes: Vec<String>) {
        if self.protocols.locked {
            return;
        }
        match target {
            SelectionTarget::Clipboard => {
                data_device::set_data_device_selection(&self.dh, &self.seat, mimes, id)
            }
            SelectionTarget::Primary => {
                primary_selection::set_primary_selection(&self.dh, &self.seat, mimes, id)
            }
        }
    }
    pub(crate) fn clear_x11_selection(&mut self, id: XwmId, target: SelectionTarget) {
        match target {
            SelectionTarget::Clipboard
                if data_device::current_data_device_selection_userdata(&self.seat).as_deref()
                    == Some(&id) =>
            {
                data_device::clear_data_device_selection(&self.dh, &self.seat)
            }
            SelectionTarget::Primary
                if primary_selection::current_primary_selection_userdata(&self.seat).as_deref()
                    == Some(&id) =>
            {
                primary_selection::clear_primary_selection(&self.dh, &self.seat)
            }
            _ => {}
        }
    }
    pub(crate) fn send_wayland_selection(
        &mut self,
        target: SelectionTarget,
        mime: String,
        fd: OwnedFd,
    ) {
        let result = match target {
            SelectionTarget::Clipboard => {
                data_device::request_data_device_client_selection(&self.seat, mime, fd)
                    .map_err(|e| e.to_string())
            }
            SelectionTarget::Primary => {
                primary_selection::request_primary_client_selection(&self.seat, mime, fd)
                    .map_err(|e| e.to_string())
            }
        };
        if let Err(error) = result {
            eprintln!("Wayland selection transfer: {error}");
        }
    }
}
smithay::delegate_data_device!(State);
smithay::delegate_primary_selection!(State);
