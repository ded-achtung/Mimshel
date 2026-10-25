//! Раскладки клавиатуры через XKB: какие есть, какая сейчас, переключить;
//! события о смене раскладки ([`Event::KeyboardGroup`]) и о смене набора
//! раскладок ([`Event::KeyboardLayouts`]).
//!
//! [`Event::KeyboardGroup`]: crate::Event::KeyboardGroup
//! [`Event::KeyboardLayouts`]: crate::Event::KeyboardLayouts

use x11rb::protocol::xkb::{
    ConnectionExt as _, EventType, Group, ID, MapPart, NKNDetail, NameDetail, SelectEventsAux,
    SelectEventsAuxNamesNotify, SelectEventsAuxNewKeyboardNotify, SelectEventsAuxStateNotify,
    StatePart,
};
use x11rb::protocol::xproto::ConnectionExt as _;

use crate::{Display, Result};

const CORE: u16 = 256;

impl Display {
    /// Получать события о смене раскладки и набора раскладок.
    pub fn watch_keyboard(&self) -> Result<()> {
        debug_assert_eq!(u16::from(ID::USE_CORE_KBD), CORE);
        let names = NameDetail::SYMBOLS | NameDetail::GROUP_NAMES;
        let nkn = NKNDetail::KEYCODES | NKNDetail::DEVICE_ID;
        let details = SelectEventsAux::new()
            .state_notify(SelectEventsAuxStateNotify {
                affect_state: StatePart::GROUP_STATE,
                state_details: StatePart::GROUP_STATE,
            })
            .names_notify(SelectEventsAuxNamesNotify {
                affect_names: names,
                names_details: names,
            })
            // Клавиатура сменилась целиком: нажали на другом устройстве (у
            // виртуальной XTEST своя раскладка) или выполнили setxkbmap.
            .new_keyboard_notify(SelectEventsAuxNewKeyboardNotify {
                affect_new_keyboard: nkn,
                new_keyboard_details: nkn,
            });
        self.conn
            .xkb_select_events(
                CORE,
                EventType::from(0u16),
                EventType::from(0u16),
                MapPart::from(0u16),
                MapPart::from(0u16),
                &details,
            )?
            .check()?;
        Ok(())
    }

    /// Раскладки по порядку групп (`["us", "ru"]`).
    pub fn keyboard_layouts(&self) -> Vec<String> {
        let atom = self
            .conn
            .xkb_get_names(CORE, NameDetail::SYMBOLS)
            .ok()
            .and_then(|c| c.reply().ok())
            .and_then(|r| r.value_list.symbols_name);
        let name = atom
            .and_then(|a| self.conn.get_atom_name(a).ok())
            .and_then(|c| c.reply().ok())
            .map(|r| String::from_utf8_lossy(&r.name).into_owned())
            .unwrap_or_default();
        mimshel_core::keyboard::layouts_from_symbols(&name)
    }

    /// Номер нынешней раскладки (с 0).
    pub fn keyboard_group(&self) -> u8 {
        self.conn
            .xkb_get_state(CORE)
            .ok()
            .and_then(|c| c.reply().ok())
            .map_or(0, |r| u8::from(r.group))
    }

    /// Включить раскладку `group` (как Alt+Shift).
    pub fn set_keyboard_group(&self, group: u8) -> Result<()> {
        self.conn.xkb_latch_lock_state(
            CORE,
            0u8.into(),
            0u8.into(),
            true,
            Group::from(group),
            0u8.into(),
            false,
            0,
        )?;
        self.sync()
    }
}
