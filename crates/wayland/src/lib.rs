//! The Wayland backend of the shell's door (`apps/mimshel/src/platform.rs`):
//! the same events and windows the X11 backend gives, from a Wayland
//! compositor. Step 2.4 of `docs/compositor.md`, starting with what a single
//! app window needs (the terminal): an `xdg_toplevel` with shm buffers, the
//! keyboard with xkb layouts and key repeat, the pointer and the wheel.
//!
//! Threads: one thread waits in [`Display::next_event`] (it alone reads the
//! socket and dispatches); the others only send requests (present a picture,
//! set a title). Both sides share the state behind a mutex; the reader never
//! holds it while it waits.

use std::collections::VecDeque;
use std::num::NonZeroU32;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use mimshel_platform::{Event, Key, KeyPress, Property, Result};
use mimshel_render::Image;
use rustix::event::{PollFd, PollFlags, poll};
use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState};
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::seat::keyboard::{
    KeyEvent, KeyboardHandler, Keysym, Modifiers, RepeatInfo,
};
use smithay_client_toolkit::seat::pointer::{PointerEvent, PointerEventKind, PointerHandler};
use smithay_client_toolkit::seat::{Capability, SeatHandler, SeatState};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shell::xdg::XdgShell;
use smithay_client_toolkit::shell::xdg::window::{
    Window, WindowConfigure, WindowDecorations, WindowHandler,
};
use smithay_client_toolkit::shm::slot::{Buffer, SlotPool};
use smithay_client_toolkit::shm::{Shm, ShmHandler};
use smithay_client_toolkit::{
    delegate_compositor, delegate_keyboard, delegate_output, delegate_pointer, delegate_registry,
    delegate_seat, delegate_shm, delegate_xdg_shell, delegate_xdg_window, registry_handlers,
};
use wayland_client::globals::registry_queue_init;
use wayland_client::protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface};
use wayland_client::{Connection, EventQueue, QueueHandle};

/// Linux input codes of the mouse buttons (`linux/input-event-codes.h`).
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;
const BTN_SIDE: u32 = 0x113;
const BTN_EXTRA: u32 = 0x114;

/// The connection to a Wayland compositor.
pub struct Display {
    conn: Connection,
    qh: QueueHandle<State>,
    /// Only the thread in [`Display::next_event`] takes this (and at start).
    queue: Mutex<EventQueue<State>>,
    state: Mutex<State>,
}

/// An app window ([`Display::create_app_window`]), named by its id.
pub struct AppWindow {
    id: u32,
}

struct Win {
    id: u32,
    window: Window,
    /// Size in points (logical coordinates).
    size: (u32, u32),
    configured: bool,
    /// The last buffer shown, to update only a rectangle of it when the
    /// compositor has released it.
    buffer: Option<Buffer>,
}

struct Held {
    key: KeyPress,
    code: u32,
    next: Instant,
}

struct State {
    registry: RegistryState,
    seats: SeatState,
    outputs: OutputState,
    compositor: CompositorState,
    xdg: XdgShell,
    shm: Shm,
    pool: Option<SlotPool>,
    windows: Vec<Win>,
    next_id: u32,
    events: VecDeque<Event>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    pointer: Option<wl_pointer::WlPointer>,
    modifiers: Modifiers,
    focus: Option<u32>,
    /// Key repeat: (delay, interval); `None` — the compositor said no repeat.
    repeat: Option<(Duration, Duration)>,
    held: Option<Held>,
    /// Points to pixels, and the integer buffer scale used for surfaces.
    scale: f32,
    buffer_scale: i32,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Display {
    /// Connect to the compositor named by `WAYLAND_DISPLAY`.
    pub fn connect() -> Result<Self> {
        let conn = Connection::connect_to_env()?;
        let (globals, mut queue) = registry_queue_init::<State>(&conn)?;
        let qh = queue.handle();
        let scale = mimshel_render::scale();
        let mut state = State {
            registry: RegistryState::new(&globals),
            seats: SeatState::new(&globals, &qh),
            outputs: OutputState::new(&globals, &qh),
            compositor: CompositorState::bind(&globals, &qh)?,
            xdg: XdgShell::bind(&globals, &qh)?,
            shm: Shm::bind(&globals, &qh)?,
            pool: None,
            windows: Vec::new(),
            next_id: 1,
            events: VecDeque::new(),
            keyboard: None,
            pointer: None,
            modifiers: Modifiers::default(),
            focus: None,
            repeat: Some((Duration::from_millis(600), Duration::from_millis(40))),
            held: None,
            scale,
            // Fractional scales would need wp_viewporter; until then the
            // nearest whole number.
            buffer_scale: (scale.round() as i32).max(1),
        };
        // Seats and outputs announce themselves in the first roundtrips.
        queue.roundtrip(&mut state)?;
        queue.roundtrip(&mut state)?;
        Ok(Self {
            conn,
            qh,
            queue: Mutex::new(queue),
            state: Mutex::new(state),
        })
    }

    /// Wait for the next event. Reads the socket without holding the state,
    /// and makes key repeats while a key is held.
    pub fn next_event(&self) -> Result<Event> {
        loop {
            let timeout = {
                let mut state = lock(&self.state);
                if let Some(ev) = state.events.pop_front() {
                    return Ok(ev);
                }
                match state.repeat_due(Instant::now()) {
                    Ok(ev) => return Ok(ev),
                    Err(wait) => wait,
                }
            };
            self.conn.flush()?;
            let mut queue = lock(&self.queue);
            if queue.dispatch_pending(&mut lock(&self.state))? > 0 {
                continue;
            }
            if let Some(guard) = queue.prepare_read() {
                let ready = {
                    let fd = guard.connection_fd();
                    let mut fds = [PollFd::new(&fd, PollFlags::IN)];
                    let ms = timeout.map_or(-1, |d| d.as_millis().min(i32::MAX as u128) as i32);
                    match poll(&mut fds, ms) {
                        Ok(n) => n > 0,
                        Err(rustix::io::Errno::INTR) => false,
                        Err(e) => return Err(e.into()),
                    }
                };
                if ready {
                    guard.read()?;
                }
            }
            queue.dispatch_pending(&mut lock(&self.state))?;
        }
    }

    /// Points to pixels.
    pub fn scale(&self) -> f32 {
        lock(&self.state).scale
    }

    /// Points to pixels, rounded as the X11 backend does.
    pub fn px(&self, points: u32) -> u32 {
        mimshel_render::to_px(points, lock(&self.state).scale)
    }

    /// The first output's size in pixels (`(0, 0)` before it is known).
    pub fn screen_size(&self) -> (u16, u16) {
        let state = lock(&self.state);
        state
            .outputs
            .outputs()
            .find_map(|o| state.outputs.info(&o))
            .and_then(|info| info.modes.iter().find(|m| m.current).map(|m| m.dimensions))
            .map_or((0, 0), |(w, h)| {
                (
                    u16::try_from(w).unwrap_or(u16::MAX),
                    u16::try_from(h).unwrap_or(u16::MAX),
                )
            })
    }

    /// Send everything queued. (A full roundtrip would need the reading
    /// thread's queue.)
    pub fn sync(&self) -> Result<()> {
        self.conn.flush()?;
        Ok(())
    }

    /// Our window that has the keyboard.
    pub fn active_window(&self) -> Option<u32> {
        lock(&self.state).focus
    }

    /// Ctrl and Shift held, in that order.
    pub fn modifiers(&self) -> (bool, bool) {
        let m = lock(&self.state).modifiers;
        (m.ctrl, m.shift)
    }

    /// A titled toplevel; `class` is its app id. Sizes are in points. It
    /// maps when its first picture arrives after the compositor's first
    /// configure ([`Event::Expose`] says when to draw).
    pub fn create_app_window(
        &self,
        class: &str,
        title: &str,
        size: (u32, u32),
        min: (u32, u32),
    ) -> Result<AppWindow> {
        let mut state = lock(&self.state);
        let surface = state.compositor.create_surface(&self.qh);
        surface.set_buffer_scale(state.buffer_scale);
        let window = state
            .xdg
            .create_window(surface, WindowDecorations::RequestServer, &self.qh);
        window.set_title(title);
        window.set_app_id(class);
        window.set_min_size(Some(min));
        window.commit();
        let id = state.next_id;
        state.next_id += 1;
        state.windows.push(Win {
            id,
            window,
            size,
            configured: false,
            buffer: None,
        });
        drop(state);
        self.conn.flush()?;
        Ok(AppWindow { id })
    }
}

impl AppWindow {
    pub fn id(&self) -> u32 {
        self.id
    }

    /// Nothing to do: a Wayland window maps with its first picture.
    pub fn show(&self, d: &Display) -> Result<()> {
        d.conn.flush()?;
        Ok(())
    }

    pub fn set_title(&self, d: &Display, title: &str) -> Result<()> {
        if let Some(w) = lock(&d.state).window(self.id) {
            w.window.set_title(title);
        }
        d.conn.flush()?;
        Ok(())
    }

    /// Change the size: the next picture has the new size. Tells the window
    /// itself through [`Event::Configure`], as the X11 server would.
    pub fn resize(&self, d: &Display, size: (u32, u32)) -> Result<()> {
        let mut state = lock(&d.state);
        if let Some(w) = state.window(self.id) {
            w.size = size;
        }
        state.events.push_back(Event::Configure { window: self.id });
        Ok(())
    }

    /// The size in points.
    pub fn size(&self, d: &Display) -> Option<(u32, u32)> {
        lock(&d.state).window(self.id).map(|w| w.size)
    }

    /// Show a picture the size of the window, in pixels.
    pub fn present(&self, d: &Display, image: &impl Image) -> Result<()> {
        let pixmap = image.image();
        self.present_part(d, pixmap, (0, 0, pixmap.width(), pixmap.height()))
    }

    /// Show only `(x0, y0, x1, y1)` of a whole-window picture: written into
    /// the last buffer when the compositor has released it, else the whole
    /// picture goes into a new buffer.
    pub fn present_rect(
        &self,
        d: &Display,
        image: &impl Image,
        rect: (u32, u32, u32, u32),
    ) -> Result<()> {
        self.present_part(d, image.image(), rect)
    }

    fn present_part(
        &self,
        d: &Display,
        pixmap: &tiny_skia::Pixmap,
        rect: (u32, u32, u32, u32),
    ) -> Result<()> {
        let mut guard = lock(&d.state);
        let state = &mut *guard;
        let (w, h) = (pixmap.width(), pixmap.height());
        let stride = w * 4;
        let pool = match &mut state.pool {
            Some(pool) => pool,
            slot => slot.insert(SlotPool::new((stride * h) as usize, &state.shm)?),
        };
        let Some(win) = state.windows.iter_mut().find(|win| win.id == self.id) else {
            return Ok(());
        };
        if !win.configured {
            // Too early: the compositor's first configure brings an Expose.
            return Ok(());
        }
        let reuse = win
            .buffer
            .as_ref()
            .filter(|b| b.height() == h as i32 && b.stride() == stride as i32)
            .is_some_and(|b| pool.canvas(b).is_some());
        let rect = if reuse {
            rect
        } else {
            let (buffer, _) =
                pool.create_buffer(w as i32, h as i32, stride as i32, wl_shm::Format::Argb8888)?;
            win.buffer = Some(buffer);
            (0, 0, w, h)
        };
        let Some(buffer) = &win.buffer else {
            return Ok(());
        };
        let Some(canvas) = pool.canvas(buffer) else {
            return Ok(());
        };
        copy_rect(pixmap, canvas, rect);
        let surface = win.window.wl_surface();
        let (x0, y0, x1, y1) = rect;
        surface.damage_buffer(
            x0 as i32,
            y0 as i32,
            x1.saturating_sub(x0) as i32,
            y1.saturating_sub(y0) as i32,
        );
        buffer.attach_to(surface)?;
        win.window.commit();
        drop(guard);
        d.conn.flush()?;
        Ok(())
    }

    pub fn destroy(&self, d: &Display) -> Result<()> {
        let mut state = lock(&d.state);
        state.windows.retain(|w| w.id != self.id);
        if state.focus == Some(self.id) {
            state.focus = None;
            state.held = None;
        }
        drop(state);
        d.conn.flush()?;
        Ok(())
    }
}

/// Copy `rect` of a premultiplied RGBA picture into an ARGB8888 shm buffer
/// (little-endian: B, G, R, A).
fn copy_rect(
    pixmap: &tiny_skia::Pixmap,
    canvas: &mut [u8],
    (x0, y0, x1, y1): (u32, u32, u32, u32),
) {
    let w = pixmap.width() as usize;
    let src = pixmap.data();
    let (x0, x1) = (x0 as usize, (x1 as usize).min(w));
    let y1 = (y1 as usize).min(pixmap.height() as usize);
    for y in y0 as usize..y1 {
        let from = (y * w + x0) * 4;
        let to = (y * w + x1) * 4;
        let (Some(src), Some(dst)) = (src.get(from..to), canvas.get_mut(from..to)) else {
            return;
        };
        for (s, d) in src
            .as_chunks::<4>()
            .0
            .iter()
            .zip(dst.as_chunks_mut::<4>().0)
        {
            *d = [s[2], s[1], s[0], s[3]];
        }
    }
}

impl State {
    fn window(&mut self, id: u32) -> Option<&mut Win> {
        self.windows.iter_mut().find(|w| w.id == id)
    }

    fn id_of(&self, surface: &wl_surface::WlSurface) -> Option<u32> {
        self.windows
            .iter()
            .find(|w| w.window.wl_surface() == surface)
            .map(|w| w.id)
    }

    /// A repeat that is due now (`Ok`), or how long until the next one
    /// (`Err(None)` — no key held).
    fn repeat_due(&mut self, now: Instant) -> std::result::Result<Event, Option<Duration>> {
        let interval = self.repeat.map(|(_, interval)| interval);
        match (&mut self.held, interval) {
            (Some(held), Some(interval)) if held.next <= now => {
                held.next = now + interval;
                Ok(Event::Key(held.key))
            }
            (Some(held), Some(_)) => Err(Some(held.next - now)),
            _ => Err(None),
        }
    }

    fn key_press(&self, event: &KeyEvent) -> KeyPress {
        let m = self.modifiers;
        let key = mimshel_platform::special_key(event.keysym.raw()).unwrap_or_else(|| {
            // The keysym already follows the layout, Shift and Caps Lock.
            match event.keysym.key_char() {
                Some(c) if !c.is_control() => Key::Char(c),
                _ => Key::Other,
            }
        });
        KeyPress {
            key,
            ctrl: m.ctrl,
            shift: m.shift,
            alt: m.alt,
            logo: m.logo,
        }
    }
}

impl CompositorHandler for State {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: i32,
    ) {
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for State {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl WindowHandler for State {
    fn request_close(&mut self, _: &Connection, _: &QueueHandle<Self>, window: &Window) {
        if let Some(id) = self.id_of(window.wl_surface()) {
            self.events.push_back(Event::Close { window: id });
        }
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        window: &Window,
        configure: WindowConfigure,
        _: u32,
    ) {
        let Some(id) = self.id_of(window.wl_surface()) else {
            return;
        };
        let Some(win) = self.window(id) else {
            return;
        };
        let size = match configure.new_size {
            (Some(w), Some(h)) => (w.get(), h.get()),
            _ => win.size,
        };
        let changed = size != win.size;
        let first = !win.configured;
        win.size = size;
        win.configured = true;
        if changed {
            win.buffer = None;
            self.events.push_back(Event::Configure { window: id });
        }
        if first || changed {
            self.events.push_back(Event::Expose { window: id });
        }
    }
}

impl SeatHandler for State {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seats
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            self.keyboard = self.seats.get_keyboard(qh, &seat, None).ok();
        }
        if capability == Capability::Pointer && self.pointer.is_none() {
            self.pointer = self.seats.get_pointer(qh, &seat).ok();
        }
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard
            && let Some(keyboard) = self.keyboard.take()
        {
            keyboard.release();
            self.held = None;
        }
        if capability == Capability::Pointer
            && let Some(pointer) = self.pointer.take()
        {
            pointer.release();
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl KeyboardHandler for State {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        surface: &wl_surface::WlSurface,
        _: u32,
        _: &[u32],
        _: &[Keysym],
    ) {
        self.focus = self.id_of(surface);
        self.events.push_back(Event::Property {
            window: 0,
            what: Property::ActiveWindow,
        });
    }

    fn leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: &wl_surface::WlSurface,
        _: u32,
    ) {
        self.focus = None;
        self.held = None;
        self.events.push_back(Event::Property {
            window: 0,
            what: Property::ActiveWindow,
        });
    }

    fn press_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        let key = self.key_press(&event);
        self.events.push_back(Event::Key(key));
        self.held = match (self.repeat, key.key) {
            (_, Key::Other) | (None, _) => None,
            (Some((delay, _)), _) => Some(Held {
                key,
                code: event.raw_code,
                next: Instant::now() + delay,
            }),
        };
    }

    fn release_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        if self.held.as_ref().is_some_and(|h| h.code == event.raw_code) {
            self.held = None;
        }
    }

    fn update_modifiers(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        modifiers: Modifiers,
        _: u32,
    ) {
        self.modifiers = modifiers;
    }

    fn update_repeat_info(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        info: RepeatInfo,
    ) {
        self.repeat = match info {
            RepeatInfo::Repeat { rate, delay } => Some((
                Duration::from_millis(u64::from(delay)),
                Duration::from_millis(1000 / u64::from(NonZeroU32::get(rate))),
            )),
            RepeatInfo::Disable => None,
        };
    }
}

impl PointerHandler for State {
    fn pointer_frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            let Some(window) = self.id_of(&event.surface) else {
                continue;
            };
            // Surface coordinates are points (the buffer scale matches).
            let (x, y) = (
                event.position.0.floor() as i16,
                event.position.1.floor() as i16,
            );
            match event.kind {
                PointerEventKind::Enter { .. } => {
                    self.events.push_back(Event::Enter { window, x, y });
                }
                PointerEventKind::Leave { .. } => {
                    self.events.push_back(Event::Leave { window });
                }
                PointerEventKind::Motion { .. } => {
                    self.events.push_back(Event::Motion { window, x, y });
                }
                PointerEventKind::Press { button, .. } => {
                    if let Some(button) = x11_button(button) {
                        self.events.push_back(Event::Button {
                            window,
                            x,
                            y,
                            button,
                        });
                    }
                }
                PointerEventKind::Release { button, .. } => {
                    if let Some(button) = x11_button(button) {
                        self.events.push_back(Event::Release {
                            window,
                            x,
                            y,
                            button,
                        });
                    }
                }
                PointerEventKind::Axis { vertical, .. } => {
                    // The wheel as X11 buttons 4 (up) and 5 (down), one per step.
                    let steps = if vertical.discrete != 0 {
                        vertical.discrete
                    } else {
                        (vertical.absolute / 10.0).round() as i32
                    };
                    let button = if steps < 0 { 4 } else { 5 };
                    for _ in 0..steps.unsigned_abs().min(10) {
                        self.events.push_back(Event::Button {
                            window,
                            x,
                            y,
                            button,
                        });
                        self.events.push_back(Event::Release {
                            window,
                            x,
                            y,
                            button,
                        });
                    }
                }
            }
        }
    }
}

/// Linux button codes as the X11 button numbers the shell's parts know.
fn x11_button(code: u32) -> Option<u8> {
    Some(match code {
        BTN_LEFT => 1,
        BTN_MIDDLE => 2,
        BTN_RIGHT => 3,
        BTN_SIDE => 8,
        BTN_EXTRA => 9,
        _ => return None,
    })
}

impl ShmHandler for State {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for State {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }

    registry_handlers![OutputState, SeatState];
}

delegate_compositor!(State);
delegate_output!(State);
delegate_shm!(State);
delegate_seat!(State);
delegate_keyboard!(State);
delegate_pointer!(State);
delegate_xdg_shell!(State);
delegate_xdg_window!(State);
delegate_registry!(State);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buttons_like_x11() {
        assert_eq!(x11_button(BTN_LEFT), Some(1));
        assert_eq!(x11_button(BTN_MIDDLE), Some(2));
        assert_eq!(x11_button(BTN_RIGHT), Some(3));
        assert_eq!(x11_button(0x1234), None);
    }

    #[test]
    fn copies_a_rectangle_as_bgra() {
        let mut pixmap = tiny_skia::Pixmap::new(3, 2).unwrap();
        pixmap.fill(tiny_skia::Color::from_rgba8(10, 20, 30, 255));
        let mut canvas = vec![0u8; 3 * 2 * 4];
        copy_rect(&pixmap, &mut canvas, (1, 1, 3, 2));
        // Only the second row, from the second pixel on.
        assert_eq!(&canvas[..16], &[0; 16]);
        assert_eq!(&canvas[16..20], &[30, 20, 10, 255]);
        assert_eq!(&canvas[20..24], &[30, 20, 10, 255]);
    }
}
