//! Цвета терминала: тёмная тема (как была в kitty у Mimshel), 256 цветов xterm
//! и цвета, которые программа поменяла сама (OSC 4/10/11).

use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor};

pub type Rgb = [u8; 3];

#[derive(Debug, Clone)]
pub struct Palette {
    pub background: Rgb,
    pub foreground: Rgb,
    pub cursor: Rgb,
    pub selection: Rgb,
    /// 16 основных цветов: 0–7 и яркие 8–15.
    pub ansi: [Rgb; 16],
}

const fn hex(v: u32) -> Rgb {
    [(v >> 16) as u8, (v >> 8) as u8, v as u8]
}

impl Default for Palette {
    fn default() -> Palette {
        Palette {
            background: hex(0x1e1e2e),
            foreground: hex(0xcdd6f4),
            cursor: hex(0xf5e0dc),
            selection: hex(0x45475a),
            // Обычные цвета (0–7) — насыщенные и потемнее: программы красят ими
            // и фон (синие панели mc, полосы htop), и пастельный фон гасил
            // светлые буквы. Яркие (8–15) — пастельные, для текста.
            ansi: [
                hex(0x3b3f51),
                hex(0xc94f6d),
                hex(0x5fa85f),
                hex(0xc9a03f),
                hex(0x2f5fa8),
                hex(0x9d5fae),
                hex(0x3a9ea5),
                hex(0xbac2de),
                hex(0x585b70),
                hex(0xf38ba8),
                hex(0xa6e3a1),
                hex(0xf9e2af),
                hex(0x89b4fa),
                hex(0xf5c2e7),
                hex(0x94e2d5),
                hex(0xf4f6fb),
            ],
        }
    }
}

impl Palette {
    /// Цвет знака или фона: заданный числами, из 256 цветов или по имени.
    /// Программа могла поменять цвет — тогда её (`colors`).
    pub fn resolve(&self, c: Color, colors: &Colors) -> Rgb {
        match c {
            Color::Spec(rgb) => [rgb.r, rgb.g, rgb.b],
            Color::Named(n) => self.indexed(n as usize, colors),
            Color::Indexed(i) => self.indexed(usize::from(i), colors),
        }
    }

    fn indexed(&self, i: usize, colors: &Colors) -> Rgb {
        if let Some(rgb) = colors[i] {
            return [rgb.r, rgb.g, rgb.b];
        }
        let dim = |c: Rgb| c.map(|v| (u16::from(v) * 2 / 3) as u8);
        match i {
            0..=15 => self.ansi[i],
            // Куб 6×6×6.
            16..=231 => {
                let n = i - 16;
                let level = |v: usize| if v == 0 { 0 } else { (55 + v * 40) as u8 };
                [level(n / 36), level(n / 6 % 6), level(n % 6)]
            }
            // Серые.
            232..=255 => {
                let v = (8 + (i - 232) * 10) as u8;
                [v, v, v]
            }
            _ => match i {
                x if x == NamedColor::Foreground as usize => self.foreground,
                x if x == NamedColor::Background as usize => self.background,
                x if x == NamedColor::Cursor as usize => self.cursor,
                x if x == NamedColor::BrightForeground as usize => self.foreground,
                x if x == NamedColor::DimForeground as usize => dim(self.foreground),
                x if (NamedColor::DimBlack as usize..=NamedColor::DimWhite as usize)
                    .contains(&x) =>
                {
                    dim(self.ansi[x - NamedColor::DimBlack as usize])
                }
                _ => self.foreground,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xterm_256_colors() {
        let p = Palette::default();
        let colors = Colors::default();
        assert_eq!(p.resolve(Color::Indexed(1), &colors), p.ansi[1]);
        assert_eq!(p.resolve(Color::Indexed(16), &colors), [0, 0, 0]);
        assert_eq!(p.resolve(Color::Indexed(231), &colors), [255, 255, 255]);
        assert_eq!(p.resolve(Color::Indexed(232), &colors), [8, 8, 8]);
        assert_eq!(
            p.resolve(Color::Named(NamedColor::Background), &colors),
            p.background
        );
    }
}
