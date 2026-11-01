//! Shell protocols and the policy shared by rendering and input.

mod foreign;
mod lock;

use crate::{model::Rect, state::State};
use smithay::{
    desktop::{LayerSurface, WindowSurfaceType, layer_map_for_output},
    reexports::{
        calloop::LoopHandle,
        wayland_server::{
            DisplayHandle, Resource,
            protocol::{wl_output::WlOutput, wl_surface::WlSurface},
        },
    },
    utils::{Logical, Point},
    wayland::{
        foreign_toplevel_list::{
            ForeignToplevelHandle, ForeignToplevelListHandler, ForeignToplevelListState,
        },
        idle_notify::{IdleNotifierHandler, IdleNotifierState},
        session_lock::{LockSurface, SessionLockManagerState, SessionLocker},
        shell::wlr_layer::{
            self, KeyboardInteractivity, Layer, WlrLayerShellHandler, WlrLayerShellState,
        },
        xdg_activation::{
            XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData,
        },
    },
};

/// Persistent shell resources; no polling timers or per-frame lists.
pub struct Protocols {
    layer: WlrLayerShellState,
    layers: Vec<LayerSurface>,
    foreign: ForeignToplevelListState,
    toplevels: Vec<(crate::model::WindowId, ForeignToplevelHandle)>,
    management: foreign::Management,
    pub(crate) toplevels_dirty: bool,
    activation: XdgActivationState,
    idle: IdleNotifierState<State>,
    lock: SessionLockManagerState,
    /// Remains set if the lock client disconnects.
    pub locked: bool,
    locker: Option<SessionLocker>,
    owner: Option<smithay::reexports::wayland_protocols::ext::session_lock::v1::server::ext_session_lock_v1::ExtSessionLockV1>,
    /// Lock surfaces for the current owner.
    pub locks: Vec<LockSurface>,
}

impl Protocols {
    pub(crate) fn new(dh: &DisplayHandle, handle: LoopHandle<'static, State>) -> Self {
        Self {
            layer: WlrLayerShellState::new::<State>(dh),
            layers: Vec::new(),
            foreign: ForeignToplevelListState::new::<State>(dh),
            toplevels: Vec::new(),
            management: foreign::Management::new(dh),
            toplevels_dirty: true,
            activation: XdgActivationState::new::<State>(dh),
            idle: IdleNotifierState::new(dh, handle),
            lock: SessionLockManagerState::new::<State, _>(dh, |_| true),
            locked: false,
            locker: None,
            owner: None,
            locks: Vec::new(),
        }
    }
}

impl State {
    /// Reset shell idle timers on actual seat input.
    pub fn notify_activity(&mut self) {
        self.protocols.idle.notify_activity(&self.seat);
    }

    pub(crate) fn refresh_protocols(&mut self) {
        self.refresh_lock();
        self.arrange_layers();
        self.publish_toplevels();
    }

    pub(crate) fn commit_layers(&mut self, surface: &WlSurface) {
        let Some(output) = self.space.outputs().next().cloned() else {
            return;
        };
        let Some(layer) = self
            .protocols
            .layers
            .iter()
            .find(|l| l.wl_surface() == surface)
            .cloned()
        else {
            return;
        };
        let initial = smithay::wayland::compositor::with_states(surface, |s| {
            s.data_map
                .get::<wlr_layer::LayerSurfaceData>()
                .and_then(|d| d.lock().ok())
                .is_some_and(|d| d.initial_configure_sent)
        });
        let mut map = layer_map_for_output(&output);
        if buffered(surface) || !initial {
            if let Err(e) = map.map_layer(&layer) {
                eprintln!("layer map: {e}");
            }
            map.arrange();
            if !initial {
                layer.layer_surface().send_configure();
            }
        }
        // A NULL buffer unmaps a layer. An initial bufferless commit only
        // negotiates size; it must not reserve space before the panel is shown.
        if !buffered(surface) {
            map.unmap_layer(&layer);
        }
        drop(map);
        self.arrange_layers();
        self.sync_focus();
    }

    /// Recompute the work area from committed anchors, margins and exclusive zones.
    pub fn arrange_layers(&mut self) {
        let Some(output) = self.space.outputs().next().cloned() else {
            return;
        };
        let mut map = layer_map_for_output(&output);
        self.protocols.layers.retain(|l| l.layer_surface().alive());
        map.cleanup();
        map.arrange();
        let zone = map.non_exclusive_zone();
        drop(map);
        let area = Rect {
            x: zone.loc.x,
            y: zone.loc.y,
            width: zone.size.w.max(1),
            height: zone.size.h.max(1),
        };
        if self.model.area != area {
            self.model.set_work_area(area);
            self.configure_windows();
        }
    }

    pub(crate) fn exclusive_layer_focus(&self) -> Option<WlSurface> {
        let output = self.space.outputs().next()?;
        let map = layer_map_for_output(output);
        for kind in [Layer::Overlay, Layer::Top] {
            if let Some(layer) = map.layers_on(kind).rev().find(|l| {
                l.cached_state().keyboard_interactivity == KeyboardInteractivity::Exclusive
                    && buffered(l.wl_surface())
            }) {
                return Some(layer.wl_surface().clone());
            }
        }
        None
    }

    pub(crate) fn current_layer_focus(&self) -> Option<WlSurface> {
        let focus = self.keyboard.current_focus()?;
        let output = self.space.outputs().next()?;
        let map = layer_map_for_output(output);
        map.layer_for_surface(&focus, WindowSurfaceType::ALL)
            .filter(|l| l.can_receive_keyboard_focus() && buffered(l.wl_surface()))
            .map(|_| focus)
    }

    pub(crate) fn leave_on_demand_layer(&mut self) {
        if self.current_layer_focus().is_some() && self.exclusive_layer_focus().is_none() {
            self.keyboard.clone().set_focus(
                self,
                None,
                smithay::utils::SERIAL_COUNTER.next_serial(),
            );
        }
    }

    pub(crate) fn layer_under(&self, above: bool) -> Option<(WlSurface, Point<f64, Logical>)> {
        let output = self.space.outputs().next()?;
        let map = layer_map_for_output(output);
        for kind in if above {
            [Layer::Overlay, Layer::Top]
        } else {
            [Layer::Bottom, Layer::Background]
        } {
            for layer in map.layers_on(kind).rev() {
                if !buffered(layer.wl_surface()) {
                    continue;
                }
                let geometry = map.layer_geometry(layer)?;
                if let Some((surface, offset)) = layer
                    .surface_under(self.pointer - geometry.loc.to_f64(), WindowSurfaceType::ALL)
                {
                    return Some((surface, (geometry.loc + offset).to_f64()));
                }
            }
        }
        None
    }

    /// Consume clicks on layer surfaces, including on-demand keyboard focus.
    pub(crate) fn focus_shell_click(&mut self) -> bool {
        if self.protocols.locked {
            self.focus_lock();
            return true;
        }
        let Some((surface, _)) = self.layer_under(true).or_else(|| {
            if self.space.element_under(self.pointer).is_none() {
                self.layer_under(false)
            } else {
                None
            }
        }) else {
            return false;
        };
        let interactive = self.space.outputs().next().is_some_and(|o| {
            layer_map_for_output(o)
                .layer_for_surface(&surface, WindowSurfaceType::ALL)
                .is_some_and(|l| l.can_receive_keyboard_focus())
        });
        if interactive && self.exclusive_layer_focus().is_none() {
            self.keyboard.clone().set_focus(
                self,
                Some(surface),
                smithay::utils::SERIAL_COUNTER.next_serial(),
            );
        }
        true
    }

    pub(crate) fn activate_window(&mut self, id: crate::model::WindowId) {
        if self.protocols.locked {
            return;
        }
        self.leave_on_demand_layer();
        if let Some((_, window)) = self.windows.iter().find(|(i, _)| *i == id)
            && window.toplevel().is_some()
        {
            if self
                .model
                .windows
                .iter()
                .any(|w| w.id == id && w.mode == crate::model::Mode::Minimized)
            {
                self.model.restore(id);
                self.configure_windows();
            }
            self.model.raise(id);
            self.sync_positions();
            self.sync_focus();
        }
    }
}

pub(crate) fn buffered(surface: &WlSurface) -> bool {
    smithay::backend::renderer::utils::with_renderer_surface_state(surface, |s| {
        s.buffer().is_some()
    })
    .unwrap_or(false)
}

impl WlrLayerShellHandler for State {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.protocols.layer
    }
    fn new_layer_surface(
        &mut self,
        surface: wlr_layer::LayerSurface,
        output: Option<WlOutput>,
        _: Layer,
        namespace: String,
    ) {
        let output = output
            .as_ref()
            .and_then(smithay::output::Output::from_resource)
            .or_else(|| self.space.outputs().next().cloned());
        if let Some(output) = output {
            let _ = output;
            self.protocols
                .layers
                .push(LayerSurface::new(surface, namespace));
        }
    }
    fn layer_destroyed(&mut self, surface: wlr_layer::LayerSurface) {
        self.protocols
            .layers
            .retain(|l| l.layer_surface() != &surface);
        if let Some(output) = self.space.outputs().next().cloned() {
            let mut map = layer_map_for_output(&output);
            let layer = map
                .layers()
                .find(|l| l.layer_surface() == &surface)
                .cloned();
            if let Some(layer) = layer {
                map.unmap_layer(&layer);
            }
        }
        self.arrange_layers();
        self.sync_focus();
        self.dirty = true;
    }
    fn new_popup(
        &mut self,
        _: wlr_layer::LayerSurface,
        popup: smithay::wayland::shell::xdg::PopupSurface,
    ) {
        let positioner = popup.with_pending_state(|s| s.positioner);
        let geometry = self.popup_geometry(&popup, positioner);
        popup.with_pending_state(|s| s.geometry = geometry);
        if self.popups.find_popup(popup.wl_surface()).is_none()
            && let Err(e) = self
                .popups
                .track_popup(smithay::desktop::PopupKind::Xdg(popup))
        {
            eprintln!("layer popup: {e}");
        }
    }
}
impl ForeignToplevelListHandler for State {
    fn foreign_toplevel_list_state(&mut self) -> &mut ForeignToplevelListState {
        &mut self.protocols.foreign
    }
}
impl IdleNotifierHandler for State {
    fn idle_notifier_state(&mut self) -> &mut IdleNotifierState<Self> {
        &mut self.protocols.idle
    }
}
impl XdgActivationHandler for State {
    fn activation_state(&mut self) -> &mut XdgActivationState {
        &mut self.protocols.activation
    }
    fn request_activation(
        &mut self,
        token: XdgActivationToken,
        data: XdgActivationTokenData,
        surface: WlSurface,
    ) {
        self.protocols.activation.remove_token(&token);
        if data.timestamp.elapsed() > std::time::Duration::from_secs(10) || self.protocols.locked {
            return;
        }
        if let Some((id, _)) = self
            .windows
            .iter()
            .find(|(_, w)| w.toplevel().is_some_and(|t| t.wl_surface() == &surface))
        {
            self.activate_window(*id);
        }
    }
}
smithay::delegate_layer_shell!(State);
smithay::delegate_foreign_toplevel_list!(State);
smithay::delegate_idle_notify!(State);
smithay::delegate_xdg_activation!(State);
