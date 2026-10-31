//! Damage tracking and GPU completion timing; presentation wait is excluded.
use crate::{Error, state::State};
use smithay::{
    backend::{
        renderer::{
            damage::OutputDamageTracker,
            element::{AsRenderElements, surface::WaylandSurfaceRenderElement},
            gles::GlesRenderer,
        },
        winit::WinitGraphicsBackend,
    },
    output::Output,
};
use std::time::Instant;
pub fn frame(
    backend: &mut WinitGraphicsBackend<GlesRenderer>,
    state: &State,
    tracker: &mut OutputDamageTracker,
) -> Result<(), Error> {
    let start = Instant::now();
    let age = backend.buffer_age().unwrap_or(0);
    let (renderer, mut fb) = backend.bind()?;
    let mut elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = Vec::new();
    let windows: Vec<_> = state.space.elements().collect();
    for window in windows.into_iter().rev() {
        let location = state.space.element_location(window).unwrap() - window.geometry().loc;
        elements.extend(window.render_elements(
            renderer,
            location.to_physical_precise_round(1.0),
            1.0.into(),
            1.0,
        ));
    }
    let result =
        tracker.render_output(renderer, &mut fb, age, &elements, [0.025, 0.035, 0.06, 1.0])?;
    result.sync.wait()?;
    let elapsed = start.elapsed();
    let damage = result.damage.cloned();
    drop(fb);
    if let Some(damage) = damage {
        if std::env::var_os("MIMSHEL_TIMING").as_deref() == Some(std::ffi::OsStr::new("1")) {
            eprintln!("frame render={:.3}ms", elapsed.as_secs_f64() * 1000.0);
        }
        backend.submit(Some(&damage))?;
    }
    Ok(())
}
pub fn callbacks(state: &State, output: &Output, elapsed: std::time::Duration) {
    for (_, window) in &state.windows {
        window.send_frame(output, elapsed, Some(std::time::Duration::ZERO), |_, _| {
            Some(output.clone())
        });
    }
}
