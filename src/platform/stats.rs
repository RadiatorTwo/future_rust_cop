//! `FCOP_STATS=1` (on with `FCOP_SAMPLE=1` too): frames a second and frame times, drawn by us
//! in the window's top left corner, so no overlay tool (RivaTuner) has to hook into our GL.
//! Counted are the new pictures shown (`picture`: Glide's and 2D frames), not the window's
//! repaints. The text is the last half second's: frames a second and the mean frame time,
//! then the longest and the 99th percentile frame time of the last second and the graph's
//! top (at least 10 ms, as RivaTuner's fixed range: small changes stay small). The graph goes
//! by time, as RivaTuner's: one column per 1/60 s, 4 s across, each the mean frame time of the
//! frames ending in it (a frame longer than a column fills every column it lasted with its
//! time: a hitch stays as tall as it was), so it moves at the same pace at any frame rate;
//! yellow above twice the mean, red above four times. Redrawn 60 times a second; not in
//! FCOP_SHOT's pictures.
use super::dialog::draw::glyph;
use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| ["FCOP_STATS", "FCOP_SAMPLE"].iter().any(|v| std::env::var_os(v).is_some_and(|v| v != "0")))
}

/// Columns of the graph, one pixel each: the picture's width.
pub const W: usize = 240;
const COL_MS: f32 = 1000.0 / 60.0;
const LINE: usize = 10;
const GRAPH: usize = 48;
/// The picture's height: two lines of text, a gap, the graph.
pub const H: usize = 2 * LINE + 2 + GRAPH;

struct Stats {
    start: Option<Instant>,
    last: Option<Instant>,
    /// frame times in ms of the last second, oldest first, and their sum
    times: VecDeque<f32>,
    total: f32,
    /// the graph's columns, oldest first; `col`: the number of columns made so far
    cols: VecDeque<Col>,
    col: u64,
    text: [String; 2],
    mean: f32,
    text_at: Option<Instant>,
    drawn: Option<Instant>,
}

/// A column of the graph: the frames ending in it (sum of their times, count), and the
/// longest frame it lies within.
#[derive(Clone, Copy, Default)]
struct Col {
    sum: f32,
    n: u32,
    long: f32,
}

impl Col {
    fn ms(&self) -> f32 {
        if self.n == 0 { self.long } else { (self.sum / self.n as f32).max(self.long) }
    }
}

static STATS: Mutex<Stats> = Mutex::new(Stats {
    start: None,
    last: None,
    times: VecDeque::new(),
    total: 0.0,
    cols: VecDeque::new(),
    col: 0,
    text: [String::new(), String::new()],
    mean: 0.0,
    text_at: None,
    drawn: None,
});

/// A new picture is shown now.
pub fn picture() {
    if !on() {
        return;
    }
    let s = &mut *STATS.lock().unwrap();
    let now = Instant::now();
    let start = *s.start.get_or_insert(now);
    let col_of = |t: Instant| ((t - start).as_secs_f32() * 1000.0 / COL_MS) as u64;
    let c = col_of(now);
    while s.col <= c {
        s.cols.push_back(Col::default());
        s.col += 1;
        if s.cols.len() > W {
            s.cols.pop_front();
        }
    }
    let Some(t) = s.last.replace(now) else { return };
    let ms = (now - t).as_secs_f32() * 1000.0;
    s.times.push_back(ms);
    s.total += ms;
    while s.total - s.times[0] >= 1000.0 {
        s.total -= s.times.pop_front().unwrap();
    }
    // the column it ends in, and every column before it lasted
    let n = s.cols.len() as u64;
    let last = s.cols.back_mut().unwrap();
    last.sum += ms;
    last.n += 1;
    for k in col_of(t).max(s.col - n)..s.col - 1 {
        let i = (n - (s.col - k)) as usize;
        s.cols[i].long = s.cols[i].long.max(ms);
    }
}

/// The display's new pixels (ARGB words, W x H) when they are due.
pub fn pixels() -> Option<Vec<u32>> {
    let s = &mut *STATS.lock().unwrap();
    let now = Instant::now();
    if s.drawn.is_some_and(|d| now - d < Duration::from_millis(16)) {
        return None;
    }
    s.drawn = Some(now);
    if s.text_at.is_none_or(|d| now - d >= Duration::from_millis(500)) {
        s.text_at = Some(now);
        text_of(s);
    }
    Some(render(s))
}

/// The text: frames a second and mean over the last half second, longest and 99th percentile
/// over the last second.
fn text_of(s: &mut Stats) {
    let (mut sum, mut n) = (0.0, 0);
    for &t in s.times.iter().rev() {
        if sum >= 500.0 {
            break;
        }
        sum += t;
        n += 1;
    }
    if n == 0 {
        return;
    }
    s.mean = sum / n as f32;
    let mut second: Vec<f32> = s.times.iter().copied().collect();
    second.sort_unstable_by(f32::total_cmp);
    let max = second[second.len() - 1];
    let p99 = second[((second.len() - 1) as f32 * 0.99).round() as usize];
    s.text[0] = format!("{:.0} fps  {:.2} ms", n as f32 * 1000.0 / sum, s.mean);
    s.text[1] = format!("max {max:.2}  99% {p99:.2}");
}

fn render(s: &Stats) -> Vec<u32> {
    let mut px = vec![0xa000_0000u32; W * H];
    let max_graph = s.cols.iter().map(Col::ms).fold(0.0f32, f32::max);
    let top = [10.0f32, 20.0, 50.0, 100.0, 200.0, 500.0, 1000.0].into_iter().find(|&t| t >= 1.25 * max_graph).unwrap_or(1000.0);
    text(&mut px, 2, 1, &s.text[0], 0xffff_ffff);
    text(&mut px, 2, 1 + LINE, &format!("{}  top {top:.0} ms", s.text[1]), 0xffc0_c0c0);
    let skip = W - s.cols.len();
    for (x, t) in s.cols.iter().map(Col::ms).enumerate() {
        if t == 0.0 {
            continue;
        }
        let x = x + skip;
        let bar = ((t / top).min(1.0) * GRAPH as f32).round().max(1.0) as usize;
        let (line, fill) = if t > 4.0 * s.mean {
            (0xffff_5050, 0x80c0_3030)
        } else if t > 2.0 * s.mean {
            (0xffff_e050, 0x80c0_a030)
        } else {
            (0xff60_ff60, 0x8030_a030)
        };
        for y in H - bar..H {
            px[y * W + x] = if y == H - bar { line } else { fill };
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
