//! DirectShow, as far as the game plays its videos with it (0x408800): CoCreateInstance gives a
//! filter graph, from which it asks for IMediaControl, IMediaEventEx, IVideoWindow and
//! IBasicAudio; RenderFile opens an MPEG-1 file, the video window is placed in the game
//! window, Run starts it. At the end EC_COMPLETE is queued and the notify message (WM_USER)
//! posted; the game then fetches the event (0x4086c0) and releases everything. Space skips a
//! video: the game calls Stop itself.
//!
//! The movie is decoded by `crate::mpeg` on a thread of its own: its frames go to the window's
//! video layer (`window::set_video`), its sound to the mixer's stream (`dsound::stream_*`).

use super::{Import, arg, com_new, host_path, log, mem, window};
use crate::mpeg;
use crate::rt::Cpu;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const OK: u32 = 0;
const E_NOTIMPL: u32 = 0x8000_4001;
const E_NOINTERFACE: u32 = 0x8000_4002;
const E_ABORT: u32 = 0x8000_4004;
const E_FAIL: u32 = 0x8000_4005;
const REGDB_E_CLASSNOTREG: u32 = 0x8004_0154;
const VFW_E_NOT_FOUND: u32 = 0x8004_0216;

const EC_COMPLETE: u32 = 1;

/// A GUID as its 16 bytes in memory.
const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> [u8; 16] {
    let (a, b, c) = (d1.to_le_bytes(), d2.to_le_bytes(), d3.to_le_bytes());
    [a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d4[0], d4[1], d4[2], d4[3], d4[4], d4[5], d4[6], d4[7]]
}

const QUARTZ: [u8; 8] = [0xb0, 0x3a, 0x00, 0x20, 0xaf, 0x0b, 0xa7, 0x70];
const CLSID_FILTER_GRAPH: [u8; 16] = guid(0xe436ebb3, 0x524f, 0x11ce, [0x9f, 0x53, 0x00, 0x20, 0xaf, 0x0b, 0xa7, 0x70]);
const IID_IUNKNOWN: [u8; 16] = guid(0, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46]);
const IID_IGRAPH_BUILDER: [u8; 16] = guid(0x56a868a9, 0x0ad4, 0x11ce, QUARTZ);
const IID_IFILTER_GRAPH: [u8; 16] = guid(0x56a8689f, 0x0ad4, 0x11ce, QUARTZ);
const IID_IMEDIA_CONTROL: [u8; 16] = guid(0x56a868b1, 0x0ad4, 0x11ce, QUARTZ);
const IID_IMEDIA_EVENT: [u8; 16] = guid(0x56a868b6, 0x0ad4, 0x11ce, QUARTZ);
const IID_IMEDIA_EVENT_EX: [u8; 16] = guid(0x56a868c0, 0x0ad4, 0x11ce, QUARTZ);
const IID_IVIDEO_WINDOW: [u8; 16] = guid(0x56a868b4, 0x0ad4, 0x11ce, QUARTZ);
const IID_IBASIC_AUDIO: [u8; 16] = guid(0x56a868b3, 0x0ad4, 0x11ce, QUARTZ);

/// The filter graph (one at a time: the game plays one video after the other).
struct Graph {
    /// references, for all interfaces together
    refs: u32,
    /// the interfaces' objects: graph builder, media control, media event, video window,
    /// basic audio
    objects: [u32; 5],
    movie: Option<Arc<mpeg::Streams>>,
    /// the video's size and its window's place in the owner's client area: x, y, w, h
    size: (i32, i32),
    rect: [i32; 4],
    visible: bool,
    /// hundredths of a dB
    volume: i32,
    /// SetNotifyWindow: window, message, lParam
    notify: (u32, u32, u32),
    events: VecDeque<(u32, u32, u32)>,
    player: Option<Player>,
}

struct Player {
    stop: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

static GRAPH: Mutex<Option<Graph>> = Mutex::new(None);

fn graph() -> MutexGuard<'static, Option<Graph>> {
    GRAPH.lock().unwrap_or_else(|e| e.into_inner())
}

fn trace() -> bool {
    static T: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *T.get_or_init(|| std::env::var_os("FCOP_VIDEO_LOG").is_some())
}

fn read_guid(p: u32) -> [u8; 16] {
    mem().slice(p, 16).try_into().unwrap()
}

fn wide_str(p: u32) -> String {
    let m = mem();
    (0..).map(|i| m.rd16(p + 2 * i)).take_while(|&c| c != 0).map(|c| char::from_u32(c as u32).unwrap_or('?')).collect()
}

// ---- creation and IUnknown ----------------------------------------------------------------------

/// CoCreateInstance(clsid, outer, context, iid, out): only the filter graph exists.
fn co_create_instance(c: &mut Cpu) -> u32 {
    let (clsid, iid, out) = (read_guid(arg(c, 0)), arg(c, 3), arg(c, 4));
    mem().wr32(out, 0);
    if clsid != CLSID_FILTER_GRAPH {
        log!("warn: CoCreateInstance: class {:02x?} not available", clsid);
        return REGDB_E_CLASSNOTREG;
    }
    stop_player();
    let objects = [com_new(GRAPH_BUILDER, 0), com_new(MEDIA_CONTROL, 0), com_new(MEDIA_EVENT, 0), com_new(VIDEO_WINDOW, 0), com_new(BASIC_AUDIO, 0)];
    *graph() = Some(Graph {
        refs: 0,
        objects,
        movie: None,
        size: (0, 0),
        rect: [0; 4],
        visible: true,
        volume: 0,
        notify: (0, 0, 0),
        events: VecDeque::new(),
        player: None,
    });
    query(iid, out)
}

/// Hand out the interface `iid` (a GUID's address) at `out`, with a reference.
fn query(iid: u32, out: u32) -> u32 {
    let id = read_guid(iid);
    let mut g = graph();
    let Some(g) = g.as_mut() else { return E_FAIL };
    let k = match id {
        IID_IUNKNOWN | IID_IGRAPH_BUILDER | IID_IFILTER_GRAPH => 0,
        IID_IMEDIA_CONTROL => 1,
        IID_IMEDIA_EVENT | IID_IMEDIA_EVENT_EX => 2,
        IID_IVIDEO_WINDOW => 3,
        IID_IBASIC_AUDIO => 4,
        _ => {
            log!("warn: DirectShow: interface {:02x?} not supported", id);
            mem().wr32(out, 0);
            return E_NOINTERFACE;
        }
    };
    g.refs += 1;
    mem().wr32(out, g.objects[k]);
    OK
}

fn query_interface(c: &mut Cpu) -> u32 {
    query(arg(c, 1), arg(c, 2))
}

fn add_ref(_: &mut Cpu) -> u32 {
    let mut g = graph();
    g.as_mut().map_or(0, |g| {
        g.refs += 1;
        g.refs
    })
}

fn release(_: &mut Cpu) -> u32 {
    let refs = {
        let mut g = graph();
        let Some(gr) = g.as_mut() else { return 0 };
        gr.refs = gr.refs.saturating_sub(1);
        gr.refs
    };
    if refs == 0 {
        stop_player();
        *graph() = None;
    }
    refs
}

fn not_impl(_: &mut Cpu) -> u32 {
    E_NOTIMPL
}

fn ok(_: &mut Cpu) -> u32 {
    OK
}

/// IDispatch::GetTypeInfoCount: no type information.
fn type_info_count(c: &mut Cpu) -> u32 {
    mem().wr32(arg(c, 1), 0);
    OK
}

// ---- IGraphBuilder ------------------------------------------------------------------------------

fn render_file(c: &mut Cpu) -> u32 {
    let name = wide_str(arg(c, 1));
    let path = host_path(&name);
    let data = match std::fs::read(&path) {
        Ok(d) => d,
        Err(e) => {
            log!("warn: video {name}: {} ({e})", path.display());
            return VFW_E_NOT_FOUND;
        }
    };
    let streams = mpeg::demux(&data);
    let Some(v) = mpeg::video::Decoder::new(&streams.video) else {
        log!("warn: video {name}: no MPEG-1 video in {}", path.display());
        return E_FAIL;
    };
    let size = (v.width as i32, v.height as i32);
    if trace() {
        log!("video {name}: {}x{} at {} fps, {} bytes of sound", size.0, size.1, v.fps, streams.audio.len());
    }
    stop_player();
    let mut g = graph();
    let Some(g) = g.as_mut() else { return E_FAIL };
    g.size = size;
    g.rect = [0, 0, size.0, size.1];
    g.movie = Some(Arc::new(streams));
    OK
}

static GRAPH_BUILDER: &[Import] = &[
    ("IGraphBuilder::QueryInterface", 3, query_interface),
    ("IGraphBuilder::AddRef", 1, add_ref),
    ("IGraphBuilder::Release", 1, release),
    ("IGraphBuilder::AddFilter", 3, not_impl),
    ("IGraphBuilder::RemoveFilter", 2, not_impl),
    ("IGraphBuilder::EnumFilters", 2, not_impl),
    ("IGraphBuilder::FindFilterByName", 3, not_impl),
    ("IGraphBuilder::ConnectDirect", 4, not_impl),
    ("IGraphBuilder::Reconnect", 2, not_impl),
    ("IGraphBuilder::Disconnect", 2, not_impl),
    ("IGraphBuilder::SetDefaultSyncSource", 1, ok),
    ("IGraphBuilder::Connect", 3, not_impl),
    ("IGraphBuilder::Render", 2, not_impl),
    ("IGraphBuilder::RenderFile", 3, render_file),
    ("IGraphBuilder::AddSourceFilter", 4, not_impl),
    ("IGraphBuilder::SetLogFile", 2, ok),
    ("IGraphBuilder::Abort", 1, ok),
    ("IGraphBuilder::ShouldOperationContinue", 1, ok),
];

// ---- IMediaControl ------------------------------------------------------------------------------

fn run(_: &mut Cpu) -> u32 {
    let mut g = graph();
    let Some(g) = g.as_mut() else { return E_FAIL };
    if let Some(p) = &g.player {
        p.paused.store(false, Ordering::Relaxed);
        return OK;
    }
    let Some(movie) = g.movie.clone() else { return E_FAIL };
    let stop = Arc::new(AtomicBool::new(false));
    let paused = Arc::new(AtomicBool::new(false));
    let (s, p, volume) = (stop.clone(), paused.clone(), g.volume);
    let thread = std::thread::Builder::new().name("video".into()).spawn(move || play(&movie, &s, &p, volume)).expect("cannot start the video thread");
    g.player = Some(Player { stop, paused, thread });
    OK
}

fn pause(_: &mut Cpu) -> u32 {
    if let Some(p) = graph().as_ref().and_then(|g| g.player.as_ref()) {
        p.paused.store(true, Ordering::Relaxed);
    }
    OK
}

fn stop(_: &mut Cpu) -> u32 {
    stop_player();
    OK
}

/// Stop the playing video, if any (without EC_COMPLETE).
fn stop_player() {
    let p = graph().as_mut().and_then(|g| g.player.take());
    if let Some(p) = p {
        p.stop.store(true, Ordering::Relaxed);
        let _ = p.thread.join();
    }
}

/// GetState(timeout, state): 0 stopped, 1 paused, 2 running.
fn get_state(c: &mut Cpu) -> u32 {
    let g = graph();
    let s = g.as_ref().and_then(|g| g.player.as_ref()).map_or(0, |p| if p.paused.load(Ordering::Relaxed) { 1 } else { 2 });
    mem().wr32(arg(c, 2), s);
    OK
}

static MEDIA_CONTROL: &[Import] = &[
    ("IMediaControl::QueryInterface", 3, query_interface),
    ("IMediaControl::AddRef", 1, add_ref),
    ("IMediaControl::Release", 1, release),
    ("IMediaControl::GetTypeInfoCount", 2, type_info_count),
    ("IMediaControl::GetTypeInfo", 4, not_impl),
    ("IMediaControl::GetIDsOfNames", 6, not_impl),
    ("IMediaControl::Invoke", 9, not_impl),
    ("IMediaControl::Run", 1, run),
    ("IMediaControl::Pause", 1, pause),
    ("IMediaControl::Stop", 1, stop),
    ("IMediaControl::GetState", 3, get_state),
    ("IMediaControl::RenderFile", 2, not_impl),
    ("IMediaControl::AddSourceFilter", 3, not_impl),
    ("IMediaControl::get_FilterCollection", 2, not_impl),
    ("IMediaControl::get_RegFilterCollection", 2, not_impl),
    ("IMediaControl::StopWhenReady", 1, stop),
];

// ---- IMediaEventEx ------------------------------------------------------------------------------

/// GetEvent(code, param1, param2, timeout): the next event, E_ABORT when there is none.
fn get_event(c: &mut Cpu) -> u32 {
    let e = graph().as_mut().and_then(|g| g.events.pop_front());
    let Some((code, p1, p2)) = e else { return E_ABORT };
    let m = mem();
    m.wr32(arg(c, 1), code);
    m.wr32(arg(c, 2), p1);
    m.wr32(arg(c, 3), p2);
    OK
}

fn set_notify_window(c: &mut Cpu) -> u32 {
    if let Some(g) = graph().as_mut() {
        g.notify = (arg(c, 1), arg(c, 2), arg(c, 3));
    }
    OK
}

static MEDIA_EVENT: &[Import] = &[
    ("IMediaEventEx::QueryInterface", 3, query_interface),
    ("IMediaEventEx::AddRef", 1, add_ref),
    ("IMediaEventEx::Release", 1, release),
    ("IMediaEventEx::GetTypeInfoCount", 2, type_info_count),
    ("IMediaEventEx::GetTypeInfo", 4, not_impl),
    ("IMediaEventEx::GetIDsOfNames", 6, not_impl),
    ("IMediaEventEx::Invoke", 9, not_impl),
    ("IMediaEventEx::GetEventHandle", 2, not_impl),
    ("IMediaEventEx::GetEvent", 5, get_event),
    ("IMediaEventEx::WaitForCompletion", 3, not_impl),
    ("IMediaEventEx::CancelDefaultHandling", 2, ok),
    ("IMediaEventEx::RestoreDefaultHandling", 2, ok),
    ("IMediaEventEx::FreeEventParams", 4, ok),
    ("IMediaEventEx::SetNotifyWindow", 4, set_notify_window),
    ("IMediaEventEx::SetNotifyFlags", 2, ok),
    ("IMediaEventEx::GetNotifyFlags", 2, not_impl),
];

// ---- IVideoWindow -------------------------------------------------------------------------------

fn put_visible(c: &mut Cpu) -> u32 {
    if let Some(g) = graph().as_mut() {
        // OATRUE is -1
        g.visible = arg(c, 1) != 0;
        if !g.visible {
            // the owner is painted where the video window was: the game redraws its menu
            // at that WM_PAINT after a video that played to its end
            super::user32::invalidate(super::user32::main_window());
        }
    }
    OK
}

fn get_visible(c: &mut Cpu) -> u32 {
    let v = graph().as_ref().is_some_and(|g| g.visible);
    mem().wr32(arg(c, 1), if v { u32::MAX } else { 0 });
    OK
}

/// SetWindowPosition(left, top, width, height) in the owner's client area.
fn set_window_position(c: &mut Cpu) -> u32 {
    let r = [arg(c, 1) as i32, arg(c, 2) as i32, arg(c, 3) as i32, arg(c, 4) as i32];
    if trace() {
        log!("video window at {r:?}");
    }
    if let Some(g) = graph().as_mut() {
        g.rect = r;
    }
    OK
}

fn get_window_position(c: &mut Cpu) -> u32 {
    let r = graph().as_ref().map_or([0; 4], |g| g.rect);
    for (k, v) in r.iter().enumerate() {
        mem().wr32(arg(c, 1 + k as u32), *v as u32);
    }
    OK
}

/// The window's size before the game places it: the video's.
fn ideal_size(c: &mut Cpu) -> u32 {
    let (w, h) = graph().as_ref().map_or((0, 0), |g| g.size);
    mem().wr32(arg(c, 1), w as u32);
    mem().wr32(arg(c, 2), h as u32);
    OK
}

/// A get_ method with nothing to tell: 0.
fn get_zero(c: &mut Cpu) -> u32 {
    mem().wr32(arg(c, 1), 0);
    OK
}

static VIDEO_WINDOW: &[Import] = &[
    ("IVideoWindow::QueryInterface", 3, query_interface),
    ("IVideoWindow::AddRef", 1, add_ref),
    ("IVideoWindow::Release", 1, release),
    ("IVideoWindow::GetTypeInfoCount", 2, type_info_count),
    ("IVideoWindow::GetTypeInfo", 4, not_impl),
    ("IVideoWindow::GetIDsOfNames", 6, not_impl),
    ("IVideoWindow::Invoke", 9, not_impl),
    ("IVideoWindow::put_Caption", 2, ok),
    ("IVideoWindow::get_Caption", 2, not_impl),
    ("IVideoWindow::put_WindowStyle", 2, ok),
    ("IVideoWindow::get_WindowStyle", 2, get_zero),
    ("IVideoWindow::put_WindowStyleEx", 2, ok),
    ("IVideoWindow::get_WindowStyleEx", 2, get_zero),
    ("IVideoWindow::put_AutoShow", 2, ok),
    ("IVideoWindow::get_AutoShow", 2, get_zero),
    ("IVideoWindow::put_WindowState", 2, ok),
    ("IVideoWindow::get_WindowState", 2, get_zero),
    ("IVideoWindow::put_BackgroundPalette", 2, ok),
    ("IVideoWindow::get_BackgroundPalette", 2, get_zero),
    ("IVideoWindow::put_Visible", 2, put_visible),
    ("IVideoWindow::get_Visible", 2, get_visible),
    ("IVideoWindow::put_Left", 2, not_impl),
    ("IVideoWindow::get_Left", 2, not_impl),
    ("IVideoWindow::put_Width", 2, not_impl),
    ("IVideoWindow::get_Width", 2, not_impl),
    ("IVideoWindow::put_Top", 2, not_impl),
    ("IVideoWindow::get_Top", 2, not_impl),
    ("IVideoWindow::put_Height", 2, not_impl),
    ("IVideoWindow::get_Height", 2, not_impl),
    ("IVideoWindow::put_Owner", 2, ok),
    ("IVideoWindow::get_Owner", 2, get_zero),
    ("IVideoWindow::put_MessageDrain", 2, ok),
    ("IVideoWindow::get_MessageDrain", 2, get_zero),
    ("IVideoWindow::get_BorderColor", 2, get_zero),
    ("IVideoWindow::put_BorderColor", 2, ok),
    ("IVideoWindow::get_FullScreenMode", 2, get_zero),
    ("IVideoWindow::put_FullScreenMode", 2, ok),
    ("IVideoWindow::SetWindowForeground", 2, ok),
    ("IVideoWindow::NotifyOwnerMessage", 5, ok),
    ("IVideoWindow::SetWindowPosition", 5, set_window_position),
    ("IVideoWindow::GetWindowPosition", 5, get_window_position),
    ("IVideoWindow::GetMinIdealImageSize", 3, ideal_size),
    ("IVideoWindow::GetMaxIdealImageSize", 3, ideal_size),
    ("IVideoWindow::GetRestorePosition", 5, get_window_position),
    ("IVideoWindow::HideCursor", 2, ok),
    ("IVideoWindow::IsCursorHidden", 2, get_zero),
];

// ---- IBasicAudio --------------------------------------------------------------------------------

fn put_volume(c: &mut Cpu) -> u32 {
    let v = arg(c, 1) as i32;
    if let Some(g) = graph().as_mut() {
        g.volume = v;
        if g.player.is_some() {
            super::dsound::stream_volume(v);
        }
    }
    OK
}

fn get_volume(c: &mut Cpu) -> u32 {
    let v = graph().as_ref().map_or(0, |g| g.volume);
    mem().wr32(arg(c, 1), v as u32);
    OK
}

static BASIC_AUDIO: &[Import] = &[
    ("IBasicAudio::QueryInterface", 3, query_interface),
    ("IBasicAudio::AddRef", 1, add_ref),
    ("IBasicAudio::Release", 1, release),
    ("IBasicAudio::GetTypeInfoCount", 2, type_info_count),
    ("IBasicAudio::GetTypeInfo", 4, not_impl),
    ("IBasicAudio::GetIDsOfNames", 6, not_impl),
    ("IBasicAudio::Invoke", 9, not_impl),
    ("IBasicAudio::put_Volume", 2, put_volume),
    ("IBasicAudio::get_Volume", 2, get_volume),
    ("IBasicAudio::put_Balance", 2, ok),
    ("IBasicAudio::get_Balance", 2, get_zero),
];

// ---- playback -----------------------------------------------------------------------------------

/// Where the video window is, as parts of the owner's client area (None: not shown).
fn placement() -> Option<[f32; 4]> {
    let g = graph();
    let g = g.as_ref()?;
    let [x, y, w, h] = g.rect;
    if !g.visible || w <= 0 || h <= 0 {
        return None;
    }
    let r = super::user32::main_rect();
    let (cw, ch) = ((r[2] - r[0]).max(1) as f32, (r[3] - r[1]).max(1) as f32);
    Some([x as f32 / cw, y as f32 / ch, w as f32 / cw, h as f32 / ch])
}

/// Play a movie until its end or `stop`; at its end, queue EC_COMPLETE and post the notify
/// message.
fn play(movie: &mpeg::Streams, stop: &AtomicBool, paused: &AtomicBool, volume: i32) {
    use super::dsound;
    let Some(mut video) = mpeg::video::Decoder::new(&movie.video) else { return };
    let mut audio = mpeg::mp2::Decoder::new(&movie.audio);
    let rate = audio.as_ref().map_or(44100, |a| a.sample_rate);
    // the sound may start a little after the picture: silence before it
    let delay = match (movie.video_start, movie.audio_start) {
        (Some(v), Some(a)) => a - v,
        _ => 0.0,
    };
    if audio.is_some() {
        dsound::stream_start(rate);
        dsound::stream_volume(volume);
        dsound::stream_push(&vec![[0.0; 2]; (delay.max(0.0) * rate as f64) as usize]);
    }
    let fps = video.fps;
    let mut rgb = Vec::new();
    let mut shown = 0u64;
    let mut video_done = false;
    let mut audio_done = audio.is_none();
    let mut clock_start = Instant::now();
    let mut ended_at = None;
    while !stop.load(Ordering::Relaxed) {
        if paused.load(Ordering::Relaxed) {
            dsound::stream_pause(true);
            let t = Instant::now();
            while paused.load(Ordering::Relaxed) && !stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(10));
            }
            clock_start += t.elapsed();
            dsound::stream_pause(false);
            continue;
        }
        // keep a quarter of a second of sound queued
        if let Some(a) = &mut audio {
            while !audio_done && dsound::stream_state().0 < rate as usize / 4 {
                match a.next_frame() {
                    Some(f) => dsound::stream_push(f),
                    None => audio_done = true,
                }
            }
        }
        // the sound's position is the clock, unless it does not move (no device)
        let wall = clock_start.elapsed().as_secs_f64();
        let clock = if audio.is_some() { (dsound::stream_state().1 as f64 / rate as f64).max(wall - 0.25) } else { wall };
        if video_done {
            // let the sound play out (at most 2 s more)
            let end = *ended_at.get_or_insert(wall);
            if audio_done && dsound::stream_state().0 == 0 || wall - end > 2.0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
            continue;
        }
        let due = shown as f64 / fps + (-delay).max(0.0);
        if clock < due {
            std::thread::sleep(Duration::from_secs_f64((due - clock).min(0.01)));
            continue;
        }
        match video.next_frame() {
            Some(f) => {
                // a frame more than one behind is skipped
                if clock - due < 1.0 / fps {
                    f.to_rgb(&mut rgb);
                    let place = placement();
                    window::set_video(place.map(|r| (rgb.clone(), (f.width & !1) as u32, (f.height & !1) as u32, r)));
                }
                shown += 1;
            }
            None => video_done = true,
        }
    }
    window::set_video(None);
    if audio.is_some() {
        dsound::stream_stop();
    }
    if trace() {
        log!("video: {shown} frames, {}", if stop.load(Ordering::Relaxed) { "stopped" } else { "complete" });
    }
    if !stop.load(Ordering::Relaxed) {
        let mut g = graph();
        if let Some(g) = g.as_mut() {
            g.events.push_back((EC_COMPLETE, 0, 0));
            let (hwnd, msg, lp) = g.notify;
            if msg != 0 {
                super::user32::post(hwnd, msg, 0, lp);
            }
        }
    }
}

pub const IMPORTS: &[Import] = &[("CoCreateInstance", 5, co_create_instance)];
