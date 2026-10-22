//! Окно-док: тип `_NET_WM_WINDOW_TYPE_DOCK` (оконный менеджер держит его
//! поверх окон, без рамки, на всех рабочих столах, не даёт ему фокус), с
//! прозрачностью и областью кликов — только там, где док нарисован.

use tiny_skia::Pixmap;
use x11rb::connection::Connection;
use x11rb::protocol::shape::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{
    AtomEnum, ClipOrdering, ConfigureWindowAux, ConnectionExt as _, CreateGCAux, CreateWindowAux,
    EventMask, PropMode, Rectangle, Window, WindowClass,
};
use x11rb::wrapper::ConnectionExt as _;

use crate::{Display, Result, put_pixmap};

pub struct DockWindow {
    window: Window,
    gc: u32,
}

impl Display {
    /// Создаёт окно-док (пока скрытое).
    pub fn create_dock(&self, name: &str) -> Result<DockWindow> {
        let window = self.conn.generate_id()?;
        let events = EventMask::BUTTON_PRESS
            | EventMask::POINTER_MOTION
            | EventMask::ENTER_WINDOW
            | EventMask::LEAVE_WINDOW
            | EventMask::EXPOSURE;
        self.conn.create_window(
            32,
            window,
            self.root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_OUTPUT,
            self.visual,
            &CreateWindowAux::new()
                .background_pixel(0)
                .border_pixel(0)
                .colormap(self.colormap)
                .event_mask(events),
        )?;
        let a = &self.atoms;
        let class = format!("{name}\0Shell\0");
        self.conn.change_property8(
            PropMode::REPLACE,
            window,
            AtomEnum::WM_CLASS,
            AtomEnum::STRING,
            class.as_bytes(),
        )?;
        self.conn.change_property32(
            PropMode::REPLACE,
            window,
            a._NET_WM_WINDOW_TYPE,
            AtomEnum::ATOM,
            &[a._NET_WM_WINDOW_TYPE_DOCK],
        )?;
        self.conn.change_property32(
            PropMode::REPLACE,
            window,
            a._NET_WM_STATE,
            AtomEnum::ATOM,
            &[
                a._NET_WM_STATE_ABOVE,
                a._NET_WM_STATE_STICKY,
                a._NET_WM_STATE_SKIP_TASKBAR,
            ],
        )?;
        // На всех рабочих столах.
        self.conn.change_property32(
            PropMode::REPLACE,
            window,
            a._NET_WM_DESKTOP,
            AtomEnum::CARDINAL,
            &[u32::MAX],
        )?;
        // WM_HINTS: флаг «input» и input = 0 — клик по доку не отнимает фокус у окна.
        self.conn.change_property32(
            PropMode::REPLACE,
            window,
            AtomEnum::WM_HINTS,
            AtomEnum::WM_HINTS,
            &[1, 0, 0, 0, 0, 0, 0, 0, 0],
        )?;
        let gc = self.conn.generate_id()?;
        self.conn.create_gc(gc, window, &CreateGCAux::new())?;
        self.conn.flush()?;
        Ok(DockWindow { window, gc })
    }
}

impl DockWindow {
    pub fn id(&self) -> u32 {
        self.window
    }

    pub fn show(&self, d: &Display) -> Result<()> {
        d.conn.map_window(self.window)?;
        d.conn.flush()?;
        Ok(())
    }

    pub fn set_geometry(&self, d: &Display, x: i32, y: i32, w: u32, h: u32) -> Result<()> {
        d.conn.configure_window(
            self.window,
            &ConfigureWindowAux::new().x(x).y(y).width(w).height(h),
        )?;
        d.conn.flush()?;
        Ok(())
    }

    pub fn present(&self, d: &Display, pixmap: &Pixmap) -> Result<()> {
        put_pixmap(d, self.window, self.gc, pixmap)
    }

    /// Где окно принимает клики и указатель — прямоугольники `(x, y, w, h)`
    /// в координатах окна. В остальных местах клик проходит к окнам под доком.
    pub fn set_input(&self, d: &Display, rects: &[(i32, i32, u32, u32)]) -> Result<()> {
        let rects: Vec<Rectangle> = rects
            .iter()
            .map(|&(x, y, w, h)| Rectangle {
                x: x as i16,
                y: y as i16,
                width: w as u16,
                height: h as u16,
            })
            .collect();
        d.conn.shape_rectangles(
            shape::SO::SET,
            shape::SK::INPUT,
            ClipOrdering::UNSORTED,
            self.window,
            0,
            0,
            &rects,
        )?;
        d.conn.flush()?;
        Ok(())
    }

    /// Резерв места внизу экрана: развёрнутые окна заканчиваются над доком
    /// (`height` — сколько пикселей снизу, `x0..x1` — где по ширине). 0 — без резерва.
    pub fn set_strut(&self, d: &Display, height: u32, x0: u32, x1: u32) -> Result<()> {
        let a = &d.atoms;
        let partial = [0, 0, 0, height, 0, 0, 0, 0, 0, 0, x0, x1];
        d.conn.change_property32(
            PropMode::REPLACE,
            self.window,
            a._NET_WM_STRUT_PARTIAL,
            AtomEnum::CARDINAL,
            &partial,
        )?;
        d.conn.change_property32(
            PropMode::REPLACE,
            self.window,
            a._NET_WM_STRUT,
            AtomEnum::CARDINAL,
            &[0, 0, 0, height],
        )?;
        d.conn.flush()?;
        Ok(())
    }
}
