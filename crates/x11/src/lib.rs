//! Связь с X-сервером: всплывающее окно с прозрачностью, вывод в него
//! картинки, захват клавиатуры и мыши, перевод нажатий в буквы с учётом
//! раскладки (us/ru) и регистра. Для дока — окна программ ([`windows`]) и
//! окно-док ([`dock`]).

mod clipboard;
mod desktop;
mod dnd;
mod dock;
mod keyboard;
mod selection;
mod shm;
mod windows;

pub use clipboard::{clipboard_files, copy_files};
pub use desktop::DesktopWindow;
pub use dock::DockWindow;
pub use selection::SelectionRenderer;

use std::sync::Mutex;
use std::time::Duration;

pub use shell_core::input::{Key, KeyPress};
use shell_render::Image;
use tiny_skia::Pixmap;
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::Event as XEvent;
use x11rb::protocol::xkb::ConnectionExt as _;
use x11rb::protocol::xproto::{
    AtomEnum, ChangeWindowAttributesAux, ColormapAlloc, ConfigureWindowAux, ConnectionExt as _,
    CreateGCAux, CreateWindowAux, EventMask, GrabMode, GrabStatus, ImageFormat, InputFocus,
    PropMode, StackMode, VisualClass, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{CURRENT_TIME, NONE};
use xkeysym::{Keysym, key};

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Result<T> = std::result::Result<T, Error>;

/// Отдельное тихое соединение с X-сервером, чтобы узнать, когда он закроется
/// (выход из сеанса). Событий не просит — только ждёт обрыва.
pub struct ServerWatch(RustConnection);

impl ServerWatch {
    pub fn connect() -> Result<ServerWatch> {
        Ok(ServerWatch(RustConnection::connect(None)?.0))
    }

    /// Вернётся, когда соединение оборвётся.
    pub fn wait_closed(self) {
        while self.0.wait_for_event().is_ok() {}
    }
}

/// DPI из ресурсов X (`Xft.dpi`; его ставит xfsettingsd по настройке Xfce
/// «Внешний вид → Шрифты → DPI»). `None` — не задан или нет связи с X.
pub fn xft_dpi() -> Option<f32> {
    let (conn, screen) = RustConnection::connect(None).ok()?;
    let root = conn.setup().roots.get(screen)?.root;
    let reply = conn
        .get_property(
            false,
            root,
            AtomEnum::RESOURCE_MANAGER,
            AtomEnum::STRING,
            0,
            1 << 20,
        )
        .ok()?
        .reply()
        .ok()?;
    parse_dpi(&String::from_utf8_lossy(&reply.value))
}

fn parse_dpi(resources: &str) -> Option<f32> {
    resources.lines().find_map(|l| {
        let (name, value) = l.split_once(':')?;
        (name.trim() == "Xft.dpi")
            .then(|| value.trim().parse::<f32>().ok())
            .flatten()
            .filter(|d| *d > 0.0)
    })
}

/// Масштаб по DPI: 96 — 1, 144 — 1,5, 192 — 2 (с шагом 0,25).
pub fn scale_for_dpi(dpi: f32) -> f32 {
    ((dpi / 96.0) * 4.0).round().clamp(4.0, 12.0) / 4.0
}

x11rb::atom_manager! {
    /// Имена свойств EWMH/ICCCM, которые нужны оболочке.
    pub(crate) Atoms: AtomsCookie {
        UTF8_STRING,
        WM_STATE,
        WM_CHANGE_STATE,
        _NET_CLIENT_LIST,
        _NET_CLIENT_LIST_STACKING,
        _NET_ACTIVE_WINDOW,
        _NET_CLOSE_WINDOW,
        _NET_WM_PID,
        _NET_WM_NAME,
        _NET_WM_ICON,
        _NET_WM_DESKTOP,
        _NET_WM_STATE,
        _NET_WM_STATE_SKIP_TASKBAR,
        _NET_WM_STATE_HIDDEN,
        _NET_WM_STATE_MAXIMIZED_VERT,
        _NET_WM_STATE_MAXIMIZED_HORZ,
        _NET_WM_STATE_ABOVE,
        _NET_WM_STATE_STICKY,
        _NET_WM_WINDOW_TYPE,
        _NET_WM_WINDOW_TYPE_NORMAL,
        _NET_WM_WINDOW_TYPE_DIALOG,
        _NET_WM_WINDOW_TYPE_DOCK,
        _NET_WM_WINDOW_TYPE_DESKTOP,
        XdndAware,
        XdndEnter,
        XdndPosition,
        XdndStatus,
        XdndLeave,
        XdndDrop,
        XdndFinished,
        XdndSelection,
        XdndActionCopy,
        XdndActionMove,
        TEXT_URI_LIST: b"text/uri-list",
        SHELL_XDND,
        _NET_WM_STATE_SKIP_PAGER,
        _NET_WM_STRUT,
        _NET_WM_STRUT_PARTIAL,
        _NET_FRAME_EXTENTS,
        _GTK_FRAME_EXTENTS,
    }
}

/// Какое свойство окна изменилось.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Property {
    /// Список окон программ (у корневого окна).
    ClientList,
    /// Активное окно (у корневого окна).
    ActiveWindow,
    /// Свёрнуто/развёрнуто и т. п.
    State,
    Title,
    Icon,
    Class,
    Other,
}

/// События, которые нужны оболочке. Координаты — относительно окна `window`;
/// при захвате мыши клик мимо окна приходит с координатами за его краями.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Key(KeyPress),
    Button {
        window: u32,
        x: i16,
        y: i16,
        button: u8,
    },
    /// Кнопку мыши отпустили.
    Release {
        window: u32,
        x: i16,
        y: i16,
        button: u8,
    },
    Motion {
        window: u32,
        x: i16,
        y: i16,
    },
    /// Указатель вошёл в окно (в точку `x`, `y`) / вышел из него.
    Enter {
        window: u32,
        x: i16,
        y: i16,
    },
    Leave {
        window: u32,
    },
    /// Окно нужно перерисовать.
    Expose {
        window: u32,
    },
    Property {
        window: u32,
        what: Property,
    },
    /// Окно сдвинулось или поменяло размер (корневое — сменился размер экрана).
    Configure {
        window: u32,
    },
    /// Окно появилось на экране (приходит после [`Display::watch_stacking`]).
    Mapped {
        window: u32,
    },
    /// Над окном несут файлы из другой программы (в точке `x`, `y`).
    DropHover {
        window: u32,
        x: i16,
        y: i16,
    },
    DropLeave {
        window: u32,
    },
    /// Файлы бросили на окно; действие определено протоколом XDND.
    /// Пути читаются через [`Display::take_drop`], затем [`Display::finish_drop`].
    Dropped {
        window: u32,
        x: i16,
        y: i16,
        moving: bool,
        ticket: u64,
    },
    /// Включилась раскладка с этим номером (с 0).
    KeyboardGroup(u8),
    /// Поменялся набор раскладок (например, setxkbmap).
    KeyboardLayouts,
    Other,
}

pub struct Display {
    pub(crate) conn: RustConnection,
    pub(crate) root: Window,
    pub(crate) atoms: Atoms,
    screen_size: (u16, u16),
    /// Вид пикселей с прозрачностью (32 бита) и палитра для него.
    pub(crate) visual: u32,
    pub(crate) colormap: u32,
    keymap: Mutex<Keymap>,
    /// Общая память для больших картинок (см. [`shm`]).
    shm: shm::Shm,
    /// Перетаскивание из других программ (XDND).
    drop: dnd::DropCell,
    /// Масштаб (`shell_render::scale` при подключении): размеры окон и
    /// координаты мыши снаружи — в точках, серверу — в пикселях.
    scale: f32,
}

impl Display {
    /// Подключается к X-серверу из переменной DISPLAY.
    pub fn connect() -> Result<Display> {
        let (conn, screen_num) = RustConnection::connect(None)?;
        // Просим XKB: тогда в событиях клавиш есть номер раскладки (us/ru).
        conn.xkb_use_extension(1, 0)?.reply()?;
        conn.prefetch_maximum_request_bytes();

        let screen = &conn.setup().roots[screen_num];
        let root = screen.root;
        let screen_size = (screen.width_in_pixels, screen.height_in_pixels);
        let visual = screen
            .allowed_depths
            .iter()
            .filter(|d| d.depth == 32)
            .flat_map(|d| &d.visuals)
            .find(|v| v.class == VisualClass::TRUE_COLOR)
            .map(|v| v.visual_id)
            .ok_or("X-сервер не умеет окна с прозрачностью (нет 32-битного вида)")?;
        let colormap = conn.generate_id()?;
        conn.create_colormap(ColormapAlloc::NONE, colormap, root, visual)?;
        let keymap = Mutex::new(Keymap::load(&conn)?);
        let atoms = Atoms::new(&conn)?.reply()?;
        let shm = shm::Shm::new(&conn);

        Ok(Display {
            conn,
            root,
            atoms,
            screen_size,
            visual,
            colormap,
            keymap,
            shm,
            drop: dnd::DropCell::default(),
            scale: shell_render::scale(),
        })
    }

    /// Масштаб: сколько пикселей экрана в одной точке.
    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// Размер экрана в точках (с округлением вверх — окна во весь экран его
    /// закрывают целиком).
    pub fn screen_size(&self) -> (u16, u16) {
        let (w, h) = self.screen_pixels();
        let pt = |v: u16| (f32::from(v) / self.scale).ceil() as u16;
        (pt(w), pt(h))
    }

    /// Размер экрана в пикселях. Спрашиваем каждый раз: в ВМ он меняется вместе
    /// с окном VirtualBox. Если сервер не ответил — размер при подключении.
    pub fn screen_pixels(&self) -> (u16, u16) {
        self.conn
            .get_geometry(self.root)
            .ok()
            .and_then(|c| c.reply().ok())
            .map_or(self.screen_size, |g| (g.width, g.height))
    }

    /// Длина в точках → в пикселях (как у холста `shell_render::Canvas`).
    pub fn px(&self, v: u32) -> u32 {
        shell_render::to_px(v, self.scale)
    }

    /// Координата в пикселях → в точках.
    pub(crate) fn pt(&self, v: i16) -> i16 {
        (f32::from(v) / self.scale).floor() as i16
    }

    /// Прямоугольник окна в точках → в пикселях: угол округляется, размер —
    /// как у холста той же ширины (картинка ляжет ровно). Край, вышедший за
    /// экран на долю точки (экран в точках округлён вверх), прижат к экрану.
    pub(crate) fn rect_px(&self, x: i32, y: i32, w: u32, h: u32) -> (i32, i32, u32, u32) {
        if self.scale == 1.0 {
            return (x, y, w, h);
        }
        let at = |v: i32| (v as f32 * self.scale).round() as i32;
        let (x, y, mut w, mut h) = (at(x), at(y), self.px(w), self.px(h));
        let (sw, sh) = self.screen_pixels();
        let snap = |pos: i32, len: &mut u32, screen: u16| {
            let over = pos + *len as i32 - i32::from(screen);
            if over > 0 && over <= self.scale.ceil() as i32 + 1 {
                *len -= over as u32;
            }
        };
        snap(x, &mut w, sw);
        snap(y, &mut h, sh);
        (x, y, w.max(1), h.max(1))
    }

    /// Создаёт скрытое всплывающее окно (без рамки, поверх остальных).
    pub fn create_popup(&self, name: &str) -> Result<Popup> {
        let window = self.conn.generate_id()?;
        let events = EventMask::KEY_PRESS
            | EventMask::BUTTON_PRESS
            | EventMask::BUTTON_RELEASE
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
                .override_redirect(1)
                .event_mask(events),
        )?;
        let class = format!("{name}\0Shell\0");
        self.conn.change_property8(
            PropMode::REPLACE,
            window,
            AtomEnum::WM_CLASS,
            AtomEnum::STRING,
            class.as_bytes(),
        )?;
        let gc = self.conn.generate_id()?;
        self.conn.create_gc(gc, window, &CreateGCAux::new())?;
        self.conn.flush()?;
        Ok(Popup { window, gc })
    }

    /// Снимок всего экрана (как он виден сейчас, с окнами).
    pub fn capture_screen(&self) -> Result<Pixmap> {
        let (w, h) = self.screen_size();
        let mut data = match self.shm_get(self.root, w, h) {
            Some(Some(data)) => data,
            Some(None) => return Err("снимок экрана не получился".into()),
            None => {
                let reply = self
                    .conn
                    .get_image(ImageFormat::Z_PIXMAP, self.root, 0, 0, w, h, !0)?
                    .reply()?;
                if reply.data.len() != usize::from(w) * usize::from(h) * 4 {
                    return Err(format!(
                        "неожиданный формат снимка экрана (глубина {})",
                        reply.depth
                    )
                    .into());
                }
                reply.data
            }
        };
        // X отдаёт BGRX, картинке нужен RGBA без прозрачности.
        for px in data.as_chunks_mut::<4>().0 {
            px.swap(0, 2);
            px[3] = 255;
        }
        let size = tiny_skia::IntSize::from_wh(u32::from(w), u32::from(h)).ok_or("пустой экран")?;
        Pixmap::from_vec(data, size).ok_or_else(|| "не удалось собрать снимок экрана".into())
    }

    /// Дожидается, пока сервер выполнит все отправленные запросы.
    pub fn sync(&self) -> Result<()> {
        self.conn.get_input_focus()?.reply()?;
        Ok(())
    }

    /// Вернуть фокус активному окну (после экрана блокировки, который уводил
    /// фокус в корневое окно). Нет активного окна — фокус корню, но он
    /// следует за указателем.
    pub fn restore_focus(&self) -> Result<()> {
        let reply = self
            .conn
            .get_property(
                false,
                self.root,
                self.atoms._NET_ACTIVE_WINDOW,
                AtomEnum::WINDOW,
                0,
                1,
            )?
            .reply()?;
        let active = reply
            .value32()
            .and_then(|mut v| v.next())
            .filter(|&w| w != 0 && w != u32::MAX);
        self.conn.set_input_focus(
            InputFocus::POINTER_ROOT,
            active.unwrap_or(self.root),
            CURRENT_TIME,
        )?;
        self.conn.flush()?;
        Ok(())
    }

    /// Перечитывает таблицу клавиш. Смену раскладок (setxkbmap, настройки Xfce)
    /// сервер сообщает событиями XKB, а не `MappingNotify`, поэтому проще
    /// перечитывать при каждом открытии окна — это один короткий запрос.
    pub fn refresh_keymap(&self) -> Result<()> {
        let k = Keymap::load(&self.conn)?;
        *self.keymap.lock().unwrap_or_else(|p| p.into_inner()) = k;
        Ok(())
    }

    /// Ждёт следующее событие.
    pub fn next_event(&self) -> Result<Event> {
        let ev = self.conn.wait_for_event()?;
        Ok(self.translate(ev))
    }

    fn translate(&self, ev: XEvent) -> Event {
        match ev {
            XEvent::KeyPress(e) => {
                let state = u16::from(e.state);
                let keymap = self.keymap.lock().unwrap_or_else(|p| p.into_inner());
                let key = keymap.key(e.detail, state);
                if std::env::var_os("SHELL_DEBUG_KEYS").is_some() {
                    let (sym, _) = keymap.keysym(e.detail, state);
                    eprintln!(
                        "клавиша: код {} состояние {state:#06x} раскладка {} символ {sym:#x} → {key:?}",
                        e.detail,
                        (state >> 13) & 0x3
                    );
                }
                Event::Key(KeyPress {
                    key,
                    shift: state & 0x1 != 0,
                    ctrl: state & 0x4 != 0,
                    alt: state & 0x8 != 0,
                    logo: state & 0x40 != 0,
                })
            }
            XEvent::ButtonPress(e) => Event::Button {
                window: e.event,
                x: self.pt(e.event_x),
                y: self.pt(e.event_y),
                button: e.detail,
            },
            XEvent::ButtonRelease(e) => Event::Release {
                window: e.event,
                x: self.pt(e.event_x),
                y: self.pt(e.event_y),
                button: e.detail,
            },
            XEvent::MotionNotify(e) => Event::Motion {
                window: e.event,
                x: self.pt(e.event_x),
                y: self.pt(e.event_y),
            },
            XEvent::EnterNotify(e) => Event::Enter {
                window: e.event,
                x: self.pt(e.event_x),
                y: self.pt(e.event_y),
            },
            XEvent::LeaveNotify(e) => Event::Leave { window: e.event },
            XEvent::Expose(e) if e.count == 0 => Event::Expose { window: e.window },
            XEvent::PropertyNotify(e) => {
                let a = &self.atoms;
                let what = match e.atom {
                    x if x == a._NET_CLIENT_LIST || x == a._NET_CLIENT_LIST_STACKING => {
                        Property::ClientList
                    }
                    x if x == a._NET_ACTIVE_WINDOW => Property::ActiveWindow,
                    x if x == a._NET_WM_STATE || x == a.WM_STATE => Property::State,
                    x if x == a._NET_WM_NAME || x == u32::from(AtomEnum::WM_NAME) => {
                        Property::Title
                    }
                    x if x == a._NET_WM_ICON => Property::Icon,
                    x if x == u32::from(AtomEnum::WM_CLASS) => Property::Class,
                    _ => Property::Other,
                };
                Event::Property {
                    window: e.window,
                    what,
                }
            }
            XEvent::ConfigureNotify(e) => Event::Configure { window: e.window },
            XEvent::MapNotify(e) => Event::Mapped { window: e.window },
            XEvent::ClientMessage(e) => self
                .dnd_message(e.window, e.type_, e.data.as_data32())
                .unwrap_or(Event::Other),
            XEvent::SelectionNotify(e)
                if e.property == self.atoms.SHELL_XDND
                    || e.selection == self.atoms.XdndSelection =>
            {
                self.dnd_selection(e.requestor, e.property)
            }
            XEvent::XkbStateNotify(e) => Event::KeyboardGroup(u8::from(e.group)),
            XEvent::XkbNamesNotify(_) | XEvent::XkbNewKeyboardNotify(_) => Event::KeyboardLayouts,
            XEvent::MappingNotify(_) => {
                // Сменилась раскладка клавиатуры (например, setxkbmap) —
                // перечитываем таблицу клавиш и сообщаем о новом наборе.
                if let Ok(k) = Keymap::load(&self.conn) {
                    *self.keymap.lock().unwrap_or_else(|p| p.into_inner()) = k;
                }
                Event::KeyboardLayouts
            }
            other => {
                if std::env::var_os("SHELL_DEBUG_DOCK").is_some() {
                    eprintln!("X (не разобрано): {other:?}");
                }
                Event::Other
            }
        }
    }
}

/// Всплывающее окно.
pub struct Popup {
    window: Window,
    gc: u32,
}

/// Цвет окна блокировки до первого кадра: непрозрачный тёмный (ARGB).
const LOCK_BACKGROUND: u32 = 0xFF1C_1C20;

impl Popup {
    /// Показывает окно в заданном месте и забирает себе клавиатуру и мышь.
    pub fn show(&self, d: &Display, x: i16, y: i16, w: u16, h: u16) -> Result<()> {
        d.refresh_keymap()?;
        self.set_geometry(d, x, y, w, h)?;
        d.conn.map_window(self.window)?;
        d.conn.configure_window(
            self.window,
            &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
        )?;
        d.conn.flush()?;
        // Пока держат горячую клавишу, клавиатура ещё занята тем, кто её поймал,
        // поэтому пробуем несколько раз (до ~1 с).
        for _ in 0..100 {
            let status = d
                .conn
                .grab_keyboard(
                    true,
                    self.window,
                    CURRENT_TIME,
                    GrabMode::ASYNC,
                    GrabMode::ASYNC,
                )?
                .reply()?
                .status;
            if status == GrabStatus::SUCCESS {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let mask = EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE | EventMask::POINTER_MOTION;
        d.conn
            .grab_pointer(
                true,
                self.window,
                mask,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
                NONE,
                NONE,
                CURRENT_TIME,
            )?
            .reply()?;
        Ok(())
    }

    /// Показывает окно поверх остальных, не забирая клавиатуру и мышь
    /// (например, значок, который тащат из дока).
    pub fn show_passive(&self, d: &Display, x: i16, y: i16, w: u16, h: u16) -> Result<()> {
        self.set_geometry(d, x, y, w, h)?;
        d.conn.map_window(self.window)?;
        d.conn.configure_window(
            self.window,
            &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
        )?;
        d.conn.flush()?;
        Ok(())
    }

    /// Показывает окно на весь экран и забирает клавиатуру и мышь насовсем —
    /// для экрана блокировки. `false` — клавиатуру забрать не удалось: окно
    /// остаётся показанным, но ввод может перехватить кто-то другой (экран
    /// блокировки должен в этом случае выйти, чтобы сторож запустил его снова).
    pub fn show_lock(&self, d: &Display, w: u16, h: u16) -> Result<bool> {
        self.set_geometry(d, 0, 0, w, h)?;
        // Непрозрачный тёмный фон окна: сервер закрашивает им окно сам в
        // момент показа. Иначе окно с прозрачностью до первого кадра программы
        // (а фон рисуется секунды) пустое — то есть прозрачное, и рабочий стол
        // с уведомлениями виден сквозь «заблокированный» экран.
        d.conn.change_window_attributes(
            self.window,
            &ChangeWindowAttributesAux::new().background_pixel(LOCK_BACKGROUND),
        )?;
        d.conn.map_window(self.window)?;
        d.conn.clear_area(false, self.window, 0, 0, 0, 0)?;
        d.conn.configure_window(
            self.window,
            &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
        )?;
        // Как i3lock: фокус — корневому окну, чтобы нажатия не уходили окнам
        // программ, даже если захват клавиатуры на мгновение пропадёт.
        d.conn
            .set_input_focus(InputFocus::NONE, d.root, CURRENT_TIME)?;
        d.conn.flush()?;
        self.regrab(d)
    }

    /// Поднять окно поверх всех.
    pub fn raise(&self, d: &Display) -> Result<()> {
        d.conn.configure_window(
            self.window,
            &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
        )?;
        d.conn.flush()?;
        Ok(())
    }

    /// Проверить и вернуть себе захват клавиатуры. `true` — захват наш
    /// (уже был или только что забрали заново). Вызывается периодически:
    /// другая программа может снять захват, и экран блокировки обязан его
    /// вернуть (или уступить место новому — через сторожа).
    pub fn regrab(&self, d: &Display) -> Result<bool> {
        // Успех — только SUCCESS: повторный захват своим же соединением даёт
        // его же. ALREADY_GRABBED значит, что клавиатуру держит ДРУГАЯ
        // программа и нажатия уходят ей. Её захват может скоро кончиться
        // (меню, окно «Поиск») — несколько коротких попыток, дальше решает
        // вызывающий (экран блокировки повторит, не убирая окна).
        let mut keyboard = false;
        for _ in 0..5 {
            let status = d
                .conn
                .grab_keyboard(
                    false,
                    self.window,
                    CURRENT_TIME,
                    GrabMode::ASYNC,
                    GrabMode::ASYNC,
                )?
                .reply()?
                .status;
            if status == GrabStatus::SUCCESS {
                keyboard = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        // Мышь — чтобы щелчки не уходили окнам, оказавшимся поверх.
        let mask = EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE | EventMask::POINTER_MOTION;
        let mut pointer = false;
        for _ in 0..5 {
            let status = d
                .conn
                .grab_pointer(
                    false,
                    self.window,
                    mask,
                    GrabMode::ASYNC,
                    GrabMode::ASYNC,
                    NONE,
                    NONE,
                    CURRENT_TIME,
                )?
                .reply()?
                .status;
            if status == GrabStatus::SUCCESS {
                pointer = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok(keyboard && pointer)
    }

    /// Прячет окно, не трогая захват клавиатуры и мыши.
    pub fn hide_passive(&self, d: &Display) -> Result<()> {
        d.conn.unmap_window(self.window)?;
        d.conn.flush()?;
        Ok(())
    }

    /// Прячет окно и отпускает клавиатуру и мышь.
    pub fn hide(&self, d: &Display) -> Result<()> {
        d.conn.ungrab_keyboard(CURRENT_TIME)?;
        d.conn.ungrab_pointer(CURRENT_TIME)?;
        d.conn.unmap_window(self.window)?;
        d.conn.flush()?;
        Ok(())
    }

    /// Место и размер окна — в точках.
    pub fn set_geometry(&self, d: &Display, x: i16, y: i16, w: u16, h: u16) -> Result<()> {
        let (x, y, w, h) = d.rect_px(x.into(), y.into(), w.into(), h.into());
        d.conn.configure_window(
            self.window,
            &ConfigureWindowAux::new().x(x).y(y).width(w).height(h),
        )?;
        Ok(())
    }

    /// Выводит картинку в часть окна: левый верхний угол — в пикселе `x`, `y`
    /// (остальное в окне не меняется).
    pub fn present_at(&self, d: &Display, pixmap: &impl Image, x: i32, y: i32) -> Result<()> {
        put_at(d, self.window, self.gc, pixmap.image(), x, y)?;
        d.conn.flush()?;
        Ok(())
    }

    /// Выводит картинку (или холст) в окно (левый верхний угол к углу).
    pub fn present(&self, d: &Display, pixmap: &impl Image) -> Result<()> {
        put_pixmap(d, self.window, self.gc, pixmap.image())
    }

    pub fn id(&self) -> u32 {
        self.window
    }
}

/// Выводит картинку в 32-битное окно.
pub(crate) fn put_pixmap(d: &Display, window: Window, gc: u32, pixmap: &Pixmap) -> Result<()> {
    put_columns(d, window, gc, pixmap, 0, pixmap.width())
}

/// Выводит картинку в `drawable` (окно или картинку на сервере) левым
/// верхним углом в пиксель `x`, `y`. Полосами — у запроса к X есть предел.
pub(crate) fn put_at(
    d: &Display,
    drawable: u32,
    gc: u32,
    image: &Pixmap,
    x: i32,
    y: i32,
) -> Result<()> {
    let w = image.width() as usize;
    if w == 0 || image.height() == 0 {
        return Ok(());
    }
    let mut data = image.data().to_vec();
    // tiny-skia хранит RGBA, X-сервер ждёт BGRA.
    for px in data.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    let max_rows = (d.conn.maximum_request_bytes().saturating_sub(64) / (w * 4)).max(1);
    for (n, rows) in data.chunks(max_rows * w * 4).enumerate() {
        d.conn.put_image(
            ImageFormat::Z_PIXMAP,
            drawable,
            gc,
            w as u16,
            (rows.len() / (w * 4)) as u16,
            x as i16,
            (y + (n * max_rows) as i32) as i16,
            0,
            32,
            rows,
        )?;
    }
    Ok(())
}

/// Через общую память выводим только большие картинки: на маленьких
/// ожидание ответа сервера съедает выигрыш.
const SHM_MIN_BYTES: usize = 2 * 1024 * 1024;

/// Выводит столбцы `x0..x1` картинки в те же столбцы окна (остальное в окне
/// не меняется).
pub(crate) fn put_columns(
    d: &Display,
    window: Window,
    gc: u32,
    pixmap: &Pixmap,
    x0: u32,
    x1: u32,
) -> Result<()> {
    let (x0, x1) = (
        x0.min(pixmap.width()) as usize,
        x1.min(pixmap.width()) as usize,
    );
    let (full, h) = (pixmap.width() as usize, pixmap.height() as usize);
    let w = x1.saturating_sub(x0);
    if w == 0 || h == 0 {
        return Ok(());
    }
    // tiny-skia хранит RGBA, X-сервер ждёт BGRA.
    let src = pixmap.data();
    let mut data = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        data.extend_from_slice(&src[(y * full + x0) * 4..(y * full + x1) * 4]);
    }
    for px in data.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    if data.len() >= SHM_MIN_BYTES && d.shm_put(window, gc, (w as u16, h as u16), x0 as i16, &data)
    {
        return Ok(());
    }
    // Большую картинку отправляем полосами — у запроса к X есть предел размера.
    let max = d.conn.maximum_request_bytes().saturating_sub(64);
    let rows = (max / (w * 4)).clamp(1, h);
    for (i, chunk) in data.chunks(rows * w * 4).enumerate() {
        d.conn.put_image(
            ImageFormat::Z_PIXMAP,
            window,
            gc,
            w as u16,
            (chunk.len() / (w * 4)) as u16,
            x0 as i16,
            (i * rows) as i16,
            0,
            32,
            chunk,
        )?;
    }
    d.conn.flush()?;
    Ok(())
}

/// Таблица «код клавиши → символы» из X-сервера.
struct Keymap {
    min_code: u8,
    per_code: usize,
    syms: Vec<u32>,
}

impl Keymap {
    fn load(conn: &RustConnection) -> Result<Keymap> {
        let setup = conn.setup();
        let (min, max) = (setup.min_keycode, setup.max_keycode);
        let reply = conn.get_keyboard_mapping(min, max - min + 1)?.reply()?;
        Ok(Keymap {
            min_code: min,
            per_code: usize::from(reply.keysyms_per_keycode),
            syms: reply.keysyms,
        })
    }

    /// Символ клавиши с учётом раскладки, Shift и Caps Lock.
    fn keysym(&self, code: u8, state: u16) -> (u32, bool) {
        let Some(start) = usize::from(code)
            .checked_sub(usize::from(self.min_code))
            .map(|i| i * self.per_code)
        else {
            return (0, false);
        };
        let syms = self.syms.get(start..start + self.per_code).unwrap_or(&[]);
        let at = |i: usize| syms.get(i).copied().unwrap_or(0);
        // Раскладка — в битах 13–14; в общей таблице X на каждую из первых
        // двух приходится по две колонки: без Shift и с Shift.
        let group = usize::from((state >> 13) & 0x3).min(1);
        let shift = state & 0x1 != 0;
        let (base, shifted) = match (at(group * 2), at(group * 2 + 1)) {
            (0, 0) => (at(0), at(1)),
            pair => pair,
        };
        if shift && shifted != 0 {
            (shifted, false)
        } else {
            // Нет отдельного символа для Shift — значит, нужна заглавная буква.
            (base, shift)
        }
    }

    fn key(&self, code: u8, state: u16) -> Key {
        let (sym, want_upper) = self.keysym(code, state);
        let caps = state & 0x2 != 0;
        match sym {
            key::Return | key::KP_Enter => Key::Return,
            key::Escape => Key::Escape,
            key::BackSpace => Key::BackSpace,
            key::Delete | key::KP_Delete => Key::Delete,
            key::Tab | key::ISO_Left_Tab => Key::Tab,
            key::Up | key::KP_Up => Key::Up,
            key::Down | key::KP_Down => Key::Down,
            key::Left | key::KP_Left => Key::Left,
            key::Right | key::KP_Right => Key::Right,
            key::Home | key::KP_Home => Key::Home,
            key::End | key::KP_End => Key::End,
            key::Page_Up | key::KP_Page_Up => Key::PageUp,
            key::Page_Down | key::KP_Page_Down => Key::PageDown,
            key::F2 => Key::F2,
            key::XF86_AudioRaiseVolume => Key::VolumeUp,
            key::XF86_AudioLowerVolume => Key::VolumeDown,
            key::XF86_AudioMute | key::XF86_AudioMicMute => Key::VolumeMute,
            _ => match Keysym::new(sym).key_char() {
                Some(c) if !c.is_control() => {
                    // Caps Lock меняет регистр букв (и наоборот при зажатом Shift).
                    if c.is_alphabetic() && (want_upper != caps) {
                        Key::Char(swap_case(c))
                    } else {
                        Key::Char(c)
                    }
                }
                _ => Key::Other,
            },
        }
    }
}

fn swap_case(c: char) -> char {
    if c.is_lowercase() {
        c.to_uppercase().next().unwrap_or(c)
    } else {
        c.to_lowercase().next().unwrap_or(c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Две клавиши: «q/Q | й/Й» и «1/! | 1/!», плюс Enter.
    fn keymap() -> Keymap {
        Keymap {
            min_code: 8,
            per_code: 4,
            syms: vec![
                key::q,
                key::Q,
                key::Cyrillic_shorti,
                key::Cyrillic_SHORTI,
                key::_1,
                key::exclam,
                key::_1,
                key::exclam,
                key::Return,
                0,
                key::Return,
                0,
            ],
        }
    }

    const SHIFT: u16 = 0x1;
    const CAPS: u16 = 0x2;
    const RU: u16 = 1 << 13;

    #[test]
    fn layouts_and_case() {
        let k = keymap();
        assert_eq!(k.key(8, 0), Key::Char('q'));
        assert_eq!(k.key(8, SHIFT), Key::Char('Q'));
        assert_eq!(k.key(8, RU), Key::Char('й'));
        assert_eq!(k.key(8, RU | SHIFT), Key::Char('Й'));
        assert_eq!(k.key(8, RU | CAPS), Key::Char('Й'));
        assert_eq!(k.key(8, RU | CAPS | SHIFT), Key::Char('й'));
        // Caps Lock не трогает цифры.
        assert_eq!(k.key(9, CAPS), Key::Char('1'));
        assert_eq!(k.key(9, SHIFT), Key::Char('!'));
        assert_eq!(k.key(10, RU), Key::Return);
        assert_eq!(k.key(200, 0), Key::Other);
    }
}

#[cfg(test)]
mod dpi_tests {
    use super::*;

    #[test]
    fn dpi_from_resources_and_scale() {
        let r = "Xft.antialias:\t1\nXft.dpi:\t144\nXft.hinting:\t1\n";
        assert_eq!(parse_dpi(r), Some(144.0));
        assert_eq!(parse_dpi("Xft.antialias:\t1\n"), None);
        assert_eq!(scale_for_dpi(96.0), 1.0);
        assert_eq!(scale_for_dpi(144.0), 1.5);
        assert_eq!(scale_for_dpi(120.0), 1.25);
        assert_eq!(scale_for_dpi(110.0), 1.25);
        assert_eq!(scale_for_dpi(72.0), 1.0);
        assert_eq!(scale_for_dpi(192.0), 2.0);
    }
}
