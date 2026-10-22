//! Служба уведомлений по стандарту freedesktop (`org.freedesktop.Notifications`,
//! спецификация 1.2): программы присылают уведомление (`Notify`), мы
//! показываем его баннером и сообщаем, если его закрыли или выбрали действие.
//!
//! Наружу — [`NotifyEvent`] через `notify`; внутрь — [`NotifyServer::closed`]
//! и [`NotifyServer::action`] (служба шлёт программе сигналы).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use zbus::blocking::Connection;
use zbus::interface;
use zbus::zvariant::{OwnedValue, Value};

const NAME: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";

/// Картинка уведомления, присланная данными: ширина, высота, RGBA без
/// умножения на прозрачность.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub id: u32,
    pub app_name: String,
    /// Иконка: имя в теме или путь к файлу (может быть пустой).
    pub app_icon: String,
    pub summary: String,
    /// Текст уже без разметки.
    pub body: String,
    /// Действия: (ключ, подпись). Ключ `default` — щелчок по уведомлению.
    pub actions: Vec<(String, String)>,
    pub image: Option<Image>,
    /// Ярлык программы (`firefox` без `.desktop`), если она его назвала.
    pub desktop_entry: Option<String>,
    /// 0 — низкая, 1 — обычная, 2 — срочная (не исчезает сама).
    pub urgency: u8,
    /// Сколько показывать, мс: -1 — на наше усмотрение, 0 — пока не закроют.
    pub expire_timeout: i32,
    /// Не сохранять в истории.
    pub transient: bool,
}

/// Что сообщает служба.
#[derive(Debug, Clone)]
pub enum NotifyEvent {
    /// Новое уведомление (или замена прежнего с тем же id).
    Show(Notification),
    /// Программа сама закрыла уведомление.
    Close(u32),
}

/// Почему уведомление закрыто (числа — из спецификации).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    Expired = 1,
    Dismissed = 2,
    Closed = 3,
}

pub struct NotifyServer {
    conn: Connection,
}

impl NotifyServer {
    /// Уведомление закрыто — сообщить программе.
    pub fn closed(&self, id: u32, reason: CloseReason) {
        let _ = self.conn.emit_signal(
            None::<&str>,
            PATH,
            NAME,
            "NotificationClosed",
            &(id, reason as u32),
        );
    }

    /// Выбрано действие уведомления (`default` — щелчок по нему).
    pub fn action(&self, id: u32, key: &str) {
        let _ = self
            .conn
            .emit_signal(None::<&str>, PATH, NAME, "ActionInvoked", &(id, key));
    }
}

/// Куда отдавать события службы.
type Sink = Arc<Mutex<Box<dyn Fn(NotifyEvent) + Send>>>;

struct Server {
    next_id: Arc<AtomicU32>,
    notify: Sink,
}

#[interface(name = "org.freedesktop.Notifications")]
impl Server {
    fn get_capabilities(&self) -> Vec<&'static str> {
        vec![
            "body",
            "body-markup",
            "actions",
            "icon-static",
            "persistence",
        ]
    }

    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: String,
        replaces_id: u32,
        app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
        expire_timeout: i32,
    ) -> u32 {
        let id = if replaces_id != 0 {
            replaces_id
        } else {
            self.next_id.fetch_add(1, Ordering::Relaxed)
        };
        let hint = |k: &str| hints.get(k).and_then(|v| v.try_clone().ok());
        let text = |k: &str| hint(k).and_then(|v| String::try_from(v).ok());
        let image = ["image-data", "image_data", "icon_data"]
            .iter()
            .find_map(|k| hint(k).and_then(image_from));
        // Картинка файлом — как иконка, если своей иконки нет.
        let app_icon = if app_icon.is_empty() {
            text("image-path")
                .or_else(|| text("image_path"))
                .unwrap_or_default()
        } else {
            app_icon
        };
        let n = Notification {
            id,
            app_name,
            app_icon,
            summary: strip_markup(&summary),
            body: strip_markup(&body),
            actions: actions
                .chunks(2)
                .filter_map(|c| Some((c.first()?.clone(), c.get(1)?.clone())))
                .collect(),
            image,
            desktop_entry: text("desktop-entry"),
            urgency: hint("urgency")
                .and_then(|v| u8::try_from(v).ok())
                .unwrap_or(1),
            expire_timeout,
            transient: hint("transient")
                .and_then(|v| bool::try_from(v).ok())
                .unwrap_or(false),
        };
        (self.notify.lock().unwrap_or_else(|p| p.into_inner()))(NotifyEvent::Show(n));
        id
    }

    fn close_notification(&self, id: u32) {
        (self.notify.lock().unwrap_or_else(|p| p.into_inner()))(NotifyEvent::Close(id));
    }

    fn get_server_information(&self) -> (&'static str, &'static str, &'static str, &'static str) {
        ("shell", "shell", env!("CARGO_PKG_VERSION"), "1.2")
    }
}

/// Картинка из подсказки `image-data`: `(iiibiiay)` — ширина, высота, шаг
/// строки, есть ли прозрачность, бит на канал, каналов, байты.
fn image_from(v: OwnedValue) -> Option<Image> {
    let (w, h, stride, alpha, bits, channels, data) =
        <(i32, i32, i32, bool, i32, i32, Vec<u8>)>::try_from(Value::from(v)).ok()?;
    if w <= 0 || h <= 0 || bits != 8 || !(3..=4).contains(&channels) {
        return None;
    }
    let (w, h, stride, ch) = (w as usize, h as usize, stride as usize, channels as usize);
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let row = data.get(y * stride..y * stride + w * ch)?;
        for px in row.chunks(ch) {
            let a = if alpha && ch == 4 { px[3] } else { 255 };
            rgba.extend_from_slice(&[px[0], px[1], px[2], a]);
        }
    }
    Some(Image {
        width: w as u32,
        height: h as u32,
        rgba,
    })
}

/// Текст без разметки: `<b>`, `<i>`, `<a href…>` убираются, `&amp;` и
/// прочие — заменяются знаками.
pub fn strip_markup(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

/// Запустить службу уведомлений. Имя занято (xfce4-notifyd) — ждём в
/// очереди: оно перейдёт к нам, когда тот уйдёт.
pub fn start(notify: impl Fn(NotifyEvent) + Send + 'static) -> zbus::Result<NotifyServer> {
    let conn = Connection::session()?;
    conn.object_server().at(
        PATH,
        Server {
            next_id: Arc::new(AtomicU32::new(1)),
            notify: Arc::new(Mutex::new(Box::new(notify))),
        },
    )?;
    let reply =
        conn.request_name_with_flags(NAME, zbus::fdo::RequestNameFlags::ReplaceExisting.into())?;
    eprintln!("Уведомления: {reply:?}");
    Ok(NotifyServer { conn })
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::StructureBuilder;

    #[test]
    fn markup_is_stripped() {
        assert_eq!(
            strip_markup("<b>Готово</b>: файл &lt;a&gt; &amp; <a href=\"x\">ссылка</a>"),
            "Готово: файл <a> & ссылка"
        );
    }

    #[test]
    fn image_data_to_rgba() {
        // 2×1, RGB без прозрачности, шаг строки с запасом.
        let v: Value = StructureBuilder::new()
            .add_field(2i32)
            .add_field(1i32)
            .add_field(8i32)
            .add_field(false)
            .add_field(8i32)
            .add_field(3i32)
            .add_field(vec![255u8, 0, 0, 0, 255, 0, 9, 9])
            .build()
            .unwrap()
            .into();
        let img = image_from(OwnedValue::try_from(v).unwrap()).unwrap();
        assert_eq!((img.width, img.height), (2, 1));
        assert_eq!(img.rgba, [255, 0, 0, 255, 0, 255, 0, 255]);
    }
}
