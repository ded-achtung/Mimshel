//! Window policy, independent of the display protocol. Stack order is bottom to top.

/// Stable identity of a mapped policy window.
pub type WindowId = u64;

/// Logical window or output geometry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    /// Horizontal origin.
    pub x: i32,
    /// Vertical origin.
    pub y: i32,
    /// Logical width.
    pub width: i32,
    /// Logical height.
    pub height: i32,
}

/// Window presentation mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// Floating window.
    #[default]
    Normal,
    /// Output-sized window with a title bar.
    Maximized,
    /// Output-sized window without decorations.
    Fullscreen,
    /// Hidden window.
    Minimized,
}

/// Logical title bar height.
pub const TITLE_HEIGHT: i32 = 30;

/// A window and its policy geometry.
#[derive(Clone, Debug)]
pub struct Window {
    /// Stable policy identity.
    pub id: WindowId,
    /// Frame origin and client size in logical points.
    pub rect: Rect,
    /// Presentation mode.
    pub mode: Mode,
    /// Floating geometry saved before maximizing or entering fullscreen.
    pub saved: Option<Rect>,
    /// Whether this window uses compositor decorations.
    pub decorated: bool,
    /// Cached X11 title, updated on property events.
    pub x11_title: String,
    /// Minimum client size.
    pub min_size: (i32, i32),
    /// Maximum client size; zero means unconstrained.
    pub max_size: (i32, i32),
    resume: Mode,
    minimized_order: u64,
}

/// Window stack, focus, and output placement policy.
#[derive(Debug)]
pub struct Model {
    /// Mapped windows in stack or creation order.
    pub windows: Vec<Window>,
    /// Focused policy window.
    pub focus: Option<WindowId>,
    /// Available logical output bounds.
    pub area: Rect,
    /// Complete output bounds, including shell exclusive zones.
    pub output: Rect,
    next: WindowId,
    cascade: usize,
    minimize_clock: u64,
}

impl Model {
    /// Create initialized state.
    pub fn new(width: i32, height: i32) -> Self {
        Self {
            windows: Vec::new(),
            focus: None,
            area: Rect {
                x: 0,
                y: 0,
                width,
                height,
            },
            output: Rect {
                x: 0,
                y: 0,
                width,
                height,
            },
            next: 0,
            cascade: 0,
            minimize_clock: 0,
        }
    }

    /// Place and focus a new window.
    pub fn open(&mut self, width: i32, height: i32) -> WindowId {
        self.next += 1;
        let width = width.max(1).min(self.area.width.max(1));
        let height = height.max(1).min((self.area.height - TITLE_HEIGHT).max(1));
        let steps = ((self.area.width - width).min(self.area.height - height - TITLE_HEIGHT) / 32)
            .max(0) as usize
            + 1;
        let offset = (self.cascade % steps) as i32 * 32;
        self.cascade += 1;
        self.windows.push(Window {
            id: self.next,
            mode: Mode::Normal,
            saved: None,
            decorated: true,
            x11_title: String::new(),
            min_size: (1, 1),
            max_size: (0, 0),
            resume: Mode::Normal,
            minimized_order: 0,
            rect: Rect {
                x: self.area.x + offset,
                y: self.area.y + offset,
                width,
                height,
            },
        });
        self.focus = Some(self.next);
        self.next
    }

    /// Remove a window and restore focus.
    pub fn close(&mut self, id: WindowId) {
        self.windows.retain(|w| w.id != id);
        if self.focus == Some(id) {
            self.focus = self
                .windows
                .iter()
                .rev()
                .find(|w| w.mode != Mode::Minimized)
                .map(|w| w.id);
        }
    }

    /// Raise and focus an existing window.
    pub fn raise(&mut self, id: WindowId) {
        if let Some(index) = self
            .windows
            .iter()
            .position(|w| w.id == id && w.mode != Mode::Minimized)
        {
            let window = self.windows.remove(index);
            self.windows.push(window);
            self.focus = Some(id);
        }
    }

    /// Find the current geometry of a window.
    pub fn rect(&self, id: WindowId) -> Option<Rect> {
        self.windows.iter().find(|w| w.id == id).map(|w| w.rect)
    }

    /// Apply window geometry with minimum size and top bounds.
    pub fn geometry(&mut self, id: WindowId, mut rect: Rect) {
        rect.y = rect.y.max(self.area.y);
        rect.width = rect.width.max(1);
        rect.height = rect.height.max(1);
        if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
            constrain(w, &mut rect);
            w.rect = rect;
        }
    }

    /// Title bar height for the current mode.
    pub fn title_height(&self, id: WindowId) -> i32 {
        self.windows
            .iter()
            .find(|w| w.id == id)
            .filter(|w| w.decorated && w.mode != Mode::Fullscreen)
            .map_or(0, |_| TITLE_HEIGHT)
    }

    /// Change presentation mode, preserving floating geometry and focus.
    pub fn set_mode(&mut self, id: WindowId, mode: Mode) {
        let Some(w) = self.windows.iter_mut().find(|w| w.id == id) else {
            return;
        };
        if w.mode == mode {
            return;
        }
        if mode == Mode::Minimized {
            self.minimize_clock += 1;
            w.minimized_order = self.minimize_clock;
            w.resume = w.mode;
        } else {
            let previous = if w.mode == Mode::Minimized {
                w.resume
            } else {
                w.mode
            };
            if previous == Mode::Normal && mode != Mode::Normal {
                w.saved = Some(w.rect);
            }
            if mode == Mode::Normal {
                if let Some(saved) = w.saved.take() {
                    w.rect = saved;
                }
            } else {
                w.rect = if mode == Mode::Fullscreen {
                    self.output
                } else {
                    self.area
                };
                if mode == Mode::Maximized && w.decorated {
                    w.rect.height = (w.rect.height - TITLE_HEIGHT).max(1);
                }
            }
        }
        if mode != Mode::Fullscreen {
            let mut rect = w.rect;
            constrain(w, &mut rect);
            w.rect = rect;
        }
        w.mode = mode;
        if mode == Mode::Minimized && self.focus == Some(id) {
            self.focus = self
                .windows
                .iter()
                .rev()
                .find(|w| w.mode != Mode::Minimized)
                .map(|w| w.id);
        }
    }

    /// Restore a particular minimized window to its previous presentation mode.
    pub fn restore(&mut self, id: WindowId) {
        if let Some(mode) = self
            .windows
            .iter()
            .find(|w| w.id == id && w.mode == Mode::Minimized)
            .map(|w| w.resume)
        {
            self.set_mode(id, mode);
        }
        self.raise(id);
    }

    /// Restore and focus the most recently minimized window.
    pub fn restore_recent(&mut self) -> Option<WindowId> {
        let (id, mode) = self
            .windows
            .iter()
            .filter(|w| w.mode == Mode::Minimized)
            .max_by_key(|w| w.minimized_order)
            .map(|w| (w.id, w.resume))?;
        self.set_mode(id, mode);
        self.raise(id);
        Some(id)
    }

    /// Update output bounds and keep window origins visible.
    pub fn resize_output(&mut self, width: i32, height: i32) {
        self.output.width = width.max(1);
        self.output.height = height.max(1);
        self.set_work_area(self.output);
    }

    /// Apply shell exclusive zones without constraining fullscreen windows.
    pub fn set_work_area(&mut self, area: Rect) {
        self.area = area;
        for w in &mut self.windows {
            if matches!(w.mode, Mode::Maximized | Mode::Fullscreen) {
                w.rect = if w.mode == Mode::Fullscreen {
                    self.output
                } else {
                    self.area
                };
                if w.mode == Mode::Maximized && w.decorated {
                    w.rect.height = (w.rect.height - TITLE_HEIGHT).max(1);
                }
                if w.mode == Mode::Maximized {
                    let mut rect = w.rect;
                    constrain(w, &mut rect);
                    w.rect = rect;
                }
                continue;
            }
            w.rect.x = w
                .rect
                .x
                .clamp(self.area.x, self.area.x + self.area.width - 1);
            w.rect.y = w
                .rect
                .y
                .clamp(self.area.y, self.area.y + self.area.height - 1);
        }
    }
}

fn constrain(window: &Window, rect: &mut Rect) {
    let limit = |value: i32, min: i32, max: i32| {
        value.max(min.max(1)).min(if max > 0 {
            max.max(min.max(1))
        } else {
            i32::MAX
        })
    };
    rect.width = limit(rect.width, window.min_size.0, window.max_size.0);
    rect.height = limit(rect.height, window.min_size.1, window.max_size.1);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stack_and_focus() {
        let mut m = Model::new(800, 600);
        let a = m.open(400, 300);
        let b = m.open(400, 300);
        assert_eq!(m.focus, Some(b));
        m.raise(a);
        assert_eq!(m.windows.iter().map(|w| w.id).collect::<Vec<_>>(), [b, a]);
        m.close(a);
        assert_eq!(m.focus, Some(b));
        m.close(b);
        assert_eq!(m.focus, None);
    }
    #[test]
    fn placement_and_oversize() {
        let mut m = Model::new(800, 600);
        for _ in 0..40 {
            let id = m.open(640, 400);
            let r = m.rect(id).unwrap();
            assert!(r.x >= 0 && r.y >= 0 && r.x + r.width <= 800 && r.y + r.height <= 600);
        }
        let id = m.open(2000, 2000);
        assert_eq!(
            m.rect(id),
            Some(Rect {
                x: 0,
                y: 0,
                width: 800,
                height: 570
            })
        );
    }
    #[test]
    fn top_clamp_and_background_close() {
        let mut m = Model::new(800, 600);
        let a = m.open(100, 100);
        let b = m.open(100, 100);
        m.close(a);
        assert_eq!(m.focus, Some(b));
        m.geometry(
            b,
            Rect {
                x: 10,
                y: -100,
                width: 50,
                height: 50,
            },
        );
        assert_eq!(m.rect(b).unwrap().y, 0);
    }
}

#[cfg(test)]
mod transition_tests {
    use super::*;

    #[test]
    fn modes_restore_floating_geometry() {
        for first in [Mode::Maximized, Mode::Fullscreen] {
            let mut m = Model::new(800, 600);
            let id = m.open(320, 200);
            let normal = m.rect(id);
            m.set_mode(id, first);
            assert_eq!(
                m.rect(id).unwrap().height,
                if first == Mode::Maximized { 570 } else { 600 }
            );
            m.set_mode(id, first);
            m.set_mode(id, Mode::Minimized);
            assert_eq!(m.focus, None);
            assert_eq!(m.restore_recent(), Some(id));
            assert_eq!(m.windows[0].mode, first);
            m.set_mode(id, Mode::Normal);
            assert_eq!(m.rect(id), normal);
            assert_eq!(m.windows[0].saved, None);
        }
    }

    #[test]
    fn switching_modes_and_output_resize() {
        let mut m = Model::new(800, 600);
        let id = m.open(320, 200);
        let normal = m.rect(id);
        m.set_mode(id, Mode::Maximized);
        m.set_mode(id, Mode::Fullscreen);
        assert_eq!(m.title_height(id), 0);
        m.resize_output(1000, 700);
        assert_eq!(m.rect(id), Some(m.area));
        m.set_mode(id, Mode::Maximized);
        assert_eq!(m.rect(id).unwrap().height, 670);
        m.set_mode(id, Mode::Normal);
        assert_eq!(m.rect(id), normal);
        m.windows[0].decorated = false;
        m.set_mode(id, Mode::Maximized);
        assert_eq!(m.rect(id), Some(m.area));
    }

    #[test]
    fn close_focus_skips_hidden_and_restore_is_recent() {
        let mut m = Model::new(800, 600);
        let a = m.open(100, 100);
        let b = m.open(100, 100);
        let c = m.open(100, 100);
        m.set_mode(b, Mode::Minimized);
        m.close(c);
        assert_eq!(m.focus, Some(a));
        m.raise(b);
        assert_eq!(m.focus, Some(a));
        m.set_mode(a, Mode::Minimized);
        assert_eq!(m.focus, None);
        assert_eq!(m.restore_recent(), Some(a));
        assert_eq!(m.restore_recent(), Some(b));
        assert_eq!(m.restore_recent(), None);
        m.close(b);
        assert_eq!(m.focus, Some(a));
    }
}
