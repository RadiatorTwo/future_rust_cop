//! `FCOP_STATS=1` (on with `FCOP_SAMPLE=1` too): frames a second and frame times, drawn by us
//! in the window's top left corner, so no overlay tool (RivaTuner) has to hook into our GL.
//! Counted are the pictures shown (each `Gl::draw`, at its start: the time between two
//! presents). First line: frames a second and the mean frame time over the last second;
//! second line: the longest and the 99th percentile frame time of that second and the graph's
//! top; below, the last 240 frame times as bars (yellow above 1.5 times the mean, red at the
//! top). Redrawn 10 times a second; not in FCOP_SHOT's pictures.
use super::dialog::draw::glyph;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

pub fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| ["FCOP_STATS", "FCOP_SAMPLE"].iter().any(|v| std::env::var_os(v).is_some_and(|v| v != "0")))
}

/// Bars in the graph, one pixel each: the picture's width.
pub const W: usize = 240;
const LINE: usize = 10;
const GRAPH: usize = 48;
/// The picture's height: two lines of text, a gap, the graph.
pub const H: usize = 2 * LINE + 2 + GRAPH;

struct Stats {
    last: Option<Instant>,
    /// frame times in ms, oldest first: at least the last second and the graph's bars
    times: VecDeque<f32>,
    total: f32,
    drawn: Option<Instant>,
}

thread_local! {
    static STATS: RefCell<Stats> = const { RefCell::new(Stats { last: None, times: VecDeque::new(), total: 0.0, drawn: None }) };
}

/// A picture is shown now. Returns the display's new pixels (ARGB words, W x H) when it is due.
pub fn tick() -> Option<Vec<u32>> {
    STATS.with_borrow_mut(|s| {
        let now = Instant::now();
        if let Some(t) = s.last.replace(now) {
            let ms = (now - t).as_secs_f32() * 1000.0;
            s.times.push_back(ms);
            s.total += ms;
            while s.times.len() > W && s.total - s.times[0] >= 1000.0 {
                s.total -= s.times.pop_front().unwrap();
            }
        }
        if s.drawn.is_some_and(|d| now - d < Duration::from_millis(100)) {
            return None;
        }
        s.drawn = Some(now);
        Some(render(&s.times))
    })
}

fn render(times: &VecDeque<f32>) -> Vec<u32> {
    let mut px = vec![0xa000_0000u32; W * H];
    // the last second
    let mut second = Vec::new();
    let mut sum = 0.0;
    for &t in times.iter().rev() {
        if sum >= 1000.0 {
            break;
        }
        sum += t;
        second.push(t);
    }
    let graph: Vec<f32> = times.iter().rev().take(W).rev().copied().collect();
    let max_graph = graph.iter().copied().fold(0.0f32, f32::max);
    let top = [1.0f32, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0, 1000.0].into_iter().find(|&t| t >= max_graph).unwrap_or(1000.0);
    if !second.is_empty() {
        let mean = sum / second.len() as f32;
        let max = second.iter().copied().fold(0.0f32, f32::max);
        second.sort_unstable_by(f32::total_cmp);
        let p99 = second[((second.len() - 1) as f32 * 0.99).round() as usize];
        text(&mut px, 2, 1, &format!("{:.0} fps  {mean:.2} ms", second.len() as f32 * 1000.0 / sum), 0xffff_ffff);
        text(&mut px, 2, 1 + LINE, &format!("max {max:.2}  99% {p99:.2}  top {top:.0} ms"), 0xffc0_c0c0);
        for (x, &t) in graph.iter().enumerate() {
            let x = x + W - graph.len();
            let bar = ((t / top).min(1.0) * GRAPH as f32).round().max(1.0) as usize;
            let c = if t >= top { 0xffff_4040 } else if t > 1.5 * mean { 0xffff_d040 } else { 0xff40_e040 };
            for y in H - bar..H {
                px[y * W + x] = c;
            }
        }
    }
    px
}

fn text(px: &mut [u32], x: usize, y: usize, s: &str, c: u32) {
    let mut x = x as i32;
    for ch in s.chars() {
        let Some((g, l, w)) = glyph(ch) else {
            x += 3;
            continue;
        };
        for (row, bits) in g.iter().enumerate() {
            for col in l..l + w {
                let px_x = x + col - l;
                if bits >> col & 1 != 0 && (0..W as i32).contains(&px_x) {
                    px[(y + row) * W + px_x as usize] = c;
                }
            }
        }
        x += w + 1;
    }
}
