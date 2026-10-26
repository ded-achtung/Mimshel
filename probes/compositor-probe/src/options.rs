use std::time::Duration;

pub struct Options {
    pub size: (i32, i32),
    pub bench: Option<Duration>,
    pub unthrottled: bool,
    pub wallpaper: bool,
    pub dock: bool,
    pub spawn: Vec<String>,
}
impl Options {
    pub fn parse() -> Result<Self, Box<dyn std::error::Error>> {
        let mut opts = Self {
            size: (1280, 800),
            bench: None,
            unthrottled: false,
            wallpaper: true,
            dock: true,
            spawn: Vec::new(),
        };
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--size" => {
                    let size = args.next().ok_or("--size needs WxH")?;
                    let (w, h) = size.split_once('x').ok_or("--size needs WxH")?;
                    opts.size = (w.parse()?, h.parse()?);
                    if opts.size.0 <= 0 || opts.size.1 <= 0 {
                        return Err("size must be positive".into());
                    }
                }
                "--bench" => {
                    let seconds: f64 = args.next().ok_or("--bench needs seconds")?.parse()?;
                    if !seconds.is_finite() || seconds <= 0.0 || seconds > 86400.0 {
                        return Err("benchmark duration must be in (0, 86400] seconds".into());
                    }
                    opts.bench = Some(Duration::from_secs_f64(seconds));
                }
                "--unthrottled" => opts.unthrottled = true,
                "--no-wallpaper" => opts.wallpaper = false,
                "--no-dock" => opts.dock = false,
                "--spawn" => opts
                    .spawn
                    .push(args.next().ok_or("--spawn needs a shell command")?),
                "--help" | "-h" => {
                    println!(
                        "compositor-probe [--size WxH] [--bench SECONDS] [--unthrottled] [--no-wallpaper] [--no-dock] [--spawn CMD]..."
                    );
                    std::process::exit(0);
                }
                _ => return Err(format!("unknown option: {arg}").into()),
            }
        }
        Ok(opts)
    }
}
