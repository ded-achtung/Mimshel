//! Служба уведомлений по стандарту freedesktop (`org.freedesktop.Notifications`,
//! спецификация 1.2): программы присылают уведомление (`Notify`), мы
//! показываем его баннером и сообщаем, если его закрыли или выбрали действие.
//!
//! Наружу — [`NotifyEvent`] через `notify`; внутрь — [`NotifyServer::closed`]
//! и [`NotifyServer::action`] (служба шлёт программе сигналы).

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use zbus::blocking::Connection;
use zbus::interface;
use zbus::zvariant::{OwnedValue, Value};

const NAME: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";

/// Bytes kept for `app_name`, `app_icon`, `desktop_entry` and the
/// `image-path` hint: long strings from programs would pile up in history.
const MAX_NAME: usize = 256;
/// Bytes kept for the summary and the body.
const MAX_SUMMARY: usize = 1024;
const MAX_BODY: usize = 8192;
/// Action key–label pairs kept (`take(MAX_ACTIONS)`).
const MAX_ACTIONS: usize = 8;
/// Images are drawn at a 32-point icon (96 px even at 3x scale), so a
/// 256-px cap leaves generous headroom while keeping every stored copy small.
const MAX_IMAGE_SIDE: u32 = 256;
/// More notifications than this within `RATE_WINDOW`: the extra ones are
/// dropped (an id is still returned, per the spec).
const RATE_LIMIT: usize = 20;
const RATE_WINDOW: Duration = Duration::from_secs(1);

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
    /// Rate limiter for incoming notifications.
    rate: Mutex<RateLimit>,
}

/// Lets through at most `limit` notifications per `window`; the rest are
/// dropped (a flood must not grind the daemon down).
struct RateLimit {
    /// Arrival times of the let-through ones, oldest first.
    times: VecDeque<Instant>,
    limit: usize,
    window: Duration,
}

impl RateLimit {
    fn new(limit: usize, window: Duration) -> Self {
        RateLimit {
            times: VecDeque::new(),
            limit,
            window,
        }
    }

    /// `true` — show this one; `false` — drop it (an id is still returned).
    fn allow(&mut self, now: Instant) -> bool {
        while self
            .times
            .front()
            .is_some_and(|t| now.duration_since(*t) >= self.window)
        {
            self.times.pop_front();
        }
        if self.times.len() >= self.limit {
            return false;
        }
        self.times.push_back(now);
        true
    }
}

/// `s` cut to at most `max` bytes, on a char boundary.
fn truncate(mut s: String, max: usize) -> String {
    if s.len() > max {
        let mut end = max;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
    }
    s
}

/// Smaller copy of `img` with the longer side at most `max` pixels (box
/// average of the covered pixels, like the dock previews, with the colour
/// weighted by alpha — the data is not premultiplied, so plain averaging
/// would darken transparent edges); small enough images are kept as they are.
fn downscale(img: Image, max: u32) -> Image {
    let (w, h) = (img.width, img.height);
    let big = w.max(h);
    if big <= max {
        return img;
    }
    let ow = ((w * max + big / 2) / big).max(1);
    let oh = ((h * max + big / 2) / big).max(1);
    let mut rgba = vec![0u8; (ow * oh * 4) as usize];
    for oy in 0..oh {
        let y0 = oy * h / oh;
        let y1 = (((oy + 1) * h / oh).max(y0 + 1)).min(h);
        for ox in 0..ow {
            let x0 = ox * w / ow;
            let x1 = (((ox + 1) * w / ow).max(x0 + 1)).min(w);
            let mut sum = [0u64; 4];
            let mut n = 0u64;
            for sy in y0..y1 {
                let row = &img.rgba[(sy * w + x0) as usize * 4..(sy * w + x1) as usize * 4];
                for px in row.as_chunks::<4>().0 {
                    let a = u64::from(px[3]);
                    for (c, v) in px.iter().enumerate().take(3) {
                        sum[c] += u64::from(*v) * a;
                    }
                    sum[3] += a;
                    n += 1;
                }
            }
            let out = &mut rgba[(oy * ow + ox) as usize * 4..][..4];
            for (c, v) in out.iter_mut().enumerate().take(3) {
                *v = sum[c].checked_div(sum[3]).unwrap_or(0) as u8;
            }
            out[3] = (sum[3] / n) as u8;
        }
    }
    Image {
        width: ow,
        height: oh,
        rgba,
    }
}

impl Server {
    /// The notification to show, or `None` if this one is dropped by the rate
    /// limit. Strings and images are cut to sane sizes so history stays small.
    #[allow(clippy::too_many_arguments)]
    fn build(
        &self,
        app_name: String,
        app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
        expire_timeout: i32,
        now: Instant,
    ) -> Option<Notification> {
        if !self
            .rate
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .allow(now)
        {
            return None;
        }
        let hint = |k: &str| hints.get(k).and_then(|v| v.try_clone().ok());
        let text = |k: &str| hint(k).and_then(|v| String::try_from(v).ok());
        // An image sent as data: bounded to 2048x2048 while parsing, then
        // shrunk to what the UI actually draws.
        let image = ["image-data", "image_data", "icon_data"]
            .iter()
            .find_map(|k| hint(k).and_then(image_from))
            .map(|img| downscale(img, MAX_IMAGE_SIDE));
        // An image sent as a path is used like the icon when there is none.
        let app_icon = if app_icon.is_empty() {
            text("image-path")
                .or_else(|| text("image_path"))
                .unwrap_or_default()
        } else {
            app_icon
        };
        Some(Notification {
            // The caller sets the real id (replaces or the next one).
            id: 0,
            app_name: truncate(app_name, MAX_NAME),
            app_icon: truncate(app_icon, MAX_NAME),
            // Cut the raw text before stripping: a client may send megabytes,
            // and markup only shrinks the text, so a 4x headroom is plenty
            // for the final exact trim below.
            summary: truncate(
                strip_markup(&truncate(summary, MAX_SUMMARY * 4)),
                MAX_SUMMARY,
            ),
            body: truncate(strip_markup(&truncate(body, MAX_BODY * 4)), MAX_BODY),
            actions: actions
                .chunks(2)
                .filter_map(|c| Some((c.first()?.clone(), c.get(1)?.clone())))
                .take(MAX_ACTIONS)
                .collect(),
            image,
            desktop_entry: text("desktop-entry").map(|e| truncate(e, MAX_NAME)),
            urgency: hint("urgency")
                .and_then(|v| u8::try_from(v).ok())
                .unwrap_or(1),
            expire_timeout,
            transient: hint("transient")
                .and_then(|v| bool::try_from(v).ok())
                .unwrap_or(false),
        })
    }
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
        if let Some(n) = self.build(
            app_name,
            app_icon,
            summary,
            body,
            actions,
            hints,
            expire_timeout,
            Instant::now(),
        ) {
            let n = Notification { id, ..n };
            (self.notify.lock().unwrap_or_else(|p| p.into_inner()))(NotifyEvent::Show(n));
        }
        id
    }

    fn close_notification(&self, id: u32) {
        (self.notify.lock().unwrap_or_else(|p| p.into_inner()))(NotifyEvent::Close(id));
    }

    fn get_server_information(&self) -> (&'static str, &'static str, &'static str, &'static str) {
        ("Mimshel", "Mimshel", env!("CARGO_PKG_VERSION"), "1.2")
    }
}

/// Картинка из подсказки `image-data`: `(iiibiiay)` — ширина, высота, шаг
/// строки, есть ли прозрачность, бит на канал, каналов, байты.
fn image_from(v: OwnedValue) -> Option<Image> {
    let (w, h, stride, alpha, bits, channels, data) =
        <(i32, i32, i32, bool, i32, i32, Vec<u8>)>::try_from(Value::from(v)).ok()?;
    // Уведомления приходят от других процессов: проверяем размеры до
    // выделения памяти, включая последний ряд без завершающего отступа.
    if !(1..=2048).contains(&w)
        || !(1..=2048).contains(&h)
        || stride <= 0
        || bits != 8
        || channels != if alpha { 4 } else { 3 }
    {
        return None;
    }
    let (w, h, stride, ch) = (w as usize, h as usize, stride as usize, channels as usize);
    let row_bytes = w.checked_mul(ch)?;
    let required = (h - 1).checked_mul(stride)?.checked_add(row_bytes)?;
    if stride < row_bytes || data.len() < required {
        return None;
    }
    let len = w.checked_mul(h)?.checked_mul(4)?;
    let mut rgba = Vec::new();
    rgba.try_reserve_exact(len).ok()?;
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
    let conn = crate::session()?;
    conn.object_server().at(
        PATH,
        Server {
            next_id: Arc::new(AtomicU32::new(1)),
            notify: Arc::new(Mutex::new(Box::new(notify))),
            rate: Mutex::new(RateLimit::new(RATE_LIMIT, RATE_WINDOW)),
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

    fn image(
        w: i32,
        h: i32,
        stride: i32,
        alpha: bool,
        channels: i32,
        data: Vec<u8>,
    ) -> Option<Image> {
        let v: Value = StructureBuilder::new()
            .add_field(w)
            .add_field(h)
            .add_field(stride)
            .add_field(alpha)
            .add_field(8i32)
            .add_field(channels)
            .add_field(data)
            .build()
            .unwrap()
            .into();
        image_from(OwnedValue::try_from(v).unwrap())
    }

    #[test]
    fn rejects_invalid_image_before_allocation() {
        for (w, h, stride) in [
            (i32::MAX, i32::MAX, i32::MAX),
            (0, 1, 3),
            (1, -1, 3),
            (1, 1, -1),
            (2, 1, 3),
            (1, 2, i32::MAX),
            (1, 2, 3),
        ] {
            assert!(image(w, h, stride, false, 3, vec![0; 3]).is_none());
        }
        assert!(image(1, 1, 4, true, 3, vec![0; 4]).is_none());
        assert!(image(1, 1, 4, false, 4, vec![0; 4]).is_none());
    }

    #[test]
    fn accepts_padding_and_rgba() {
        let img = image(1, 2, 4, false, 3, vec![1, 2, 3, 99, 4, 5, 6]).unwrap();
        assert_eq!(img.rgba, [1, 2, 3, 255, 4, 5, 6, 255]);
        assert_eq!(
            image(1, 1, 4, true, 4, vec![1, 2, 3, 4]).unwrap().rgba,
            [1, 2, 3, 4]
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

    /// A server whose notifications a test can inspect.
    fn test_server() -> (Server, Arc<Mutex<Vec<Notification>>>) {
        let got: Arc<Mutex<Vec<Notification>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&got);
        let server = Server {
            next_id: Arc::new(AtomicU32::new(1)),
            notify: Arc::new(Mutex::new(Box::new(move |e| {
                if let NotifyEvent::Show(n) = e {
                    sink.lock().unwrap_or_else(|p| p.into_inner()).push(n);
                }
            }))),
            rate: Mutex::new(RateLimit::new(RATE_LIMIT, RATE_WINDOW)),
        };
        (server, got)
    }

    fn text_hint(hints: &mut HashMap<String, OwnedValue>, key: &str, value: &str) {
        hints.insert(
            key.to_string(),
            OwnedValue::try_from(Value::from(value.to_string())).unwrap(),
        );
    }

    fn image_hint(w: i32, h: i32, data: Vec<u8>) -> OwnedValue {
        let v: Value = StructureBuilder::new()
            .add_field(w)
            .add_field(h)
            .add_field(w * 4)
            .add_field(true)
            .add_field(8i32)
            .add_field(4i32)
            .add_field(data)
            .build()
            .unwrap()
            .into();
        OwnedValue::try_from(v).unwrap()
    }

    #[test]
    fn truncates_long_fields_on_char_boundaries() {
        let (server, _got) = test_server();
        let mut hints = HashMap::new();
        text_hint(&mut hints, "desktop-entry", &"d".repeat(300));
        // With an empty icon the image-path hint becomes the icon.
        text_hint(&mut hints, "image-path", &"p".repeat(300));
        let n = server
            .build(
                "у".repeat(300), // 2 bytes per char
                String::new(),
                "s".repeat(2000),
                "b".repeat(9000),
                Vec::new(),
                hints,
                -1,
                Instant::now(),
            )
            .unwrap();
        assert_eq!(n.app_name, "у".repeat(MAX_NAME / 2));
        assert_eq!(n.app_name.len(), MAX_NAME);
        assert_eq!(n.app_icon, "p".repeat(MAX_NAME));
        assert_eq!(n.summary.len(), MAX_SUMMARY);
        assert_eq!(n.body.len(), MAX_BODY);
        assert_eq!(n.desktop_entry, Some("d".repeat(MAX_NAME)));
        // Short strings pass through.
        assert_eq!(truncate("short".into(), MAX_NAME), "short");
    }

    #[test]
    fn caps_actions_at_eight_pairs() {
        let (server, _got) = test_server();
        let actions: Vec<String> = (0..20)
            .flat_map(|i| [format!("k{i}"), format!("v{i}")])
            .collect();
        let n = server
            .build(
                "a".into(),
                String::new(),
                "s".into(),
                String::new(),
                actions,
                HashMap::new(),
                -1,
                Instant::now(),
            )
            .unwrap();
        assert_eq!(n.actions.len(), MAX_ACTIONS);
        assert_eq!(n.actions[0], ("k0".to_string(), "v0".to_string()));
        assert_eq!(
            n.actions[MAX_ACTIONS - 1],
            ("k7".to_string(), "v7".to_string())
        );
    }

    #[test]
    fn downscale_averages_pixels() {
        // Black and white columns average to grey.
        let img = Image {
            width: 2,
            height: 1,
            rgba: vec![0, 0, 0, 255, 255, 255, 255, 255],
        };
        let out = downscale(img, 1);
        assert_eq!((out.width, out.height), (1, 1));
        assert_eq!(out.rgba, vec![127, 127, 127, 255]);
        // Transparent black next to opaque white: the colour is weighted by
        // alpha, so the result stays white at half alpha — not grey.
        let img = Image {
            width: 2,
            height: 1,
            rgba: vec![0, 0, 0, 0, 255, 255, 255, 255],
        };
        let out = downscale(img, 1);
        assert_eq!(out.rgba, vec![255, 255, 255, 127]);
        // A fully transparent block keeps alpha 0 (no division by zero).
        let img = Image {
            width: 2,
            height: 2,
            rgba: [9, 9, 9, 0].repeat(4),
        };
        let out = downscale(img, 1);
        assert_eq!(out.rgba, vec![0, 0, 0, 0]);
        // Small enough images are kept as they are.
        let img = Image {
            width: 100,
            height: 50,
            rgba: vec![7; 100 * 50 * 4],
        };
        let out = downscale(img, MAX_IMAGE_SIDE);
        assert_eq!((out.width, out.height), (100, 50));
        assert_eq!(out.rgba, vec![7; 100 * 50 * 4]);
    }

    #[test]
    fn downscales_incoming_images_to_the_drawn_size() {
        let (server, _got) = test_server();
        // Solid red 512x256 -> 256x128, colour kept.
        let red: Vec<u8> = [255, 0, 0, 255].repeat(512 * 256);
        let mut hints = HashMap::new();
        hints.insert("image-data".to_string(), image_hint(512, 256, red));
        let n = server
            .build(
                "a".into(),
                String::new(),
                "s".into(),
                String::new(),
                Vec::new(),
                hints,
                -1,
                Instant::now(),
            )
            .unwrap();
        let img = n.image.unwrap();
        assert_eq!((img.width, img.height), (256, 128));
        assert!(
            img.rgba
                .as_chunks::<4>()
                .0
                .iter()
                .all(|px| px == &[255, 0, 0, 255])
        );
        assert!(img.rgba.len() <= (MAX_IMAGE_SIDE * MAX_IMAGE_SIDE * 4) as usize);
    }

    #[test]
    fn rate_limit_drops_excess_but_returns_ids() {
        let (server, got) = test_server();
        for i in 0..(RATE_LIMIT + 5) {
            let id = server.notify(
                format!("app{i}"),
                0,
                String::new(),
                "s".into(),
                String::new(),
                Vec::new(),
                HashMap::new(),
                -1,
            );
            assert_eq!(id, i as u32 + 1, "an id even for dropped ones");
        }
        assert_eq!(
            got.lock().unwrap_or_else(|p| p.into_inner()).len(),
            RATE_LIMIT
        );
    }

    #[test]
    fn rate_limit_window_slides() {
        let mut rate = RateLimit::new(3, RATE_WINDOW);
        let t0 = Instant::now();
        for i in 0..3 {
            assert!(rate.allow(t0 + Duration::from_millis(100 * i)), "{i}");
        }
        assert!(!rate.allow(t0 + Duration::from_millis(399)));
        // A second after the first arrival it expires — room again.
        assert!(rate.allow(t0 + RATE_WINDOW));
        assert!(!rate.allow(t0 + RATE_WINDOW));
    }
}
