//! Проверочный помощник: какие настройки xsettings рассылает служба
//! настроек (xfsettingsd) и с какими значениями. Аргумент — часть имени
//! (`cargo run -p mimshel-x11 --example xsettings -- Menubar`).

use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let filter = std::env::args().nth(1).unwrap_or_default();
    let (conn, screen) = x11rb::connect(None)?;
    let sel = conn
        .intern_atom(false, format!("_XSETTINGS_S{screen}").as_bytes())?
        .reply()?
        .atom;
    let owner = conn.get_selection_owner(sel)?.reply()?.owner;
    let prop = conn
        .intern_atom(false, b"_XSETTINGS_SETTINGS")?
        .reply()?
        .atom;
    let data = conn
        .get_property(false, owner, prop, AtomEnum::ANY, 0, u32::MAX / 4)?
        .reply()?
        .value;
    println!("владелец окна {owner:#x}, {} байт", data.len());
    // Формат: порядок байт, 3 пустых, serial u32, число настроек u32, затем
    // настройки: тип u8, пусто, длина имени u16, имя (с выравниванием до 4),
    // serial u32, значение.
    let le = data.first() == Some(&0);
    let u16_at = |i: usize| {
        let b = [data[i], data[i + 1]];
        if le {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        }
    };
    let u32_at = |i: usize| {
        let b = [data[i], data[i + 1], data[i + 2], data[i + 3]];
        if le {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        }
    };
    let pad = |n: usize| (n + 3) & !3;
    let count = u32_at(8) as usize;
    let mut i = 12;
    for _ in 0..count {
        let kind = data[i];
        let name_len = u16_at(i + 2) as usize;
        let name = String::from_utf8_lossy(&data[i + 4..i + 4 + name_len]).into_owned();
        i += 4 + pad(name_len) + 4;
        let value = match kind {
            0 => {
                let v = u32_at(i) as i32;
                i += 4;
                v.to_string()
            }
            1 => {
                let len = u32_at(i) as usize;
                let s = String::from_utf8_lossy(&data[i + 4..i + 4 + len]).into_owned();
                i += 4 + pad(len);
                format!("«{s}»")
            }
            _ => {
                i += 8;
                "(цвет)".into()
            }
        };
        if name.contains(&filter) {
            println!("{name} = {value}");
        }
    }
    Ok(())
}
