//! Проверочный помощник: какие события выбраны у окна и кто его родитель.
//! `cargo run -p mimshel-x11 --example attrs -- 0x1200001`

use x11rb::connection::Connection;
use x11rb::protocol::xproto::ConnectionExt;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arg = std::env::args().nth(1).ok_or("нужен номер окна")?;
    let w = u32::from_str_radix(arg.trim_start_matches("0x"), 16)?;
    let (conn, _) = x11rb::connect(None)?;
    let a = conn.get_window_attributes(w)?.reply()?;
    println!(
        "своя маска {:?}\nвсе маски {:?}\nне передавать {:?}\nmap {:?} override {}",
        a.your_event_mask,
        a.all_event_masks,
        a.do_not_propagate_mask,
        a.map_state,
        a.override_redirect
    );
    let t = conn.query_tree(w)?.reply()?;
    println!("родитель {:#x}, корень {:#x}", t.parent, t.root);
    let _ = conn.setup();
    Ok(())
}
