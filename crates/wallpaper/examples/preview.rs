//! Нарисовать все обои в PNG: `cargo run -p shell-wallpaper --example preview -- ШИРИНА ВЫСОТА ПАПКА`
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (w, h) = (a[0].parse().unwrap(), a[1].parse().unwrap());
    for style in shell_wallpaper::Style::ALL {
        let t = std::time::Instant::now();
        let pm = shell_wallpaper::draw(style, w, h).unwrap();
        let path = format!("{}/{}.png", a[2], style.name());
        pm.save_png(&path).unwrap();
        println!("{path} за {:?}", t.elapsed());
    }
}
