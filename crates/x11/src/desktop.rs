//! Окно рабочего стола: тип `_NET_WM_WINDOW_TYPE_DESKTOP` (оконный менеджер
//! держит его под всеми окнами, на всех рабочих столах, без рамки) во весь
//! экран. Обои — картинка на стороне X-сервера, она же фон окна: сервер сам
//! перерисовывает стол, когда его открывают окна, а служба не держит у себя
//! картинку во весь экран.

use std::cell::Cell;

use tiny_skia::Pixmap;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    AtomEnum, ChangeWindowAttributesAux, ConfigureWindowAux, ConnectionExt as _, CreateGCAux,
    CreateWindowAux, EventMask, PropMode, StackMode, Window, WindowClass,
};
use x11rb::wrapper::ConnectionExt as _;

use crate::{Display, Result, put_pixmap};

/// Цвет стола, пока обои не готовы (ARGB, непрозрачный).
const PLAIN: u32 = 0xFF1E_1B3A;

pub struct DesktopWindow {
    window: Window,
    gc: u32,
    /// Картинка обоев на сервере (её освобождаем, когда ставим новую).
    wallpaper: Cell<Option<u32>>,
}

impl Display {
    /// Создаёт окно рабочего стола (пока скрытое).
    pub fn create_desktop(&self) -> Result<DesktopWindow> {
        let window = self.conn.generate_id()?;
        let events = EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE;
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
                .background_pixel(PLAIN)
                .border_pixel(0)
                .colormap(self.colormap)
                .event_mask(events),
        )?;
        let a = &self.atoms;
        self.conn.change_property8(
            PropMode::REPLACE,
            window,
            AtomEnum::WM_CLASS,
            AtomEnum::STRING,
            b"desktop\0Shell\0",
        )?;
        self.conn.change_property8(
            PropMode::REPLACE,
            window,
            AtomEnum::WM_NAME,
            AtomEnum::STRING,
            "Рабочий стол".as_bytes(),
        )?;
        self.conn.change_property32(
            PropMode::REPLACE,
            window,
            a._NET_WM_WINDOW_TYPE,
            AtomEnum::ATOM,
            &[a._NET_WM_WINDOW_TYPE_DESKTOP],
        )?;
        self.conn.change_property32(
            PropMode::REPLACE,
            window,
            a._NET_WM_STATE,
            AtomEnum::ATOM,
            &[
                a._NET_WM_STATE_STICKY,
                a._NET_WM_STATE_SKIP_TASKBAR,
                a._NET_WM_STATE_SKIP_PAGER,
            ],
        )?;
        self.conn.change_property32(
            PropMode::REPLACE,
            window,
            a._NET_WM_DESKTOP,
            AtomEnum::CARDINAL,
            &[u32::MAX],
        )?;
        let gc = self.conn.generate_id()?;
        self.conn.create_gc(gc, window, &CreateGCAux::new())?;
        self.conn.flush()?;
        Ok(DesktopWindow {
            window,
            gc,
            wallpaper: Cell::new(None),
        })
    }
}

impl DesktopWindow {
    pub fn id(&self) -> u32 {
        self.window
    }

    /// Во весь экран, под всеми окнами, на экране.
    pub fn show(&self, d: &Display) -> Result<()> {
        let (w, h) = d.screen_pixels();
        d.conn.configure_window(
            self.window,
            &ConfigureWindowAux::new()
                .x(0)
                .y(0)
                .width(u32::from(w))
                .height(u32::from(h))
                .stack_mode(StackMode::BELOW),
        )?;
        d.conn.map_window(self.window)?;
        d.conn.flush()?;
        Ok(())
    }

    /// Поставить обои: картинка в пикселях экрана (левый верхний угол — к углу
    /// экрана).
    pub fn set_wallpaper(&self, d: &Display, image: &Pixmap) -> Result<()> {
        let (w, h) = (image.width(), image.height());
        let (Ok(pw), Ok(ph)) = (u16::try_from(w), u16::try_from(h)) else {
            return Ok(());
        };
        if pw == 0 || ph == 0 {
            return Ok(());
        }
        let pixmap = d.conn.generate_id()?;
        d.conn.create_pixmap(32, pixmap, self.window, pw, ph)?;
        put_pixmap(d, pixmap, self.gc, image)?;
        d.conn.change_window_attributes(
            self.window,
            &ChangeWindowAttributesAux::new().background_pixmap(pixmap),
        )?;
        d.conn.clear_area(false, self.window, 0, 0, 0, 0)?;
        if let Some(old) = self.wallpaper.replace(Some(pixmap)) {
            d.conn.free_pixmap(old)?;
        }
        d.conn.flush()?;
        Ok(())
    }
}
