//! Window policy, independent of the display protocol. Stack order is bottom to top.
pub type WindowId = u64;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}
#[derive(Clone, Debug)]
pub struct Window {
    pub id: WindowId,
    pub rect: Rect,
}
#[derive(Debug)]
pub struct Model {
    pub windows: Vec<Window>,
    pub focus: Option<WindowId>,
    pub area: Rect,
    next: WindowId,
    cascade: usize,
}
impl Model {
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
            next: 0,
            cascade: 0,
        }
    }
    pub fn open(&mut self, width: i32, height: i32) -> WindowId {
        self.next += 1;
        let width = width.max(1).min(self.area.width.max(1));
        let height = height.max(1).min(self.area.height.max(1));
        let steps =
            ((self.area.width - width).min(self.area.height - height) / 32).max(0) as usize + 1;
        let offset = (self.cascade % steps) as i32 * 32;
        self.cascade += 1;
        self.windows.push(Window {
            id: self.next,
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
    pub fn close(&mut self, id: WindowId) {
        self.windows.retain(|w| w.id != id);
        if self.focus == Some(id) {
            self.focus = self.windows.last().map(|w| w.id);
        }
    }
    pub fn raise(&mut self, id: WindowId) {
        if let Some(index) = self.windows.iter().position(|w| w.id == id) {
            let window = self.windows.remove(index);
            self.windows.push(window);
            self.focus = Some(id);
        }
    }
    pub fn rect(&self, id: WindowId) -> Option<Rect> {
        self.windows.iter().find(|w| w.id == id).map(|w| w.rect)
    }
    pub fn geometry(&mut self, id: WindowId, mut rect: Rect) {
        rect.y = rect.y.max(self.area.y);
        rect.width = rect.width.max(1);
        rect.height = rect.height.max(1);
        if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
            w.rect = rect;
        }
    }
    pub fn resize_output(&mut self, width: i32, height: i32) {
        self.area.width = width.max(1);
        self.area.height = height.max(1);
        for w in &mut self.windows {
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
                height: 600
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
