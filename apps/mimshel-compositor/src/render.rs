//! Damage tracking and GPU completion timing; presentation wait is excluded.

use crate::{Error, state::State};
use smithay::{
    backend::{
        renderer::{
            damage::OutputDamageTracker,
            element::{Kind, surface::WaylandSurfaceRenderElement},
            gles::GlesRenderer,
        },
        winit::WinitGraphicsBackend,
    },
    output::Output,
};
use std::time::Instant;

/// Reusable render storage and startup timing policy.
pub struct RenderState {
    elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>>,
    timing: bool,
}

impl Default for RenderState {
    fn default() -> Self {
        Self {
            elements: Vec::new(),
            timing: std::env::var_os("MIMSHEL_TIMING").as_deref()
                == Some(std::ffi::OsStr::new("1")),
        }
    }
}

/// Render damage using reusable storage and optional GPU timing.
pub fn frame(
    backend: &mut WinitGraphicsBackend<GlesRenderer>,
    state: &State,
    tracker: &mut OutputDamageTracker,
    render: &mut RenderState,
) -> Result<(), Error> {
    let start = Instant::now();
    let age = backend.buffer_age().unwrap_or(0);
    let (renderer, mut fb) = backend.bind()?;
    render.elements.clear();
    for window in state.space.elements().rev() {
        let Some(origin) = state.space.element_location(window) else {
            continue;
        };
        let location = origin - window.geometry().loc;
        let Some(top) = window.toplevel() else {
            continue;
        };
        for (popup, offset) in smithay::desktop::PopupManager::popups_for_surface(top.wl_surface())
        {
            let offset = window.geometry().loc + offset - popup.geometry().loc;
            append_surface(
                renderer,
                popup.wl_surface(),
                location + offset,
                &mut render.elements,
            );
        }
        append_surface(renderer, top.wl_surface(), location, &mut render.elements);
    }
    let result = tracker.render_output(
        renderer,
        &mut fb,
        age,
        &render.elements,
        [0.025, 0.035, 0.06, 1.0],
    )?;
    if render.timing {
        result.sync.wait()?;
    }
    let elapsed = start.elapsed();
    drop(fb);
    if let Some(damage) = result.damage {
        if render.timing {
            eprintln!("frame render={:.3}ms", elapsed.as_secs_f64() * 1000.0);
        }
        backend.submit(Some(damage))?;
    }
    Ok(())
}

/// Deliver frame callbacks to mapped windows.
pub fn callbacks(state: &State, output: &Output, elapsed: std::time::Duration) {
    for (_, window) in &state.windows {
        window.send_frame(output, elapsed, Some(std::time::Duration::ZERO), |_, _| {
            Some(output.clone())
        });
    }
}

/// Append mapped subsurfaces directly, preserving Smithay's front-to-back order.
fn append_surface(
    renderer: &mut GlesRenderer,
    surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    location: smithay::utils::Point<i32, smithay::utils::Logical>,
    elements: &mut Vec<WaylandSurfaceRenderElement<GlesRenderer>>,
) {
    use smithay::{
        backend::renderer::utils::RendererSurfaceStateUserData,
        wayland::compositor::{TraversalAction, with_surface_tree_downward},
    };
    with_surface_tree_downward(
        surface,
        location.to_f64().to_physical(1.0),
        |_, states, location| {
            let view = states
                .data_map
                .get::<RendererSurfaceStateUserData>()
                .and_then(|data| data.lock().ok().and_then(|data| data.view()));
            match view {
                Some(view) => {
                    TraversalAction::DoChildren(*location + view.offset.to_f64().to_physical(1.0))
                }
                None => TraversalAction::SkipChildren,
            }
        },
        |surface, states, location| {
            let view = states
                .data_map
                .get::<RendererSurfaceStateUserData>()
                .and_then(|data| data.lock().ok().and_then(|data| data.view()));
            if let Some(view) = view {
                let location = *location + view.offset.to_f64().to_physical(1.0);
                match WaylandSurfaceRenderElement::from_surface(
                    renderer,
                    surface,
                    states,
                    location,
                    1.0,
                    Kind::Unspecified,
                ) {
                    Ok(Some(element)) => elements.push(element),
                    Ok(None) => {}
                    Err(error) => eprintln!("surface import: {error}"),
                }
            }
        },
        |_, _, _| true,
    );
}
