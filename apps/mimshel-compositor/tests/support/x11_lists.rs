//! Observe both foreign-toplevel protocols from a real Wayland client.
use wayland_client::{Connection, Dispatch, QueueHandle, protocol::wl_registry};
use wayland_protocols::ext::foreign_toplevel_list::v1::client::{
    ext_foreign_toplevel_handle_v1 as ext_handle, ext_foreign_toplevel_list_v1 as ext_list,
};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1 as wlr_handle, zwlr_foreign_toplevel_manager_v1 as wlr_list,
};
#[derive(Default)]
struct Client {
    titles: [Vec<String>; 2],
    apps: [Vec<String>; 2],
}
impl Dispatch<wl_registry::WlRegistry, ()> for Client {
    fn event(
        _: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name, interface, ..
        } = event
        {
            match interface.as_str() {
                "ext_foreign_toplevel_list_v1" => {
                    registry.bind::<ext_list::ExtForeignToplevelListV1, _, _>(name, 1, qh, ());
                }
                "zwlr_foreign_toplevel_manager_v1" => {
                    registry.bind::<wlr_list::ZwlrForeignToplevelManagerV1, _, _>(name, 3, qh, ());
                }
                _ => {}
            }
        }
    }
}
macro_rules! observer {
    ($list:ty, $list_event:ty, $handle:ty, $event:ident, $index:expr) => {
        impl Dispatch<$list, ()> for Client {
            fn event(_: &mut Self, _: &$list, _: $list_event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
            wayland_client::event_created_child!(Client, $list, [0 => ($handle, ())]);
        }
        impl Dispatch<$handle, ()> for Client {
            fn event(state: &mut Self, _: &$handle, event: $event::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
                match event {
                    $event::Event::Title { title } => state.titles[$index].push(title),
                    $event::Event::AppId { app_id } => state.apps[$index].push(app_id),
                    _ => {}
                }
            }
        }
    };
}
observer!(
    ext_list::ExtForeignToplevelListV1,
    ext_list::Event,
    ext_handle::ExtForeignToplevelHandleV1,
    ext_handle,
    0
);
observer!(
    wlr_list::ZwlrForeignToplevelManagerV1,
    wlr_list::Event,
    wlr_handle::ZwlrForeignToplevelHandleV1,
    wlr_handle,
    1
);

pub fn check(path: std::path::PathBuf, title: &str) {
    let connection =
        Connection::from_socket(std::os::unix::net::UnixStream::connect(path).unwrap()).unwrap();
    let mut events = connection.new_event_queue::<Client>();
    connection.display().get_registry(&events.handle(), ());
    let mut client = Client::default();
    for _ in 0..3 {
        events.roundtrip(&mut client).unwrap();
    }
    for index in 0..2 {
        assert_eq!(client.titles[index].len(), 1);
        assert!(client.titles[index].iter().any(|s| s == title));
        assert!(client.apps[index].iter().any(|s| s == "XWaylandTest"));
    }
}
