//! Обычное окно программы (окно папки): рамку, заголовок и кнопки рисует
//! оконный менеджер, окно можно двигать и менять ему размер. Кнопка «закрыть»
//! приходит событием [`Event::Close`](crate::Event::Close) (ICCCM
//! `WM_DELETE_WINDOW`), окно не исчезает само.

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    AtomEnum, ConnectionExt as _, CreateGCAux, CreateWindowAux, EventMask, PropMode, Window,
    WindowClass,
};
use x11rb::wrapper::ConnectionExt as _;

use crate::{Display, Result, put_pixmap};

/// Фон окна до первого кадра (ARGB, непрозрачный): тёмный, как само окно.
const BACKGROUND: u32 = 0xFF1E_1E1E;

pub struct AppWindow {
    window: Window,
    gc: u32,
}

impl Display {
    /// Создаёт окно программы (пока скрытое) размером `w`×`h` точек.
    /// `class` — имя в `WM_CLASS`: по нему док и строка меню узнают программу.
    pub fn create_app_window(
        &self,
        class: &str,
        title: &str,
        (w, h): (u32, u32),
        min: (u32, u32),
    ) -> Result<AppWindow> {
        let window = self.conn.generate_id()?;
        let events = EventMask::KEY_PRESS
            | EventMask::BUTTON_PRESS
            | EventMask::BUTTON_RELEASE
            | EventMask::POINTER_MOTION
            | EventMask::LEAVE_WINDOW
            | EventMask::EXPOSURE
            | EventMask::STRUCTURE_NOTIFY;
        self.conn.create_window(
            32,
            window,
            self.root,
            0,
            0,
            self.px(w).max(1) as u16,
            self.px(h).max(1) as u16,
            0,
            WindowClass::INPUT_OUTPUT,
            self.visual,
            &CreateWindowAux::new()
                .background_pixel(BACKGROUND)
                .border_pixel(0)
                .colormap(self.colormap)
                .event_mask(events),
        )?;
        let a = &self.atoms;
        let wm_class = format!("{class}\0{}\0", mimshel_core::names::CLASS);
        self.conn.change_property8(
            PropMode::REPLACE,
            window,
            AtomEnum::WM_CLASS,
            AtomEnum::STRING,
            wm_class.as_bytes(),
        )?;
        self.conn.change_property32(
            PropMode::REPLACE,
            window,
            a.WM_PROTOCOLS,
            AtomEnum::ATOM,
            &[a.WM_DELETE_WINDOW],
        )?;
        self.conn.change_property32(
            PropMode::REPLACE,
            window,
            a._NET_WM_WINDOW_TYPE,
            AtomEnum::ATOM,
            &[a._NET_WM_WINDOW_TYPE_NORMAL],
        )?;
        self.conn.change_property32(
            PropMode::REPLACE,
            window,
            a._NET_WM_PID,
            AtomEnum::CARDINAL,
            &[std::process::id()],
        )?;
        // WM_HINTS: окну нужна клавиатура (флаг input = 1).
        self.conn.change_property32(
            PropMode::REPLACE,
            window,
            AtomEnum::WM_HINTS,
            AtomEnum::WM_HINTS,
            &[1, 1, 0, 0, 0, 0, 0, 0, 0],
        )?;
        // WM_NORMAL_HINTS: наименьший размер (флаг PMinSize = 16).
        let mut hints = [0u32; 18];
        hints[0] = 16;
        hints[5] = self.px(min.0);
        hints[6] = self.px(min.1);
        self.conn.change_property32(
            PropMode::REPLACE,
            window,
            AtomEnum::WM_NORMAL_HINTS,
            AtomEnum::WM_SIZE_HINTS,
            &hints,
        )?;
        // Принимает файлы, перетащенные из других окон.
        self.enable_drop(window)?;
        let gc = self.conn.generate_id()?;
        self.conn.create_gc(gc, window, &CreateGCAux::new())?;
        let w = AppWindow { window, gc };
        w.set_title(self, title)?;
        Ok(w)
    }
}

impl AppWindow {
    pub fn id(&self) -> u32 {
        self.window
    }

    pub fn show(&self, d: &Display) -> Result<()> {
        d.conn.map_window(self.window)?;
        d.conn.flush()?;
        Ok(())
    }

    /// Заголовок окна (в рамке и в списках окон).
    pub fn set_title(&self, d: &Display, title: &str) -> Result<()> {
        d.conn.change_property8(
            PropMode::REPLACE,
            self.window,
            d.atoms._NET_WM_NAME,
            d.atoms.UTF8_STRING,
            title.as_bytes(),
        )?;
        d.conn.change_property8(
            PropMode::REPLACE,
            self.window,
            AtomEnum::WM_NAME,
            d.atoms.UTF8_STRING,
            title.as_bytes(),
        )?;
        d.conn.flush()?;
        Ok(())
    }

    /// Размер окна сейчас — в точках.
    pub fn size(&self, d: &Display) -> Option<(u32, u32)> {
        let g = d.conn.get_geometry(self.window).ok()?.reply().ok()?;
        let k = d.scale();
        Some((
            (f32::from(g.width) / k).floor() as u32,
            (f32::from(g.height) / k).floor() as u32,
        ))
    }

    /// Где на экране левый верхний угол окна — в точках.
    pub fn origin(&self, d: &Display) -> Option<(i16, i16)> {
        let r = d
            .conn
            .translate_coordinates(self.window, d.root, 0, 0)
            .ok()?
            .reply()
            .ok()?;
        Some((d.pt(r.dst_x), d.pt(r.dst_y)))
    }

    /// Вывести картинку (в пикселях окна).
    pub fn present(&self, d: &Display, image: &impl crate::Image) -> Result<()> {
        put_pixmap(d, self.window, self.gc, image.image())
    }

    pub fn destroy(&self, d: &Display) -> Result<()> {
        d.conn.free_gc(self.gc)?;
        d.conn.destroy_window(self.window)?;
        d.conn.flush()?;
        Ok(())
    }
}
