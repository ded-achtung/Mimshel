//! Проверочный помощник: окна программ, как их видит док.
//! `cargo run -p mimshel-x11 --example windows`

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let d = mimshel_x11::Display::connect()?;
    let active = d.active_window();
    for w in d.client_list() {
        match d.window_info(w) {
            Some(i) => println!(
                "{w:#x}{} {:?}/{:?} pid {:?} свёрнуто {} развёрнуто {} «{}» иконка {} рамка {:?}",
                if Some(w) == active {
                    " (активно)"
                } else {
                    ""
                },
                i.instance,
                i.class,
                i.pid,
                i.minimized,
                i.maximized,
                i.title,
                d.window_icon(w, 48).is_some(),
                d.frame_geometry(w)
            ),
            None => println!("{w:#x} — не для дока"),
        }
    }
    Ok(())
}
