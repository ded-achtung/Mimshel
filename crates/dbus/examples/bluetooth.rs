//! Проверка Bluetooth без службы: печатает адаптер и устройства, ищет 10 с.
//! `cargo run -p mimshel-dbus --example bluetooth`
use mimshel_dbus::bluetooth::{self, BtCmd};

fn main() -> zbus::Result<()> {
    let bt = bluetooth::start(|state| match state {
        None => println!("Адаптера или службы bluetooth нет"),
        Some(s) => {
            println!(
                "Включён: {}, поиск: {}, устройств: {}",
                s.powered,
                s.discovering,
                s.devices.len()
            );
            for d in &s.devices {
                println!(
                    "  {} [{}] сопряжено: {}, подключено: {}",
                    d.name, d.icon, d.paired, d.connected
                );
            }
        }
    })?;
    bt.send(BtCmd::Scan(true));
    std::thread::sleep(std::time::Duration::from_secs(10));
    bt.send(BtCmd::Scan(false));
    std::thread::sleep(std::time::Duration::from_millis(500));
    Ok(())
}
