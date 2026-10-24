//! Иконки по стандарту freedesktop «Icon Theme»: поиск файла по имени и
//! размеру с учётом наследования тем (`Inherits=`), затем `hicolor`, затем
//! `/usr/share/pixmaps`. Найденная картинка (SVG или PNG) рисуется в нужный
//! размер и запоминается.
//!
//! В WhiteSur одно имя может означать разные картинки в разных папках
//! (`org.xfce.thunar`: Finder в `apps/`, папка в `preferences/`), поэтому при
//! равных условиях папки `apps/…` идут первыми — нам нужны значки программ.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use tiny_skia::{FilterQuality, Pixmap, PixmapPaint, Transform};

/// Как папка темы относится к размерам (поле `Type=`).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Fixed,
    Scalable { min: u32, max: u32 },
    Threshold(u32),
}

#[derive(Debug, Clone)]
struct Dir {
    path: String,
    size: u32,
    scale: u32,
    kind: Kind,
}

impl Dir {
    /// Подходит ли папка под размер точно (алгоритм из стандарта).
    fn matches(&self, size: u32, scale: u32) -> bool {
        if self.scale != scale {
            return false;
        }
        match self.kind {
            Kind::Fixed => self.size == size,
            Kind::Scalable { min, max } => (min..=max).contains(&size),
            Kind::Threshold(t) => self.size.abs_diff(size) <= t,
        }
    }

    /// Насколько папка далека от нужного размера (0 — подходит).
    fn distance(&self, size: u32, scale: u32) -> u32 {
        let want = size * scale;
        let (lo, hi) = match self.kind {
            Kind::Fixed => (self.size, self.size),
            Kind::Scalable { min, max } => (min, max),
            Kind::Threshold(t) => (self.size.saturating_sub(t), self.size + t),
        };
        if want < lo * self.scale {
            lo * self.scale - want
        } else {
            want.saturating_sub(hi * self.scale)
        }
    }
}

#[derive(Debug)]
struct Theme {
    name: String,
    dirs: Vec<Dir>,
}

pub struct Icons {
    /// Тема и все её родители по порядку, в конце — hicolor.
    themes: Vec<Theme>,
    /// Где лежат темы: `~/.icons`, `~/.local/share/icons`, `/usr/share/icons`…
    bases: Vec<PathBuf>,
    /// Где искать иконки вне тем (`/usr/share/pixmaps`).
    pixmaps: Vec<PathBuf>,
    cache: HashMap<(String, u32), Option<Pixmap>>,
    /// Есть ли папка темы (проверяется один раз): в hicolor 649 папок, и
    /// почти все они есть не в каждом месте поиска — без памяти о них поиск
    /// недостающего значка спрашивал диск о тысячах файлов.
    folders: RefCell<HashMap<PathBuf, bool>>,
    /// Масштаб экрана: значок в `size` точек рисуется в `size × scale` пикселей.
    scale: f32,
}

impl Icons {
    /// Тема с данным именем; папки — из переменных окружения сеанса.
    pub fn new(theme: &str) -> Icons {
        let var = |name| std::env::var(name).ok().filter(|v: &String| !v.is_empty());
        let home = var("HOME").map(PathBuf::from);
        let mut bases = Vec::new();
        if let Some(home) = &home {
            bases.push(home.join(".icons"));
        }
        match var("XDG_DATA_HOME") {
            Some(d) => bases.push(Path::new(&d).join("icons")),
            None => bases.extend(home.map(|h| h.join(".local/share/icons"))),
        }
        let data = var("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into());
        let data: Vec<PathBuf> = data
            .split(':')
            .filter(|d| !d.is_empty())
            .map(PathBuf::from)
            .collect();
        bases.extend(data.iter().map(|d| d.join("icons")));
        let pixmaps = data.iter().map(|d| d.join("pixmaps")).collect();
        Icons::with_dirs(theme, bases, pixmaps)
    }

    /// То же, но папки заданы явно (для проверок).
    pub fn with_dirs(theme: &str, bases: Vec<PathBuf>, pixmaps: Vec<PathBuf>) -> Icons {
        let mut themes = Vec::new();
        let mut seen = HashSet::new();
        let mut queue = vec![theme.to_string()];
        // Обход в глубину: тема, затем её родители по порядку.
        while let Some(name) = queue.pop() {
            if !seen.insert(name.clone()) {
                continue;
            }
            let Some((dirs, inherits)) = read_theme(&bases, &name) else {
                continue;
            };
            themes.push(Theme { name, dirs });
            queue.extend(inherits.into_iter().rev());
        }
        if !seen.contains("hicolor")
            && let Some((dirs, _)) = read_theme(&bases, "hicolor")
        {
            themes.push(Theme {
                name: "hicolor".into(),
                dirs,
            });
        }
        Icons {
            themes,
            bases,
            pixmaps,
            cache: HashMap::new(),
            folders: RefCell::new(HashMap::new()),
            scale: shell_render::scale(),
        }
    }

    /// Путь к файлу иконки, лучше всего подходящему под размер.
    pub fn find(&self, name: &str, size: u32) -> Option<PathBuf> {
        if name.starts_with('/') {
            return Path::new(name).is_file().then(|| PathBuf::from(name));
        }
        // Иногда в ярлыках пишут имя с расширением — стандарт так не велит, но бывает.
        let name = name
            .strip_suffix(".png")
            .or_else(|| name.strip_suffix(".svg"))
            .unwrap_or(name);
        for theme in &self.themes {
            if let Some(p) = self.find_in_theme(theme, name, size) {
                return Some(p);
            }
        }
        self.pixmaps
            .iter()
            .flat_map(|d| ["png", "svg"].map(|e| d.join(format!("{name}.{e}"))))
            .find(|p| p.is_file())
    }

    /// Забыть прочитанное содержимое папок: значки могли добавиться (поставили
    /// программу) или пропасть.
    pub fn refresh(&mut self) {
        self.folders.get_mut().clear();
        self.cache.clear();
    }

    /// Есть ли папка (с памятью: каждая проверяется один раз).
    fn folder_exists(&self, dir: &Path) -> bool {
        *self
            .folders
            .borrow_mut()
            .entry(dir.to_path_buf())
            .or_insert_with(|| dir.is_dir())
    }

    fn find_in_theme(&self, theme: &Theme, name: &str, size: u32) -> Option<PathBuf> {
        let file = |dir: &Dir| {
            self.bases.iter().find_map(|b| {
                let folder = b.join(&theme.name).join(&dir.path);
                if !self.folder_exists(&folder) {
                    return None;
                }
                ["png", "svg"]
                    .iter()
                    .map(|e| folder.join(format!("{name}.{e}")))
                    .find(|p| p.is_file())
            })
        };
        if let Some(p) = theme
            .dirs
            .iter()
            .filter(|d| d.matches(size, 1))
            .find_map(file)
        {
            return Some(p);
        }
        theme
            .dirs
            .iter()
            .filter_map(|d| file(d).map(|p| (d.distance(size, 1), p)))
            .min_by_key(|(dist, _)| *dist)
            .map(|(_, p)| p)
    }

    /// Картинка иконки размером `size`×`size` точек — в пикселях экрана
    /// (при масштабе 2 — вдвое больше: рисовать с уменьшением до `size`).
    /// Запоминается: второй раз — мгновенно.
    pub fn get(&mut self, name: &str, size: u32) -> Option<&Pixmap> {
        let px = shell_render::to_px(size, self.scale);
        let key = (name.to_string(), px);
        if !self.cache.contains_key(&key) {
            // Рисунок выбираем по размеру в точках: у одного имени в разных
            // размерах бывают разные рисунки (значок сети WhiteSur в 16, 22
            // и 24 — три разных), и при масштабе значок не должен меняться.
            // SVG рисуется чётко в любом размере; картинку PNG берём ближе к
            // пикселям, иначе она расплывётся.
            let path = self
                .find(name, size)
                .filter(|p| px == size || p.extension().is_some_and(|e| e == "svg"))
                .or_else(|| self.find(name, px));
            let pixmap = path.and_then(|p| render(&p, px));
            self.cache.insert(key.clone(), pixmap);
        }
        self.cache.get(&key).and_then(Option::as_ref)
    }
}

/// Читает `index.theme` первой найденной копии темы: папки и родители.
fn read_theme(bases: &[PathBuf], name: &str) -> Option<(Vec<Dir>, Vec<String>)> {
    let text = bases
        .iter()
        .find_map(|b| std::fs::read_to_string(b.join(name).join("index.theme")).ok())?;

    let mut groups: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut current = String::new();
    for line in text.lines().map(str::trim) {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some(g) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            current = g.to_string();
        } else if let Some((k, v)) = line.split_once('=') {
            groups
                .entry(current.clone())
                .or_default()
                .entry(k.trim().to_string())
                .or_insert_with(|| v.trim().to_string());
        }
    }

    let main = groups.get("Icon Theme")?;
    let list = |key: &str| -> Vec<String> {
        main.get(key)
            .map(|v| {
                v.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default()
    };
    let inherits = list("Inherits");
    let mut paths = list("Directories");
    paths.extend(list("ScaledDirectories"));

    let mut dirs: Vec<Dir> = paths
        .into_iter()
        .filter_map(|path| {
            let g = groups.get(&path)?;
            let num = |k: &str| g.get(k).and_then(|v| v.parse::<u32>().ok());
            let size = num("Size")?;
            let kind = match g.get("Type").map(String::as_str) {
                Some("Fixed") => Kind::Fixed,
                Some("Scalable") => Kind::Scalable {
                    min: num("MinSize").unwrap_or(size),
                    max: num("MaxSize").unwrap_or(size),
                },
                _ => Kind::Threshold(num("Threshold").unwrap_or(2)),
            };
            Some(Dir {
                size,
                scale: num("Scale").unwrap_or(1),
                kind,
                path,
            })
        })
        .collect();
    // Значки программ — вперёд (см. описание модуля). Сортировка устойчивая:
    // в остальном порядок из index.theme сохраняется.
    dirs.sort_by_key(|d| !d.path.starts_with("apps"));
    Some((dirs, inherits))
}

/// Рисует файл иконки в квадрат `size`×`size`, сохраняя пропорции.
fn render(path: &Path, size: u32) -> Option<Pixmap> {
    render_file(path, size, size, Fit::Contain)
}

/// Как уложить картинку в рамку.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fit {
    /// Целиком, с пустыми полями по краям (иконки).
    Contain,
    /// Заполнить всю рамку, лишнее по краям обрезать (обои, аватар).
    Cover,
}

/// Картинка из файла (SVG, PNG или JPEG — по содержимому, не по имени:
/// у `~/.face` расширения нет) в рамке `w`×`h`.
pub fn render_file(path: &Path, w: u32, h: u32, fit: Fit) -> Option<Pixmap> {
    let data = std::fs::read(path).ok()?;
    let mut out = Pixmap::new(w, h)?;
    let (fw, fh) = (w as f32, h as f32);
    let scale = |iw: f32, ih: f32| match fit {
        Fit::Contain => (fw / iw).min(fh / ih),
        Fit::Cover => (fw / iw).max(fh / ih),
    };
    if let Some(img) = decode_raster(&data) {
        // Сильное уменьшение одной интерполяцией даёт рябь — сначала
        // усредняем квадратиками в целое число раз.
        let k = scale(img.width() as f32, img.height() as f32);
        let img = match (1.0 / k).floor() as u32 {
            n if n >= 2 => shrink(&img, n)?,
            _ => img,
        };
        let (iw, ih) = (img.width() as f32, img.height() as f32);
        let k = scale(iw, ih);
        let paint = PixmapPaint {
            quality: FilterQuality::Bicubic,
            ..PixmapPaint::default()
        };
        let t =
            Transform::from_scale(k, k).post_translate((fw - iw * k) / 2.0, (fh - ih * k) / 2.0);
        out.draw_pixmap(0, 0, img.as_ref(), &paint, t, None);
    } else {
        let tree = resvg::usvg::Tree::from_data(&data, &resvg::usvg::Options::default()).ok()?;
        let (iw, ih) = (tree.size().width(), tree.size().height());
        let k = scale(iw, ih);
        let t =
            Transform::from_scale(k, k).post_translate((fw - iw * k) / 2.0, (fh - ih * k) / 2.0);
        resvg::render(&tree, t, &mut out.as_mut());
    }
    Some(out)
}

/// PNG или JPEG (по первым байтам файла).
fn decode_raster(data: &[u8]) -> Option<Pixmap> {
    if data.starts_with(b"\x89PNG") {
        return Pixmap::decode_png(data).ok();
    }
    if !data.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    use zune_jpeg::zune_core::bytestream::ZCursor;
    use zune_jpeg::zune_core::colorspace::ColorSpace;
    use zune_jpeg::zune_core::options::DecoderOptions;
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA);
    let mut decoder = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(data), options);
    decoder.decode_headers().ok()?;
    if decoder.output_colorspace()? != ColorSpace::RGBA {
        return None;
    }
    let rgba = decoder.decode().ok()?;
    let info = decoder.info()?;
    // В JPEG нет прозрачности — умножать на неё не нужно.
    Pixmap::from_vec(
        rgba,
        tiny_skia::IntSize::from_wh(u32::from(info.width), u32::from(info.height))?,
    )
}

/// Уменьшить в `n` раз: каждая точка — среднее квадрата `n`×`n`.
fn shrink(src: &Pixmap, n: u32) -> Option<Pixmap> {
    let (w, h) = (src.width() / n, src.height() / n);
    let mut out = Pixmap::new(w.max(1), h.max(1))?;
    let (sw, data) = (src.width() as usize, src.data());
    let n = n as usize;
    let dst = out.data_mut();
    for y in 0..h as usize {
        for x in 0..w as usize {
            let mut sum = [0u32; 4];
            for yy in y * n..(y + 1) * n {
                let row = (yy * sw + x * n) * 4;
                for px in data[row..row + n * 4].as_chunks::<4>().0 {
                    for c in 0..4 {
                        sum[c] += u32::from(px[c]);
                    }
                }
            }
            let o = (y * w as usize + x) * 4;
            for c in 0..4 {
                dst[o + c] = (sum[c] / (n * n) as u32) as u8;
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> TempDir {
            let dir = std::env::temp_dir()
                .join(format!("shell-icons-test-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }

        fn write(&self, rel: &str, text: &str) {
            let p = self.0.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><rect width="16" height="16" fill="#f00"/></svg>"##;

    /// Тема «child» наследует «parent»; у обеих есть свои иконки.
    fn setup(name: &str) -> TempDir {
        let t = TempDir::new(name);
        t.write(
            "icons/child/index.theme",
            "[Icon Theme]\nInherits=parent\nDirectories=preferences/32,apps/scalable\n\n\
             [preferences/32]\nSize=32\nType=Scalable\nMinSize=16\nMaxSize=256\n\n\
             [apps/scalable]\nSize=64\nType=Scalable\nMinSize=16\nMaxSize=512\n",
        );
        t.write(
            "icons/parent/index.theme",
            "[Icon Theme]\nDirectories=apps/16,apps/48\n\n\
             [apps/16]\nSize=16\nType=Fixed\n\n[apps/48]\nSize=48\nType=Fixed\n",
        );
        t.write(
            "icons/hicolor/index.theme",
            "[Icon Theme]\nDirectories=48x48/apps\n\n[48x48/apps]\nSize=48\nType=Threshold\n",
        );
        t.write("icons/child/preferences/32/files.svg", SVG);
        t.write("icons/child/apps/scalable/files.svg", SVG);
        t.write("icons/parent/apps/16/editor.svg", SVG);
        t.write("icons/parent/apps/48/editor.svg", SVG);
        t.write("icons/hicolor/48x48/apps/old.svg", SVG);
        t.write("pixmaps/legacy.svg", SVG);
        t
    }

    fn icons(t: &TempDir) -> Icons {
        Icons::with_dirs("child", vec![t.0.join("icons")], vec![t.0.join("pixmaps")])
    }

    fn rel(t: &TempDir, p: Option<PathBuf>) -> String {
        p.unwrap()
            .strip_prefix(&t.0)
            .unwrap()
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn scaled_icon_keeps_drawing_of_point_size() {
        // В 16 — красный, в 24 — синий рисунок: при масштабе 1,5 значок в
        // 16 точек берётся из 16 (тот же рисунок), а рисуется в 24 пикселя.
        let t = TempDir::new("scale");
        t.write(
            "icons/child/index.theme",
            "[Icon Theme]\nDirectories=status/16,status/24\n\n\
             [status/16]\nSize=16\nType=Fixed\n\n[status/24]\nSize=24\nType=Fixed\n",
        );
        t.write("icons/child/status/16/net.svg", SVG);
        t.write(
            "icons/child/status/24/net.svg",
            &SVG.replace("#f00", "#00f"),
        );
        let mut i = icons(&t);
        i.scale = 1.5;
        let pm = i.get("net", 16).unwrap();
        assert_eq!(pm.width(), 24);
        let p = pm.pixel(12, 12).unwrap();
        assert!(p.red() > 200 && p.blue() < 50, "{p:?}");
    }

    #[test]
    fn apps_folder_preferred() {
        let t = setup("apps");
        assert_eq!(
            rel(&t, icons(&t).find("files", 32)),
            "icons/child/apps/scalable/files.svg"
        );
    }

    #[test]
    fn inherited_and_closest_size() {
        let t = setup("inherit");
        let i = icons(&t);
        assert_eq!(
            rel(&t, i.find("editor", 48)),
            "icons/parent/apps/48/editor.svg"
        );
        // 26 точно не подходит ни к 16, ни к 48 — берётся ближайшая (16 на 10 ближе 48).
        assert_eq!(
            rel(&t, i.find("editor", 26)),
            "icons/parent/apps/16/editor.svg"
        );
        assert_eq!(
            rel(&t, i.find("old", 26)),
            "icons/hicolor/48x48/apps/old.svg"
        );
        assert_eq!(rel(&t, i.find("legacy", 26)), "pixmaps/legacy.svg");
        assert_eq!(
            rel(&t, i.find("editor.svg", 48)),
            "icons/parent/apps/48/editor.svg"
        );
        assert!(i.find("missing", 26).is_none());
    }

    #[test]
    fn rendered_to_size() {
        let t = setup("render");
        let mut i = icons(&t);
        let p = i.get("files", 26).unwrap();
        assert_eq!((p.width(), p.height()), (26, 26));
        // Красный квадрат на всю иконку.
        assert_eq!(p.pixel(13, 13).unwrap().red(), 255);
        assert!(i.get("missing", 26).is_none());
    }

    #[test]
    fn image_files_by_content_and_fit() {
        let t = TempDir::new("files");
        // SVG 2:1 без расширения (как ~/.face): левая половина красная, правая синяя.
        t.write(
            "face",
            "<svg xmlns='http://www.w3.org/2000/svg' width='20' height='10'>\
             <rect width='10' height='10' fill='#f00'/>\
             <rect x='10' width='10' height='10' fill='#00f'/></svg>",
        );
        let cover = render_file(&t.0.join("face"), 10, 10, Fit::Cover).unwrap();
        // Заполнено целиком: края не прозрачные, середина — стык половин.
        assert_eq!(cover.pixel(0, 0).unwrap().alpha(), 255);
        assert_eq!(cover.pixel(0, 5).unwrap().red(), 255);
        assert_eq!(cover.pixel(9, 5).unwrap().blue(), 255);
        let contain = render_file(&t.0.join("face"), 10, 10, Fit::Contain).unwrap();
        assert_eq!(contain.pixel(5, 0).unwrap().alpha(), 0, "поля сверху");

        // JPEG 64×32 (красный | синий) в 16×16 с заполнением: уменьшение в 2 раза
        // усреднением, края — чистые цвета.
        let jpg = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/half.jpg");
        let p = render_file(&jpg, 16, 16, Fit::Cover).unwrap();
        assert!(p.pixel(1, 8).unwrap().red() > 240);
        assert!(p.pixel(14, 8).unwrap().blue() > 240);
        assert_eq!(p.pixel(0, 0).unwrap().alpha(), 255);

        // PNG по содержимому, имя без расширения.
        let mut png = Pixmap::new(4, 4).unwrap();
        png.fill(tiny_skia::Color::from_rgba8(0, 255, 0, 255));
        std::fs::write(t.0.join("pic"), png.encode_png().unwrap()).unwrap();
        let p = render_file(&t.0.join("pic"), 8, 8, Fit::Contain).unwrap();
        assert_eq!(p.pixel(4, 4).unwrap().green(), 255);

        assert!(render_file(&t.0.join("missing"), 8, 8, Fit::Cover).is_none());
        t.write("junk", "не картинка");
        assert!(render_file(&t.0.join("junk"), 8, 8, Fit::Cover).is_none());
    }
}
