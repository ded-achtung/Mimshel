//! Переименование значка прямо на столе, как на Mac: вместо подписи — поле
//! с именем, выделено имя без расширения. Enter — сохранить, Esc — оставить
//! как было, щелчок мимо — сохранить.

use std::path::PathBuf;

use shell_core::input::{Key, KeyPress};
use shell_render::Canvas;
use shell_text::{Fonts, Style};
use shell_widgets::field::{Edit, TextField};
use shell_widgets::shape::{paint, rounded};
use tiny_skia::{FillRule, Rect, Transform};

const TEXT: Style = Style {
    size: 12.5,
    color: [0x10, 0x10, 0x10, 0xff],
    weight: 500,
};
const PAD_X: f32 = 6.0;
const PAD_Y: f32 = 3.0;
const MIN_W: f32 = 96.0;
const MAX_W: f32 = 280.0;
const BG: [u8; 4] = [0xff, 0xff, 0xff, 0xff];
const BORDER: [u8; 4] = [0x0a, 0x64, 0xd6, 0xff];
const SELECTION: [u8; 4] = [0xb3, 0xd7, 0xff, 0xff];
const CURSOR: [u8; 4] = [0x10, 0x10, 0x10, 0xff];

/// Что сделать после нажатия.
#[derive(Debug, Clone, PartialEq)]
pub enum RenameResult {
    Nothing,
    /// Текст или курсор изменились — перерисовать (и, может, поменять ширину).
    Redraw,
    /// Сохранить новое имя.
    Commit(String),
    Cancel,
}

pub struct RenameBox {
    /// Какой файл переименовываем.
    pub path: PathBuf,
    field: TextField,
    original: String,
}

impl RenameBox {
    /// Поле для файла `path`: выделено имя без расширения (у папки — всё).
    pub fn new(path: PathBuf, is_dir: bool) -> RenameBox {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let stem = match name.rfind('.') {
            Some(dot) if !is_dir && dot > 0 => name[..dot].chars().count(),
            _ => name.chars().count(),
        };
        RenameBox {
            path,
            field: TextField::with_text(&name, stem),
            original: name,
        }
    }

    pub fn text(&self) -> &str {
        self.field.text()
    }

    pub fn key(&mut self, k: KeyPress) -> RenameResult {
        match k.key {
            Key::Return => self.commit(),
            Key::Escape => RenameResult::Cancel,
            _ => match self.field.key(k) {
                Edit::Changed | Edit::Moved => RenameResult::Redraw,
                Edit::Ignored => RenameResult::Nothing,
            },
        }
    }

    /// Сохранить: пустое или прежнее имя — просто закрыть.
    pub fn commit(&self) -> RenameResult {
        let name = self.field.text().trim();
        if name.is_empty() || name == self.original {
            RenameResult::Cancel
        } else {
            RenameResult::Commit(name.to_string())
        }
    }

    /// Размер поля в точках.
    pub fn size(&self, fonts: &mut Fonts) -> (u32, u32) {
        let w = (fonts.measure(self.field.text(), &TEXT) + 2.0 * PAD_X + 2.0).clamp(MIN_W, MAX_W);
        let h = TEXT.line_height() + 2.0 * PAD_Y;
        (w.ceil() as u32, h.ceil() as u32)
    }

    pub fn render(&self, fonts: &mut Fonts) -> Canvas {
        let (w, h) = self.size(fonts);
        let mut pm = Canvas::new(w, h).expect("размер не нулевой");
        let (wf, hf) = (w as f32, h as f32);
        let id = Transform::identity();
        if let Some(r) = rounded(0.0, 0.0, wf, hf, 4.0) {
            pm.fill_path(&r, &paint(BORDER), FillRule::Winding, id, None);
        }
        if let Some(r) = rounded(1.5, 1.5, wf - 3.0, hf - 3.0, 3.0) {
            pm.fill_path(&r, &paint(BG), FillRule::Winding, id, None);
        }
        // Длинное имя — показываем конец, где курсор.
        let text_w = fonts.measure(self.field.text(), &TEXT);
        let room = wf - 2.0 * PAD_X;
        let cursor_x = fonts.measure(self.field.before_cursor(), &TEXT);
        let shift = (cursor_x - room + 2.0).clamp(0.0, (text_w - room).max(0.0));
        let x = PAD_X - shift;
        let selection = self.field.selection();
        if !selection.is_empty() {
            let sw = fonts.measure(selection, &TEXT);
            if let Some(r) = Rect::from_xywh(x, PAD_Y, sw, TEXT.line_height()) {
                pm.fill_rect(r, &paint(SELECTION), id, None);
            }
        }
        fonts.draw(&mut pm, self.field.text(), &TEXT, x, PAD_Y, None);
        if selection.is_empty()
            && let Some(r) =
                Rect::from_xywh(x + cursor_x, PAD_Y + 1.0, 1.2, TEXT.line_height() - 2.0)
        {
            pm.fill_rect(r, &paint(CURSOR), id, None);
        }
        pm
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(b: &mut RenameBox, text: &str) {
        for c in text.chars() {
            b.key(KeyPress::plain(Key::Char(c)));
        }
    }

    #[test]
    fn new_folder_name_replaced_by_typing() {
        let mut b = RenameBox::new(PathBuf::from("/стол/Новая папка"), true);
        typed(&mut b, "Фото");
        assert_eq!(
            b.key(KeyPress::plain(Key::Return)),
            RenameResult::Commit("Фото".into())
        );
    }

    #[test]
    fn extension_kept_and_unchanged_name_cancels() {
        let mut b = RenameBox::new(PathBuf::from("/стол/отчёт.pdf"), false);
        typed(&mut b, "итог");
        assert_eq!(b.text(), "итог.pdf");
        let b = RenameBox::new(PathBuf::from("/стол/отчёт.pdf"), false);
        assert_eq!(b.commit(), RenameResult::Cancel, "имя не меняли");
        let mut b = RenameBox::new(PathBuf::from("/стол/а"), false);
        assert_eq!(b.key(KeyPress::plain(Key::Escape)), RenameResult::Cancel);
    }

    #[test]
    fn renders() {
        let mut fonts = Fonts::new("Inter");
        let b = RenameBox::new(PathBuf::from("/стол/Новая папка"), true);
        let (w, h) = b.size(&mut fonts);
        assert!(w >= MIN_W as u32 && h > 10);
        let pm = b.render(&mut fonts);
        // Выделенный текст — на светло-синем.
        let p = pm.pixel(PAD_X as u32 + 2, PAD_Y as u32 + 1).unwrap();
        assert!(p.blue() > 200, "{p:?}");
    }
}
