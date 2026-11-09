//! Pure title bar rasterization and frame hit-testing. Dimensions are logical points.

use mimshel_text::{Fonts, Style};
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Stroke, Transform};

/// Logical height of the compositor title bar.
pub const HEIGHT: i32 = crate::model::TITLE_HEIGHT;

/// Decoration control under the pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    /// Close button.
    Close,
    /// Minimize button.
    Minimize,
    /// Maximize button.
    Maximize,
    /// Draggable title bar.
    Title,
    /// Resize zone: horizontal and vertical direction (-1, 0, 1).
    Edge(i8, i8),
}

/// Test a point relative to the outer top-left; height is the client height.
pub fn hit_test(x: f64, y: f64, width: i32, height: i32) -> Option<Hit> {
    let (w, h) = (f64::from(width), f64::from(height + HEIGHT));
    if x < -6.0 || y < -6.0 || x >= w + 6.0 || y >= h + 6.0 {
        return None;
    }
    let dx = if x < 1.0 {
        -1
    } else if x >= w - 1.0 {
        1
    } else {
        0
    };
    let dy = if y < 1.0 {
        -1
    } else if y >= h - 1.0 {
        1
    } else {
        0
    };
    if dx != 0 || dy != 0 {
        return Some(Hit::Edge(dx, dy));
    }
    if y >= f64::from(HEIGHT) {
        return None;
    }
    for (i, hit) in [Hit::Close, Hit::Minimize, Hit::Maximize]
        .into_iter()
        .enumerate()
    {
        let cx = 18.0 + i as f64 * 20.0;
        if (x - cx).powi(2) + (y - 15.0).powi(2) <= 36.0 {
            return Some(hit);
        }
    }
    Some(Hit::Title)
}

/// Rasterize a title bar without protocol or GPU state. Invalid sizes return `None`.
/// Font layout happens only when a caller invalidates its cached pixels.
pub fn render(
    title: &str,
    width: i32,
    focused: bool,
    hover: bool,
    scale: f32,
    fonts: &mut Fonts,
) -> Option<Pixmap> {
    if width <= 0 || !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    // Whatever the caller passes, the picture has a bound (`Model` keeps
    // windows within it too).
    let width = width.min(crate::model::SIDE_MAX);
    let scale = scale.min(8.0);
    let mut pm = Pixmap::new(
        (width as f32 * scale).round() as u32,
        (HEIGHT as f32 * scale).round() as u32,
    )?;
    let t = Transform::from_scale(scale, scale);
    let mut path = PathBuilder::new();
    let w = width as f32;
    let r = 10.0_f32.min(w / 2.0);
    path.move_to(0.0, 30.0);
    path.line_to(0.0, r);
    path.quad_to(0.0, 0.0, r, 0.0);
    path.line_to(w - r, 0.0);
    path.quad_to(w, 0.0, w, r);
    path.line_to(w, 30.0);
    path.close();
    let path = path.finish()?;
    let mut paint = Paint::default();
    let bg = if focused { 43 } else { 34 };
    paint.set_color_rgba8(bg, bg, bg, 255);
    pm.fill_path(&path, &paint, FillRule::Winding, t, None);
    paint.set_color_rgba8(65, 65, 65, 255);
    pm.stroke_path(
        &path,
        &paint,
        &Stroke {
            width: 1.0 / scale,
            ..Stroke::default()
        },
        t,
        None,
    );
    for (i, color) in [[235, 93, 86], [238, 186, 63], [83, 187, 78]]
        .into_iter()
        .enumerate()
    {
        let cx = 18.0 + i as f32 * 20.0;
        let circle = PathBuilder::from_circle(cx, 15.0, 6.0)?;
        let [r, g, b] = if focused { color } else { [83, 83, 83] };
        paint.set_color_rgba8(r, g, b, 255);
        pm.fill_path(&circle, &paint, FillRule::Winding, t, None);
        if hover {
            let mut symbol = PathBuilder::new();
            if i == 0 {
                symbol.move_to(cx - 2.5, 12.5);
                symbol.line_to(cx + 2.5, 17.5);
                symbol.move_to(cx + 2.5, 12.5);
                symbol.line_to(cx - 2.5, 17.5);
            } else {
                symbol.move_to(cx - 3.0, 15.0);
                symbol.line_to(cx + 3.0, 15.0);
                if i == 2 {
                    symbol.move_to(cx, 12.0);
                    symbol.line_to(cx, 18.0);
                }
            }
            paint.set_color_rgba8(45, 45, 45, 255);
            pm.stroke_path(
                &symbol.finish()?,
                &paint,
                &Stroke {
                    width: 1.2,
                    ..Stroke::default()
                },
                t,
                None,
            );
        }
    }
    let available = (w - 160.0).max(0.0) * scale;
    if available > 0.0 {
        let style = Style {
            size: (10.0 * 96.0 / 72.0) * scale,
            color: if focused {
                [225, 225, 225, 255]
            } else {
                [140, 140, 140, 255]
            },
            weight: 600,
        };
        let tw = fonts.measure(title, &style).min(available);
        fonts.draw_pixels(
            &mut pm,
            title,
            &style,
            (w * scale - tw) / 2.0,
            (30.0 * scale - style.line_height()) / 2.0,
            Some(available),
        );
    }
    Some(pm)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn controls_and_edges() {
        assert_eq!(hit_test(18.0, 15.0, 400, 200), Some(Hit::Close));
        assert_eq!(hit_test(38.0, 15.0, 400, 200), Some(Hit::Minimize));
        assert_eq!(hit_test(58.0, 15.0, 400, 200), Some(Hit::Maximize));
        assert_eq!(hit_test(200.0, 15.0, 400, 200), Some(Hit::Title));
        for (x, y, dx, dy) in [
            (-3., -3., -1, -1),
            (200., -3., 0, -1),
            (403., -3., 1, -1),
            (-3., 100., -1, 0),
            (403., 100., 1, 0),
            (-3., 233., -1, 1),
            (200., 233., 0, 1),
            (403., 233., 1, 1),
        ] {
            assert_eq!(hit_test(x, y, 400, 200), Some(Hit::Edge(dx, dy)));
        }
        assert_eq!(hit_test(200., 100., 400, 200), None);
        assert_eq!(hit_test(-7., 100., 400, 200), None);
    }
    #[test]
    fn snapshot() {
        let mut fonts = Fonts::new("Inter");
        let pm = render(
            "Mimshel — Window decorations",
            640,
            true,
            true,
            2.0,
            &mut fonts,
        )
        .unwrap();
        assert_eq!((pm.width(), pm.height()), (1280, 60));
        assert_eq!(pm.pixel(0, 0).unwrap().alpha(), 0);
        let path = std::env::var_os("MIMSHEL_DECORATION_SNAPSHOT");
        if let Some(path) = path {
            pm.save_png(path).unwrap();
        }
        let dim = render(
            "Mimshel — Window decorations",
            640,
            false,
            false,
            2.0,
            &mut fonts,
        )
        .unwrap();
        assert_ne!(pm.data(), dim.data());
    }
}
