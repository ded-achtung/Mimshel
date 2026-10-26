use std::time::{Duration, Instant};

#[derive(Debug, Default)]
pub struct Stats {
    pub avg: f64,
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
}

/// Milliseconds, nearest-rank percentiles (rank = ceil(p * n)).
pub fn statistics(samples: &[f64]) -> Stats {
    if samples.is_empty() {
        return Stats::default();
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let percentile = |p: f64| sorted[(p * sorted.len() as f64).ceil() as usize - 1];
    Stats {
        avg: sorted.iter().sum::<f64>() / sorted.len() as f64,
        p50: percentile(0.50),
        p95: percentile(0.95),
        p99: percentile(0.99),
        max: *sorted.last().unwrap(),
    }
}

pub struct FrameTimings {
    pub render: Duration,
    pub present: Duration,
}

pub struct Timing {
    render: Vec<f64>,
    present: Vec<f64>,
    frame: Vec<f64>,
    all_render: Vec<f64>,
    all_present: Vec<f64>,
    all_frame: Vec<f64>,
    last_frame: Option<Instant>,
    last_report: Instant,
    retain: bool,
    pub frames: u64,
}
impl Timing {
    pub fn new(retain: bool) -> Self {
        Self {
            render: Vec::new(),
            present: Vec::new(),
            frame: Vec::new(),
            all_render: Vec::new(),
            all_present: Vec::new(),
            all_frame: Vec::new(),
            last_frame: None,
            last_report: Instant::now(),
            retain,
            frames: 0,
        }
    }
    pub fn record(&mut self, start: Instant, measurements: FrameTimings) {
        let ms = measurements.render.as_secs_f64() * 1000.0;
        let present = measurements.present.as_secs_f64() * 1000.0;
        self.render.push(ms);
        self.present.push(present);
        if self.retain {
            self.all_render.push(ms);
            self.all_present.push(present);
        }
        if let Some(last) = self.last_frame.replace(start) {
            let interval = start.duration_since(last).as_secs_f64() * 1000.0;
            self.frame.push(interval);
            if self.retain {
                self.all_frame.push(interval);
            }
        }
        self.frames += 1;
        let elapsed = self.last_report.elapsed().as_secs_f64();
        if elapsed >= 1.0 {
            let r = statistics(&self.render);
            let p = statistics(&self.present);
            println!(
                "fps={:.1} render avg={:.3}ms p95={:.3}ms max={:.3}ms present avg={:.3}ms p95={:.3}ms max={:.3}ms frame avg={:.3}ms",
                self.render.len() as f64 / elapsed,
                r.avg,
                r.p95,
                r.max,
                p.avg,
                p.p95,
                p.max,
                statistics(&self.frame).avg
            );
            self.render.clear();
            self.present.clear();
            self.frame.clear();
            self.last_report = Instant::now();
        }
    }
    pub fn summary(&self, size: (i32, i32), pacing: &str, wallpaper: bool, dock: bool) -> String {
        let r = statistics(&self.all_render);
        let p = statistics(&self.all_present);
        format!(
            "summary size={}x{} pacing={} wallpaper={} dock={} frames={} render avg={:.3}ms p50={:.3}ms p95={:.3}ms p99={:.3}ms max={:.3}ms present avg={:.3}ms p50={:.3}ms p95={:.3}ms p99={:.3}ms max={:.3}ms frame avg={:.3}ms",
            size.0,
            size.1,
            pacing,
            wallpaper,
            dock,
            self.frames,
            r.avg,
            r.p50,
            r.p95,
            r.p99,
            r.max,
            p.avg,
            p.p50,
            p.p95,
            p.p99,
            p.max,
            statistics(&self.all_frame).avg
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn render_and_present_are_independent_and_summary_has_context() {
        let mut timing = Timing::new(true);
        let start = Instant::now();
        for i in 0..2 {
            timing.record(
                start + Duration::from_millis(i * 16),
                FrameTimings {
                    render: Duration::from_millis(2),
                    present: Duration::from_millis(14),
                },
            );
        }
        assert_eq!(statistics(&timing.all_render).avg, 2.0);
        assert_eq!(statistics(&timing.all_present).avg, 14.0);
        assert_eq!(statistics(&timing.all_frame).avg, 16.0);
        let summary = timing.summary((1920, 1200), "unthrottled(vblank_mode=0)", true, false);
        for expected in [
            "size=1920x1200",
            "pacing=unthrottled(vblank_mode=0)",
            "wallpaper=true dock=false",
            "frames=2",
            "render avg=2.000ms",
            "present avg=14.000ms",
            "frame avg=16.000ms",
        ] {
            assert!(summary.contains(expected), "{summary}");
        }
    }

    #[test]
    fn empty_and_single() {
        assert_eq!(statistics(&[]).avg, 0.0);
        let s = statistics(&[2.5]);
        assert_eq!(
            (s.avg, s.p50, s.p95, s.p99, s.max),
            (2.5, 2.5, 2.5, 2.5, 2.5)
        );
    }
    #[test]
    fn unsorted_nearest_rank() {
        let samples: Vec<_> = (1..=100).rev().map(f64::from).collect();
        let s = statistics(&samples);
        assert_eq!(
            (s.avg, s.p50, s.p95, s.p99, s.max),
            (50.5, 50.0, 95.0, 99.0, 100.0)
        );
        assert_eq!(statistics(&[9.0, 1.0]).p50, 1.0);
    }
}
