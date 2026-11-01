//! A lock is a persistent compositor state, independent of the locking client.
use super::*;
use smithay::{
    reexports::{
        wayland_protocols::ext::session_lock::v1::server::{
            ext_session_lock_manager_v1::ExtSessionLockManagerV1,
            ext_session_lock_surface_v1::ExtSessionLockSurfaceV1,
            ext_session_lock_v1::{self, ExtSessionLockV1},
        },
        wayland_server::{Client, DataInit, Dispatch},
    },
    wayland::session_lock::{ExtLockSurfaceUserData, SessionLockHandler, SessionLockState},
};

impl State {
    pub(crate) fn refresh_lock(&mut self) {
        if self.protocols.owner.as_ref().is_some_and(|o| !o.is_alive()) {
            self.protocols.owner = None;
            self.protocols.locker = None;
            self.protocols.locks.clear();
            self.dirty = true;
            self.focus_lock();
        }
        self.protocols.locks.retain(LockSurface::alive);
    }

    pub(crate) fn configure_locks(&mut self) {
        let Some(output) = self.space.outputs().next() else {
            return;
        };
        let Some(mode) = output.current_mode() else {
            return;
        };
        let size = mode
            .size
            .to_f64()
            .to_logical(output.current_scale().fractional_scale())
            .to_i32_round::<i32>();
        let size = (size.w as u32, size.h as u32).into();
        for surface in &self.protocols.locks {
            if surface.with_pending_state(|s| {
                if s.size == Some(size) {
                    false
                } else {
                    s.size = Some(size);
                    true
                }
            }) {
                surface.send_configure();
            }
        }
    }

    /// Called only after the backend has presented a frame containing no session data.
    pub fn lock_frame_presented(&mut self) {
        if self.protocols.locked
            && let Some(locker) = self.protocols.locker.take()
        {
            locker.lock();
        }
    }

    pub(crate) fn focus_lock(&mut self) {
        let focus = self
            .protocols
            .locks
            .iter()
            .find(|s| s.alive() && buffered(s.wl_surface()))
            .map(|s| s.wl_surface().clone());
        self.keyboard
            .clone()
            .set_focus(self, focus, smithay::utils::SERIAL_COUNTER.next_serial());
    }

    pub(crate) fn lock_under(&self) -> Option<(WlSurface, Point<f64, Logical>)> {
        self.protocols
            .locks
            .iter()
            .find(|s| s.alive() && buffered(s.wl_surface()))
            .map(|s| (s.wl_surface().clone(), (0.0, 0.0).into()))
    }
}
impl SessionLockHandler for State {
    fn lock_state(&mut self) -> &mut SessionLockManagerState {
        &mut self.protocols.lock
    }
    fn lock(&mut self, confirmation: SessionLocker) {
        self.refresh_lock();
        if self.protocols.owner.is_some() {
            return;
        }
        self.protocols.locked = true;
        self.protocols.owner = Some(confirmation.ext_session_lock().clone());
        self.protocols.locker = Some(confirmation);
        self.drag = None;
        self.hovered = None;
        self.decoration_pressed = false;
        self.pointer_handle.clone().unset_grab(
            self,
            smithay::utils::SERIAL_COUNTER.next_serial(),
            0,
        );
        self.keyboard.clone().unset_grab(self);
        self.focus_lock();
        self.pointer_handle.clone().motion(
            self,
            None,
            &smithay::input::pointer::MotionEvent {
                location: self.pointer,
                serial: smithay::utils::SERIAL_COUNTER.next_serial(),
                time: 0,
            },
        );
        self.pointer_handle.clone().frame(self);
        self.dirty = true;
    }
    fn unlock(&mut self) {
        self.protocols.locked = false;
        self.protocols.owner = None;
        self.protocols.locker = None;
        self.protocols.locks.clear();
        self.sync_focus();
        self.dirty = true;
    }
    fn new_surface(&mut self, surface: LockSurface, output: WlOutput) {
        if let Some(output) = smithay::output::Output::from_resource(&output)
            && let Some(mode) = output.current_mode()
        {
            let size = mode
                .size
                .to_f64()
                .to_logical(output.current_scale().fractional_scale())
                .to_i32_round::<i32>();
            surface.with_pending_state(|s| s.size = Some((size.w as u32, size.h as u32).into()));
            self.protocols.locks.push(surface);
        }
    }
}
// Filter requests before Smithay's handler: a rejected lock must never unlock
// the active session or create surfaces belonging to another lock owner.
impl Dispatch<ExtSessionLockV1, SessionLockState> for State {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &ExtSessionLockV1,
        request: ext_session_lock_v1::Request,
        data: &SessionLockState,
        dh: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if state.protocols.owner.as_ref() != Some(resource) {
            if let ext_session_lock_v1::Request::GetLockSurface { id, .. } = request {
                init.post_error(
                    id,
                    ext_session_lock_v1::Error::Role,
                    "Inactive session lock",
                );
            }
            return;
        }
        if matches!(request, ext_session_lock_v1::Request::UnlockAndDestroy)
            && state.protocols.locker.is_some()
        {
            resource.post_error(
                ext_session_lock_v1::Error::InvalidUnlock,
                "Lock has not been acknowledged",
            );
            return;
        }
        <SessionLockManagerState as Dispatch<ExtSessionLockV1, SessionLockState, Self>>::request(
            state, client, resource, request, data, dh, init,
        );
    }
}
smithay::reexports::wayland_server::delegate_global_dispatch!(State: [ExtSessionLockManagerV1: smithay::wayland::session_lock::SessionLockManagerGlobalData] => SessionLockManagerState);
smithay::reexports::wayland_server::delegate_dispatch!(State: [ExtSessionLockManagerV1: ()] => SessionLockManagerState);
smithay::reexports::wayland_server::delegate_dispatch!(State: [ExtSessionLockSurfaceV1: ExtLockSurfaceUserData] => SessionLockManagerState);
