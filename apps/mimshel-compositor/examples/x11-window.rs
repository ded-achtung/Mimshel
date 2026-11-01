//! Stationary X11 client for nested smoke tests (`--nested -- …/examples/x11-window`).
use x11rb::{
    connection::Connection,
    protocol::{
        Event,
        xproto::{
            AtomEnum, ConnectionExt, CreateGCAux, CreateWindowAux, EventMask, PropMode, WindowClass,
        },
    },
    wrapper::ConnectionExt as _,
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (connection, screen) = x11rb::connect(None)?;
    let screen = &connection.setup().roots[screen];
    let window = connection.generate_id()?;
    connection.create_window(
        x11rb::COPY_DEPTH_FROM_PARENT,
        window,
        screen.root,
        0,
        0,
        640,
        400,
        0,
        WindowClass::INPUT_OUTPUT,
        0,
        &CreateWindowAux::new()
            .background_pixel(0x173246)
            .event_mask(EventMask::EXPOSURE | EventMask::KEY_PRESS),
    )?;
    connection.change_property8(
        PropMode::REPLACE,
        window,
        AtomEnum::WM_NAME,
        AtomEnum::STRING,
        b"XWayland inside Mimshel",
    )?;
    connection.change_property8(
        PropMode::REPLACE,
        window,
        AtomEnum::WM_CLASS,
        AtomEnum::STRING,
        b"x11-window\0MimshelX11Example\0",
    )?;
    let mut hints = x11rb::properties::WmSizeHints::new();
    hints.min_size = Some((240, 160));
    hints.max_size = Some((900, 600));
    hints.set_normal_hints(&connection, window)?;
    let protocols = connection
        .intern_atom(false, b"WM_PROTOCOLS")?
        .reply()?
        .atom;
    let close = connection
        .intern_atom(false, b"WM_DELETE_WINDOW")?
        .reply()?
        .atom;
    connection.change_property32(
        PropMode::REPLACE,
        window,
        protocols,
        AtomEnum::ATOM,
        &[close],
    )?;
    let gc = connection.generate_id()?;
    connection.create_gc(
        gc,
        window,
        &CreateGCAux::new().foreground(0xe8f2f6).background(0x173246),
    )?;
    connection.map_window(window)?;
    connection.flush()?;
    loop {
        match connection.wait_for_event()? {
            Event::Expose(_) => {
                connection.image_text8(
                    window,
                    gc,
                    28,
                    42,
                    b"X11 client - XWayland - Mimshel compositor",
                )?;
                connection.image_text8(
                    window,
                    gc,
                    28,
                    70,
                    b"Click to focus. Drag the title. Resize the frame.",
                )?;
                connection.image_text8(window, gc, 28, 98, b"Maximum client size: 900 x 600.")?;
                connection.flush()?;
            }
            Event::KeyPress(event) => println!("X11 keycode={}", event.detail),
            Event::ClientMessage(event)
                if event.type_ == protocols && event.data.as_data32()[0] == close =>
            {
                break;
            }
            _ => {}
        }
    }
    Ok(())
}
