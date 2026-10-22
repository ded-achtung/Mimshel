//! Проверочный помощник: занять трей и печатать значки, которые
//! регистрируются (и меню каждого). `cargo run -p shell-dbus --example tray`.
//! Служба shell со строкой меню не должна быть запущена — имя трея одно.

use std::sync::mpsc;

use shell_dbus::tray::{self, TrayCmd, TrayUpdate};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (tx, rx) = mpsc::channel();
    let tray = tray::start(move |u| {
        let _ = tx.send(u);
    })?;
    println!("Трей запущен, жду значки…");
    for update in rx {
        match update {
            TrayUpdate::Items(items) => {
                for i in &items {
                    let pix = i.pixmap.as_ref().map(|(w, h, _)| format!("{w}x{h}"));
                    println!(
                        "значок {}: «{}» иконка «{}» {:?} статус {} меню {} подсказка {:?}",
                        i.key, i.title, i.icon_name, pix, i.status, i.has_menu, i.tooltip
                    );
                    tray.send(TrayCmd::Menu(i.key.clone()));
                }
            }
            TrayUpdate::Menu { key, items } => {
                println!("меню {key}:");
                for m in items {
                    println!(
                        "  {:?}",
                        (
                            m.id,
                            &m.label,
                            m.enabled,
                            m.separator,
                            m.toggle,
                            m.children.len()
                        )
                    );
                }
            }
        }
    }
    Ok(())
}
