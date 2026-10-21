//! Проверочный помощник: раскладка XKB и нажатия клавиш «как с клавиатуры».
//! `cargo run -p shell-x11 --example xkb_group` — показать раскладку;
//! `… -- 1` — включить вторую (у нас ru), `… -- 0` — первую (us);
//! `… -- 1 57 28` — включить раскладку и нажать клавиши с этими кодами.
//!
//! `xdotool` для такой проверки не годится: для букв не из текущей раскладки
//! он подменяет таблицу клавиш, а не меняет раскладку, как человек.

use x11rb::connection::Connection;
use x11rb::protocol::xkb::{ConnectionExt as _, Group, ID};
use x11rb::protocol::xproto::{KEY_PRESS_EVENT, KEY_RELEASE_EVENT, ModMask};
use x11rb::protocol::xtest::ConnectionExt as _;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (conn, screen) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen].root;
    conn.xkb_use_extension(1, 0)?.reply()?;
    let kbd = ID::USE_CORE_KBD.into();
    let mut args = std::env::args().skip(1);
    if let Some(n) = args.next() {
        let group = Group::from(n.parse::<u8>()?);
        let none = ModMask::from(0u16);
        conn.xkb_latch_lock_state(kbd, none, none, true, group, none, false, 0)?;
        conn.flush()?;
    }
    for code in args {
        let code: u8 = code.parse()?;
        for kind in [KEY_PRESS_EVENT, KEY_RELEASE_EVENT] {
            conn.xtest_fake_input(kind, code, 0, root, 0, 0, 0)?;
        }
        conn.flush()?;
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let state = conn.xkb_get_state(kbd)?.reply()?;
    println!("раскладка: {:?}", state.group);
    Ok(())
}
