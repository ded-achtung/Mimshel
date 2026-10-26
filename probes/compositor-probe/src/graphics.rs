use crate::timing::FrameTimings;
use std::time::Instant;

use crate::{
    math::{self, ICONS},
    state::State,
};
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            Frame, ImportMem, Renderer,
            element::{
                AsRenderElements, Element, RenderElement, surface::WaylandSurfaceRenderElement,
            },
            gles::{
                GlesPixelProgram, GlesRenderer, GlesTexture, Uniform, UniformName, UniformType,
            },
        },
        winit::WinitGraphicsBackend,
    },
    utils::{Rectangle, Transform},
};

pub struct Graphics {
    shader: Option<GlesPixelProgram>,
    icons: Vec<GlesTexture>,
    scales: [f64; ICONS],
}
impl Graphics {
    pub fn new(
        renderer: &mut GlesRenderer,
        wallpaper: bool,
        dock: bool,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let shader = if wallpaper {
            Some(renderer.compile_custom_pixel_shader(
                include_str!("wallpaper.frag"),
                &[UniformName::new("time", UniformType::_1f)],
            )?)
        } else {
            None
        };
        let mut icons = Vec::new();
        if dock {
            // Generate premultiplied RGBA rounded squares once, at maximum display size.
            for color in [
                [230, 80, 100],
                [245, 160, 65],
                [235, 205, 70],
                [70, 190, 125],
                [70, 170, 235],
                [100, 110, 240],
                [180, 95, 230],
                [220, 110, 170],
            ] {
                let mut pixels = vec![0u8; 112 * 112 * 4];
                for y in 0..112 {
                    for x in 0..112 {
                        let dx = (x as f64 - 55.5).abs() - 35.5;
                        let dy = (y as f64 - 55.5).abs() - 35.5;
                        let distance = dx.max(0.0).hypot(dy.max(0.0)) + dx.max(dy).min(0.0) - 18.0;
                        let alpha = (0.5 - distance).clamp(0.0, 1.0);
                        let offset = (y * 112 + x) * 4;
                        for c in 0..3 {
                            pixels[offset + c] = (f64::from(color[c]) * alpha) as u8;
                        }
                        pixels[offset + 3] = (255.0 * alpha) as u8;
                    }
                }
                icons.push(renderer.import_memory(
                    &pixels,
                    Fourcc::Abgr8888,
                    (112, 112).into(),
                    false,
                )?);
            }
        }
        Ok(Self {
            shader,
            icons,
            scales: [1.0; ICONS],
        })
    }
    pub fn render(
        &mut self,
        backend: &mut WinitGraphicsBackend<GlesRenderer>,
        state: &State,
        time: f64,
        dt: f64,
        bench: bool,
        frame_start: Instant,
    ) -> Result<FrameTimings, Box<dyn std::error::Error>> {
        let size = backend.window_size();
        let damage = Rectangle::from_size(size);
        let width = f64::from(size.w);
        let height = f64::from(size.h);
        let centers = math::centers(width);
        let px = if bench {
            width / 2.0 + (time * 1.2).sin() * 300.0
        } else {
            state.pointer.x
        };
        let active = bench || (state.pointer_inside && state.pointer.y > height - 125.0);
        for (i, scale) in self.scales.iter_mut().enumerate() {
            *scale = math::approach(*scale, math::target_scale(px - centers[i], active), dt);
        }
        let (renderer, mut fb) = backend.bind()?;
        let mut windows: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = Vec::new();
        // Space is stored bottom-to-top; each surface tree is front-to-back.
        for w in state.space.elements() {
            let loc = state.space.element_location(w).unwrap() - w.geometry().loc;
            let mut elements = w.render_elements(
                renderer,
                loc.to_physical_precise_round(1.0),
                1.0.into(),
                1.0,
            );
            elements.reverse();
            windows.extend(elements);
        }
        let mut frame = renderer.render(&mut fb, size, Transform::Flipped180)?;
        frame.clear([0.012, 0.022, 0.075, 1.0].into(), &[damage])?;
        if let Some(shader) = &self.shader {
            frame.render_pixel_shader_to(
                shader,
                Rectangle::from_size((size.w, size.h).into()).to_f64(),
                damage,
                (size.w, size.h).into(),
                None,
                1.0,
                &[Uniform::new("time", time as f32)],
            )?;
        }
        for element in &windows {
            let dst = element.geometry(1.0.into());
            element.draw(
                &mut frame,
                element.src(),
                dst,
                &[Rectangle::from_size(dst.size)],
                &[],
            )?;
        }
        if !self.icons.is_empty() {
            let layout = math::layout(width, &self.scales);
            let start = layout[0].0.round() as i32 - 12;
            let end = (layout[7].0 + layout[7].1).round() as i32 + 12;
            frame.draw_solid(
                Rectangle::new((start, size.h - 84).into(), (end - start, 76).into()),
                &[Rectangle::from_size((end - start, 76).into())],
                [0.035, 0.04, 0.07, 0.85].into(),
            )?;
            for (texture, (x, side)) in self.icons.iter().zip(layout) {
                let side = side.round() as i32;
                let dst = Rectangle::new(
                    (x.round() as i32, size.h - 20 - side).into(),
                    (side, side).into(),
                );
                frame.render_texture_from_to(
                    texture,
                    Rectangle::from_size((112, 112).into()).to_f64(),
                    dst,
                    &[Rectangle::from_size(dst.size)],
                    &[],
                    Transform::Normal,
                    1.0,
                    None,
                    &[],
                )?;
            }
        }
        // Include completion of GPU commands, not just command queue construction.
        frame.finish()?.wait()?;
        let render = frame_start.elapsed();
        drop(fb);
        let present_start = Instant::now();
        backend.submit(Some(&[damage]))?;
        let present = present_start.elapsed();
        Ok(FrameTimings { render, present })
    }
}
