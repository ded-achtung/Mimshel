//! Damage tracking and GPU completion timing; presentation wait is excluded.

use crate::{Error, state::State};
use smithay::backend::renderer::element::{
    Element, Id, RenderElement,
    memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
    render_elements,
    solid::SolidColorRenderElement,
};
use smithay::utils::Transform;
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

render_elements! {
    Elements<=GlesRenderer>;
    Surface=WaylandSurfaceRenderElement<GlesRenderer>,
    Decoration=MemoryRenderBufferRenderElement<GlesRenderer>,
    Border=Border,
}

/// A render source selected before importing buffers into GLES.
#[derive(Clone)]
pub enum SceneElement {
    /// A desktop window, including its decorations and popups.
    Window(smithay::desktop::Window),
    /// A shell surface and its logical origin.
    Surface {
        /// Surface tree to import.
        surface: smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
        /// Logical output location.
        location: smithay::utils::Point<i32, smithay::utils::Logical>,
        /// This source belongs to the session lock.
        lock: bool,
    },
}

/// Select all render sources, front to back. The lock branch is exhaustive:
/// neither windows, decorations, layer surfaces nor their popups can enter it.
pub fn scene(state: &State, elements: &mut Vec<SceneElement>) {
    elements.clear();
    if state.protocols.locked {
        for lock in &state.protocols.locks {
            if lock.alive() && crate::protocols::buffered(lock.wl_surface()) {
                elements.push(SceneElement::Surface {
                    surface: lock.wl_surface().clone(),
                    location: (0, 0).into(),
                    lock: true,
                });
            }
        }
        return;
    }
    if let Some((surface, offset)) = &state.dnd_icon {
        elements.push(SceneElement::Surface {
            surface: surface.clone(),
            location: state.pointer.to_i32_round() + *offset,
            lock: false,
        });
    }
    use smithay::wayland::shell::wlr_layer::Layer;
    let layers = |above: bool, elements: &mut Vec<SceneElement>| {
        if let Some(output) = state.space.outputs().next() {
            let map = smithay::desktop::layer_map_for_output(output);
            for kind in if above {
                [Layer::Overlay, Layer::Top]
            } else {
                [Layer::Bottom, Layer::Background]
            } {
                for layer in map.layers_on(kind).rev() {
                    if !crate::protocols::buffered(layer.wl_surface()) {
                        continue;
                    }
                    if let Some(geometry) = map.layer_geometry(layer) {
                        for (popup, offset) in
                            smithay::desktop::PopupManager::popups_for_surface(layer.wl_surface())
                        {
                            elements.push(SceneElement::Surface {
                                surface: popup.wl_surface().clone(),
                                location: geometry.loc + offset - popup.geometry().loc,
                                lock: false,
                            });
                        }
                        elements.push(SceneElement::Surface {
                            surface: layer.wl_surface().clone(),
                            location: geometry.loc,
                            lock: false,
                        });
                    }
                }
            }
        }
    };
    layers(true, elements);
    elements.extend(
        state
            .space
            .elements()
            .rev()
            .cloned()
            .map(SceneElement::Window),
    );
    layers(false, elements);
}

struct DecorationCache {
    id: crate::model::WindowId,
    title: String,
    width: i32,
    height: i32,
    focused: bool,
    hover: bool,
    scale: f64,
    pixels: Option<MemoryRenderBuffer>,
    borders: [Border; 3],
}

fn fonts() -> mimshel_text::Fonts {
    mimshel_text::Fonts::with_fallback(&mimshel_core::config::load().font)
}

/// Reusable render storage and startup timing policy.
pub struct RenderState {
    elements: Vec<Elements>,
    scene: Vec<SceneElement>,
    decorations: Vec<DecorationCache>,
    fonts: mimshel_text::Fonts,
    timing: bool,
}

impl Default for RenderState {
    fn default() -> Self {
        Self {
            elements: Vec::new(),
            scene: Vec::new(),
            decorations: Vec::new(),
            fonts: fonts(),
            timing: std::env::var_os("MIMSHEL_TIMING").as_deref()
                == Some(std::ffi::OsStr::new("1")),
        }
    }
}

/// Render damage using reusable storage and optional GPU timing.
pub fn frame(
    backend: &mut WinitGraphicsBackend<GlesRenderer>,
    state: &mut State,
    tracker: &mut OutputDamageTracker,
    render: &mut RenderState,
) -> Result<(), Error> {
    let start = Instant::now();
    let age = backend.buffer_age().unwrap_or(0);
    let (renderer, mut fb) = backend.bind()?;
    render.elements.clear();
    render
        .decorations
        .retain(|cache| state.windows.iter().any(|(id, _)| *id == cache.id));
    scene(state, &mut render.scene);
    for item in &render.scene {
        let window = match item {
            SceneElement::Surface {
                surface, location, ..
            } => {
                append_surface(
                    renderer,
                    surface,
                    *location,
                    state.output_scale,
                    &mut render.elements,
                );
                continue;
            }
            SceneElement::Window(window) => window,
        };
        let Some(origin) = state.space.element_location(window) else {
            continue;
        };
        let location = origin - window.geometry().loc;
        let Some(surface) = crate::xwayland::surface(window) else {
            continue;
        };
        for (popup, offset) in smithay::desktop::PopupManager::popups_for_surface(&surface) {
            let offset = window.geometry().loc + offset - popup.geometry().loc;
            append_surface(
                renderer,
                popup.wl_surface(),
                location + offset,
                state.output_scale,
                &mut render.elements,
            );
        }
        if let Some((id, _)) = state.windows.iter().find(|(_, w)| w == window)
            && state.model.title_height(*id) > 0
            && let Some(rect) = state.model.rect(*id)
        {
            let title_height = state.model.title_height(*id);
            let focused = state.model.focus == Some(*id);
            let hover = state.hovered == Some(*id);
            let scale = state.output_scale;
            let index = match render.decorations.iter().position(|c| c.id == *id) {
                Some(index) => index,
                None => {
                    render.decorations.push(DecorationCache {
                        id: *id,
                        title: String::new(),
                        width: 0,
                        height: 0,
                        focused,
                        hover,
                        scale,
                        pixels: None,
                        borders: std::array::from_fn(|_| {
                            Border::new(
                                Id::new(),
                                smithay::utils::Rectangle::from_size((1, 1).into()),
                            )
                        }),
                    });
                    render.decorations.len() - 1
                }
            };
            let cache = &mut render.decorations[index];
            let mut update_title = |title: &str| {
                if cache.width != rect.width
                    || cache.height != title_height
                    || cache.title != title
                    || cache.focused != focused
                    || cache.hover != hover
                    || cache.scale != scale
                {
                    cache.title.clear();
                    cache.title.push_str(title);
                    cache.width = rect.width;
                    cache.height = title_height;
                    cache.focused = focused;
                    cache.hover = hover;
                    cache.scale = scale;
                    if let Some(pm) = crate::decoration::render(
                        title,
                        rect.width,
                        focused,
                        hover,
                        scale as f32,
                        &mut render.fonts,
                    ) {
                        cache.pixels = Some(MemoryRenderBuffer::from_slice(
                            pm.data(),
                            smithay::backend::allocator::Fourcc::Abgr8888,
                            (pm.width() as i32, pm.height() as i32),
                            1,
                            Transform::Normal,
                            None,
                        ));
                    }
                }
            };
            if window.is_x11() {
                let title = state
                    .model
                    .windows
                    .iter()
                    .find(|w| w.id == *id)
                    .map_or("", |w| w.x11_title.as_str());
                update_title(title);
            } else {
                smithay::wayland::compositor::with_states(&surface, |states| {
                    let data = states
                        .data_map
                        .get::<smithay::wayland::shell::xdg::XdgToplevelSurfaceData>()
                        .and_then(|d| d.lock().ok());
                    update_title(data.as_ref().and_then(|d| d.title.as_deref()).unwrap_or(""));
                });
            }
            if let Some(buffer) = &cache.pixels {
                render.elements.push(
                    MemoryRenderBufferRenderElement::from_buffer(
                        renderer,
                        (f64::from(rect.x) * scale, f64::from(rect.y) * scale),
                        buffer,
                        None,
                        Some(smithay::utils::Rectangle::from_size(
                            (
                                (rect.width as f64 * scale).round(),
                                (f64::from(title_height) * scale).round(),
                            )
                                .into(),
                        )),
                        Some((rect.width, title_height).into()),
                        Kind::Unspecified,
                    )?
                    .into(),
                );
            }
            let px = |v: i32| (f64::from(v) * scale).round() as i32;
            let (left, right, top, bottom) = (
                px(rect.x),
                px(rect.x + rect.width),
                px(rect.y + title_height),
                px(rect.y + title_height + rect.height),
            );
            for (i, (x, y, w, h)) in [
                (left - 1, top, 1, bottom - top),
                (right, top, 1, bottom - top),
                (left - 1, bottom, right - left + 2, 1),
            ]
            .into_iter()
            .enumerate()
            {
                let geometry = smithay::utils::Rectangle::new((x, y).into(), (w, h).into());
                if cache.borders[i].geometry(1.0.into()) != geometry {
                    cache.borders[i] = Border::new(cache.borders[i].id().clone(), geometry);
                }
                render.elements.push(cache.borders[i].clone().into());
            }
        }
        append_surface(
            renderer,
            &surface,
            location,
            state.output_scale,
            &mut render.elements,
        );
    }
    let result = tracker.render_output(
        renderer,
        &mut fb,
        age,
        &render.elements,
        if state.protocols.locked {
            [0.0, 0.0, 0.0, 1.0]
        } else {
            [0.025, 0.035, 0.06, 1.0]
        },
    )?;
    if render.timing || state.protocols.locked {
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
    state.lock_frame_presented();
    Ok(())
}

/// Deliver frame callbacks to mapped windows.
pub fn callbacks(state: &State, output: &Output, elapsed: std::time::Duration) {
    if state.protocols.locked {
        for lock in &state.protocols.locks {
            smithay::desktop::utils::send_frames_surface_tree(
                lock.wl_surface(),
                output,
                elapsed,
                Some(std::time::Duration::ZERO),
                |_, _| Some(output.clone()),
            );
        }
        return;
    }
    if let Some((surface, _)) = &state.dnd_icon {
        smithay::desktop::utils::send_frames_surface_tree(
            surface,
            output,
            elapsed,
            Some(std::time::Duration::ZERO),
            |_, _| Some(output.clone()),
        );
    }
    let map = smithay::desktop::layer_map_for_output(output);
    for layer in map.layers() {
        layer.send_frame(output, elapsed, Some(std::time::Duration::ZERO), |_, _| {
            Some(output.clone())
        });
    }
    drop(map);
    for window in &state.overrides {
        window.send_frame(output, elapsed, Some(std::time::Duration::ZERO), |_, _| {
            Some(output.clone())
        });
    }
    for (id, window) in &state.windows {
        if state
            .model
            .windows
            .iter()
            .any(|w| w.id == *id && w.mode == crate::model::Mode::Minimized)
        {
            continue;
        }
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
    scale: f64,
    elements: &mut Vec<Elements>,
) {
    use smithay::{
        backend::renderer::utils::RendererSurfaceStateUserData,
        wayland::compositor::{TraversalAction, with_surface_tree_downward},
    };
    with_surface_tree_downward(
        surface,
        location.to_f64().to_physical(scale),
        |_, states, location| {
            let view = states
                .data_map
                .get::<RendererSurfaceStateUserData>()
                .and_then(|data| data.lock().ok().and_then(|data| data.view()));
            match view {
                Some(view) => {
                    TraversalAction::DoChildren(*location + view.offset.to_f64().to_physical(scale))
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
                let location = *location + view.offset.to_f64().to_physical(scale);
                match WaylandSurfaceRenderElement::from_surface(
                    renderer,
                    surface,
                    states,
                    location,
                    1.0,
                    Kind::Unspecified,
                ) {
                    Ok(Some(element)) => elements.push(element.into()),
                    Ok(None) => {}
                    Err(error) => eprintln!("surface import: {error}"),
                }
            }
        },
        |_, _, _| true,
    );
}

// The Smithay solid element owns an opaque-region Vec. Share it across frames
// instead of allocating one for every border on every client repaint.
#[derive(Clone, Debug)]
struct Border(std::sync::Arc<SolidColorRenderElement>);

impl Border {
    fn new(id: Id, geometry: smithay::utils::Rectangle<i32, smithay::utils::Physical>) -> Self {
        Self(std::sync::Arc::new(SolidColorRenderElement::new(
            id,
            geometry,
            smithay::backend::renderer::utils::CommitCounter::default(),
            [0.25, 0.25, 0.25, 1.0],
            Kind::Unspecified,
        )))
    }
}

impl Element for Border {
    fn id(&self) -> &Id {
        self.0.id()
    }
    fn current_commit(&self) -> smithay::backend::renderer::utils::CommitCounter {
        self.0.current_commit()
    }
    fn src(&self) -> smithay::utils::Rectangle<f64, smithay::utils::Buffer> {
        self.0.src()
    }
    fn geometry(
        &self,
        scale: smithay::utils::Scale<f64>,
    ) -> smithay::utils::Rectangle<i32, smithay::utils::Physical> {
        self.0.geometry(scale)
    }
    fn opaque_regions(
        &self,
        scale: smithay::utils::Scale<f64>,
    ) -> smithay::backend::renderer::utils::OpaqueRegions<i32, smithay::utils::Physical> {
        self.0.opaque_regions(scale)
    }
    fn alpha(&self) -> f32 {
        1.0
    }
    fn kind(&self) -> Kind {
        Kind::Unspecified
    }
}
impl<R: smithay::backend::renderer::Renderer> RenderElement<R> for Border {
    fn draw(
        &self,
        frame: &mut R::Frame<'_, '_>,
        src: smithay::utils::Rectangle<f64, smithay::utils::Buffer>,
        dst: smithay::utils::Rectangle<i32, smithay::utils::Physical>,
        damage: &[smithay::utils::Rectangle<i32, smithay::utils::Physical>],
        opaque: &[smithay::utils::Rectangle<i32, smithay::utils::Physical>],
        cache: Option<&smithay::utils::user_data::UserDataMap>,
    ) -> Result<(), R::Error> {
        <SolidColorRenderElement as RenderElement<R>>::draw(
            &self.0, frame, src, dst, damage, opaque, cache,
        )
    }
    fn underlying_storage(
        &self,
        _: &mut R,
    ) -> Option<smithay::backend::renderer::element::UnderlyingStorage<'_>> {
        None
    }
}
