//! Проверочный помощник: напечатать глобальное меню окна.
//! GTK: `cargo run -p shell-dbus --example appmenu -- 0xОКНО ШИНА МЕНЮ [app] [win] [unity]`
//! (значения — из `xprop -id 0xОКНО | grep _GTK_`); через регистратор: `… -- 0xОКНО`.

use std::sync::mpsc;

use shell_dbus::MenuItem;
use shell_dbus::appmenu::{self, AppMenuCmd, AppMenuUpdate, GtkPaths, Source};

fn print(items: &[MenuItem], depth: usize) {
    for i in items {
        let mark = match i.toggle {
            Some(true) => "✓ ",
            Some(false) => "  ",
            None => "",
        };
        let off = if i.enabled {
            ""
        } else {
            " (неактивен)"
        };
        if i.separator {
            println!("{}——", "  ".repeat(depth));
        } else {
            println!("{}{mark}{}{off} [{}]", "  ".repeat(depth), i.label, i.id);
        }
        print(&i.children, depth + 1);
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let window = u32::from_str_radix(
        args.first()
            .ok_or("нужен номер окна")?
            .trim_start_matches("0x"),
        16,
    )?;
    let opt = |i: usize| args.get(i).filter(|s| !s.is_empty() && *s != "-").cloned();
    let source = match (opt(1), opt(2)) {
        (Some(bus), Some(menubar)) => Source::Gtk(GtkPaths {
            bus,
            menubar,
            app: opt(3),
            window: opt(4),
            unity: opt(5),
        }),
        _ => Source::Registrar,
    };
    let (tx, rx) = mpsc::channel();
    let menu = appmenu::start(move |u| {
        let _ = tx.send(u);
    })?;
    menu.send(AppMenuCmd::Load {
        window,
        source,
        open: Some(0),
    });
    let AppMenuUpdate::Menu { menus, .. } = rx.recv()?;
    if menus.is_empty() {
        println!("меню нет");
    }
    for (title, items) in &menus {
        println!("== {title}");
        print(items, 1);
    }
    Ok(())
}
