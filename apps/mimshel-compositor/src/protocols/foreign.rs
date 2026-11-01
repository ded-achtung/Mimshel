//! wlr management requests supplement Smithay's ext window list.
use super::*;
use smithay::reexports::{
    wayland_protocols_wlr::foreign_toplevel::v1::server::{
        zwlr_foreign_toplevel_handle_v1::{self, ZwlrForeignToplevelHandleV1},
        zwlr_foreign_toplevel_manager_v1::{self, ZwlrForeignToplevelManagerV1},
    },
    wayland_server::{Client, DataInit, Dispatch, GlobalDispatch, New},
};

pub(super) struct Management {
    managers: Vec<ZwlrForeignToplevelManagerV1>,
    handles: Vec<(
        crate::model::WindowId,
        ZwlrForeignToplevelManagerV1,
        ZwlrForeignToplevelHandleV1,
    )>,
    published: Vec<(
        crate::model::WindowId,
        String,
        String,
        Option<crate::model::Mode>,
        bool,
    )>,
}
impl Management {
    pub(super) fn new(dh: &DisplayHandle) -> Self {
        dh.create_global::<State, ZwlrForeignToplevelManagerV1, _>(3, ());
        Self {
            managers: Vec::new(),
            handles: Vec::new(),
            published: Vec::new(),
        }
    }
}

impl State {
    pub(crate) fn publish_toplevels(&mut self) {
        if !self.protocols.toplevels_dirty {
            return;
        }
        self.protocols.toplevels_dirty = false;
        self.protocols.toplevels.retain(|(id, handle)| {
            if self.windows.iter().any(|(i, _)| i == id) {
                true
            } else {
                handle.send_closed();
                false
            }
        });
        self.protocols.management.handles.retain(|(id, _, handle)| {
            if self.windows.iter().any(|(i, _)| i == id) {
                true
            } else {
                handle.closed();
                false
            }
        });
        self.protocols
            .management
            .managers
            .retain(Resource::is_alive);
        self.protocols
            .management
            .published
            .retain(|(id, ..)| self.windows.iter().any(|(i, _)| i == id));
        for (id, window) in &self.windows {
            let Some(top) = window.toplevel() else {
                continue;
            };
            let (title, app_id) = metadata(top.wl_surface());
            let mode = self
                .model
                .windows
                .iter()
                .find(|w| w.id == *id)
                .map(|w| w.mode);
            let active = self.model.focus == Some(*id);
            let changed = !self
                .protocols
                .management
                .published
                .iter()
                .any(|(i, t, a, m, f)| {
                    i == id && t == &title && a == &app_id && *m == mode && *f == active
                });
            if changed {
                self.protocols
                    .management
                    .published
                    .retain(|(i, ..)| i != id);
                self.protocols.management.published.push((
                    *id,
                    title.clone(),
                    app_id.clone(),
                    mode,
                    active,
                ));
            }
            if let Some((_, handle)) = self.protocols.toplevels.iter().find(|(i, _)| i == id) {
                if changed {
                    handle.send_title(&title);
                    handle.send_app_id(&app_id);
                    // Smithay suppresses unchanged metadata; done delimits updates.
                    handle.send_done();
                }
            } else {
                let handle = self
                    .protocols
                    .foreign
                    .new_toplevel::<State>(&title, &app_id);
                self.protocols.toplevels.push((*id, handle));
            }
            for manager in &self.protocols.management.managers {
                let Some(client) = manager.client() else {
                    continue;
                };
                if !self
                    .protocols
                    .management
                    .handles
                    .iter()
                    .any(|(i, m, _)| i == id && m == manager)
                    && let Ok(handle) = client
                        .create_resource::<ZwlrForeignToplevelHandleV1, _, State>(
                            &self.dh,
                            manager.version(),
                            *id,
                        )
                {
                    manager.toplevel(&handle);
                    for output in self.space.outputs() {
                        for resource in output.client_outputs(&client) {
                            handle.output_enter(&resource);
                        }
                    }
                    send_metadata(&handle, &title, &app_id, mode, active);
                    self.protocols
                        .management
                        .handles
                        .push((*id, manager.clone(), handle));
                }
            }
            for (_, _, handle) in self
                .protocols
                .management
                .handles
                .iter()
                .filter(|(i, _, h)| i == id && changed && h.is_alive())
            {
                send_metadata(handle, &title, &app_id, mode, active);
            }
        }
    }
}
fn metadata(surface: &WlSurface) -> (String, String) {
    smithay::wayland::compositor::with_states(surface, |states| {
        let data = states
            .data_map
            .get::<smithay::wayland::shell::xdg::XdgToplevelSurfaceData>()
            .and_then(|d| d.lock().ok());
        data.map(|d| {
            (
                d.title.clone().unwrap_or_default(),
                d.app_id.clone().unwrap_or_default(),
            )
        })
        .unwrap_or_default()
    })
}
fn send_metadata(
    handle: &ZwlrForeignToplevelHandleV1,
    title: &str,
    app_id: &str,
    mode: Option<crate::model::Mode>,
    active: bool,
) {
    use crate::model::Mode;
    use zwlr_foreign_toplevel_handle_v1::State as W;
    handle.title(title.to_owned());
    handle.app_id(app_id.to_owned());
    let mut states = Vec::with_capacity(8);
    let state = match mode {
        Some(Mode::Maximized) => Some(W::Maximized),
        Some(Mode::Minimized) => Some(W::Minimized),
        Some(Mode::Fullscreen) if handle.version() >= 2 => Some(W::Fullscreen),
        _ => None,
    };
    if let Some(state) = state {
        states.extend_from_slice(&(state as u32).to_ne_bytes());
    }
    if active {
        states.extend_from_slice(&(W::Activated as u32).to_ne_bytes());
    }
    handle.state(states);
    handle.done();
}
impl GlobalDispatch<ZwlrForeignToplevelManagerV1, ()> for State {
    fn bind(
        state: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<ZwlrForeignToplevelManagerV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        let manager = init.init(resource, ());
        state.protocols.management.managers.push(manager);
        state.protocols.toplevels_dirty = true;
        state.publish_toplevels();
    }
}
impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for State {
    fn request(
        state: &mut Self,
        _: &Client,
        manager: &ZwlrForeignToplevelManagerV1,
        request: zwlr_foreign_toplevel_manager_v1::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if let zwlr_foreign_toplevel_manager_v1::Request::Stop = request {
            state.protocols.management.managers.retain(|m| m != manager);
            manager.finished();
        }
    }
}
impl Dispatch<ZwlrForeignToplevelHandleV1, crate::model::WindowId> for State {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &ZwlrForeignToplevelHandleV1,
        request: zwlr_foreign_toplevel_handle_v1::Request,
        id: &crate::model::WindowId,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        use crate::model::Mode;
        use zwlr_foreign_toplevel_handle_v1::Request as R;
        if state.protocols.locked {
            return;
        }
        let Some(top) = state
            .windows
            .iter()
            .find(|(i, _)| i == id)
            .and_then(|(_, w)| w.toplevel().cloned())
        else {
            return;
        };
        match request {
            R::Activate { seat }
                if smithay::input::Seat::<Self>::from_resource(&seat).as_ref()
                    == Some(&state.seat) =>
            {
                state.activate_window(*id)
            }
            R::SetRectangle { width, height, .. } if width < 0 || height < 0 => {
                resource.post_error(
                    zwlr_foreign_toplevel_handle_v1::Error::InvalidRectangle,
                    "Negative rectangle size",
                );
            }
            R::Close => top.send_close(),
            R::SetMaximized => state.request_mode(&top, Mode::Maximized),
            R::UnsetMaximized => state.request_mode(&top, Mode::Normal),
            R::SetMinimized => state.request_mode(&top, Mode::Minimized),
            R::UnsetMinimized => {
                state.model.restore(*id);
                state.configure_windows();
            }
            R::SetFullscreen { .. } => state.request_mode(&top, Mode::Fullscreen),
            R::UnsetFullscreen => state.request_mode(&top, Mode::Normal),
            _ => {}
        }
    }
}
