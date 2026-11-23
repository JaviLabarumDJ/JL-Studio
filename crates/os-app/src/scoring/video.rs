//! Reference video is decoded off-thread using a local FFmpeg executable.
pub(crate) mod setup;
use super::*;
use os_model::timeline::Video;
use std::{
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

pub const EXTENSIONS: &[&str] = &["mp4", "mov", "mkv", "avi", "webm", "m4v", "mpg", "mpeg"];
use ml_video_player::{HEIGHT, Picture, Request, WIDTH};
use std::collections::VecDeque;

const PAGE: u64 = 12;
type Frame = Arc<[u32]>;

type FrameResult = Result<Vec<Frame>, String>;
type Decode = (u64, u64, mpsc::Receiver<FrameResult>);
type Import = (usize, Option<Video>, mpsc::Receiver<Result<Video, String>>);

pub struct Player {
    pub open: bool,
    source: Option<(String, f64, f64)>,
    generation: u64,
    requested: Option<u64>,
    image: Option<Frame>,
    pages: VecDeque<(u64, Vec<Frame>)>,
    decode: Option<Decode>,
    import: Option<Import>,
    last: Option<Instant>,
    error: Option<String>,
    setup: Option<setup::Run>,
}

impl Default for Player {
    fn default() -> Self {
        Self {
            open: true,
            source: None,
            generation: 0,
            requested: None,
            image: None,
            pages: VecDeque::new(),
            decode: None,
            import: None,
            last: None,
            error: None,
            setup: None,
        }
    }
}

pub(super) fn tool() -> Result<PathBuf, String> {
    let custom = std::env::var_os("ML_STUDIO_FFMPEG")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute());
    let name = if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    };
    let exe = std::env::current_exe().ok();
    let beside_app = exe.as_deref().and_then(Path::parent);
    let path = std::env::var_os("PATH");
    custom
        .into_iter()
        .chain(
            beside_app
                .into_iter()
                .flat_map(|dir| [dir.join(name), dir.join("Sample Exchange").join(name)]),
        )
        .chain(setup::installed())
        .chain(
            path.as_deref()
                .into_iter()
                .flat_map(std::env::split_paths)
                .filter(|dir| dir.is_absolute())
                .map(|dir| dir.join(name)),
        )
        .find(|path| path.is_file())
        .ok_or_else(|| {
            "Right-click the Video track header and choose Set up video playback.".into()
        })
}

fn decode_command(ffmpeg: &Path) -> Command {
    let mut cmd = Command::new(ffmpeg);
    cmd.args([
        "-hide_banner",
        "-nostdin",
        "-protocol_whitelist",
        "file,pipe",
        "-format_whitelist",
        "mov,matroska,webm,avi,mpeg,mpegts",
        "-threads",
        "2",
        "-filter_threads",
        "1",
    ]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    cmd
}

fn bounded_output(cmd: &mut Command, timeout: Duration, limit: usize) -> Result<Output, String> {
    use std::io::Read;
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not start FFmpeg: {e}"))?;
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let read = |pipe: &mut dyn Read, limit: usize| {
        let mut buf = [0u8; 4096];
        let mut result = Vec::new();
        while let Ok(n) = pipe.read(&mut buf) {
            if n == 0 {
                break;
            }
            result.extend_from_slice(&buf[..n.min(limit.saturating_sub(result.len()))]);
        }
        result
    };
    let a = std::thread::spawn(move || read(&mut stdout, limit));
    let b = std::thread::spawn(move || read(&mut stderr, 64 * 1024));
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(8)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break Err("FFmpeg did not finish decoding in time.".to_string());
            }
        }
    };
    let stdout = a.join().unwrap_or_default();
    let stderr = b.join().unwrap_or_default();
    Ok(Output {
        status: status?,
        stdout,
        stderr,
    })
}

fn timestamp(s: &str) -> Option<f64> {
    let mut fields = s.split(':');
    let h = fields.next()?.parse::<f64>().ok()?;
    let m = fields.next()?.parse::<f64>().ok()?;
    let s = fields.next()?.parse::<f64>().ok()?;
    Some(h * 3600.0 + m * 60.0 + s)
}

fn probe(path: PathBuf, start: f64) -> Result<Video, String> {
    let ffmpeg = tool()?;
    let path = path
        .canonicalize()
        .map_err(|e| format!("Video file could not be opened: {e}"))?;
    let output = bounded_output(
        decode_command(&ffmpeg).arg("-i").arg(&path),
        Duration::from_secs(12),
        0,
    )?;
    let info = String::from_utf8_lossy(&output.stderr);
    let duration = info
        .lines()
        .find_map(|line| {
            let tail = line.trim_start().strip_prefix("Duration: ")?;
            timestamp(tail.split(',').next()?.trim())
        })
        .filter(|t| (0.001..=86400.0).contains(t))
        .ok_or("Could not read the video's duration.")?;
    let fps = info
        .lines()
        .filter(|l| l.contains("Video:") && !l.contains("attached pic"))
        .find_map(|line| {
            line.split(',')
                .find_map(|item| item.trim().strip_suffix(" fps")?.parse::<f64>().ok())
        })
        .filter(|t| (1.0..=240.0).contains(t))
        .ok_or("Could not read a supported video frame rate.")?;
    // FFmpeg's human-readable probe rounds common fractional frame rates.
    let fps = [
        (23.98, 24_000.0 / 1001.0),
        (29.97, 30_000.0 / 1001.0),
        (47.95, 48_000.0 / 1001.0),
        (59.94, 60_000.0 / 1001.0),
        (119.88, 120_000.0 / 1001.0),
    ]
    .into_iter()
    .find(|(rounded, _)| (fps - rounded).abs() < 1e-6)
    .map_or(fps, |(_, exact)| exact);
    let path = path
        .to_str()
        .ok_or("The video path cannot be saved as text.")?
        .to_owned();
    let video = Video {
        path,
        start: (start * fps).round() / fps,
        duration,
        fps,
        source_in: 0.0,
        source_duration: Some(duration),
    };
    frames(&video.path, 0, fps, 1)?;
    Ok(video)
}

// A bounded page amortizes process startup and leaves all decode work off the audio thread.
fn frames(path: &str, first: u64, fps: f64, count: u64) -> FrameResult {
    let ffmpeg = tool()?;
    let path = Path::new(path);
    if !path.is_absolute() || !path.is_file() {
        return Err("Reference video is missing. Right-click its clip to relink it.".into());
    }
    let sec = (first as f64 / fps - 0.000002).max(0.0);
    let filter = format!(
        "fps={fps:.9}:round=near,scale={WIDTH}:{HEIGHT}:force_original_aspect_ratio=decrease,pad={WIDTH}:{HEIGHT}:(ow-iw)/2:(oh-ih)/2,setsar=1"
    );
    let stride = WIDTH * HEIGHT * 3;
    let output = bounded_output(
        decode_command(&ffmpeg)
            .args(["-loglevel", "error", "-ss", &format!("{sec:.6}"), "-i"])
            .arg(path)
            .args([
                "-map",
                "0:V:0",
                "-frames:v",
                &count.to_string(),
                "-an",
                "-sn",
                "-dn",
                "-vf",
                &filter,
                "-pix_fmt",
                "rgb24",
                "-c:v",
                "rawvideo",
                "-threads",
                "1",
                "-f",
                "rawvideo",
                "pipe:1",
            ]),
        Duration::from_secs(12),
        stride * count as usize + 1,
    )?;
    if !output.status.success() || output.stdout.is_empty() || output.stdout.len() % stride != 0 {
        return Err("FFmpeg could not decode a frame here.".into());
    }
    Ok(output
        .stdout
        .chunks_exact(stride)
        .map(|frame| {
            frame
                .chunks_exact(3)
                .map(|p| u32::from(p[0]) << 16 | u32::from(p[1]) << 8 | u32::from(p[2]))
                .collect()
        })
        .collect())
}

pub fn snap(seconds: f64, fps: f64) -> f64 {
    (seconds * fps).round() / fps
}

fn fits(clips: &[Video], clip: &Video, except: Option<usize>) -> bool {
    clips.iter().enumerate().all(|(i, other)| {
        Some(i) == except || clip.end() <= other.start + 1e-9 || clip.start >= other.end() - 1e-9
    })
}

pub fn adjust(clips: &mut [Video], index: usize, edge: VideoEdge, delta: f64) -> bool {
    let Some(mut clip) = clips.get(index).cloned() else {
        return false;
    };
    let length = clip.source_length();
    let frame = (1.0 / clip.fps).min(clip.duration);
    match edge {
        VideoEdge::Body => clip.start = snap(clip.start + delta, clip.fps).clamp(0.0, 86400.0),
        VideoEdge::Left => {
            let previous_end = clips.get(index.wrapping_sub(1)).map_or(0.0, Video::end);
            let lower = (-clip.source_in).max(previous_end - clip.start).min(0.0);
            let offset = snap(clip.source_in + delta, clip.fps) - clip.source_in;
            let offset = offset.clamp(lower, (clip.duration - frame).max(0.0));
            clip.start += offset;
            clip.source_in += offset;
            clip.duration -= offset;
        }
        VideoEdge::Right => {
            let room = clips
                .get(index + 1)
                .map_or(length, |next| next.start - clip.start);
            let extent = (length - clip.source_in).min(room).max(frame);
            let out = snap(clip.source_in + clip.duration + delta, clip.fps);
            clip.duration = (out - clip.source_in).clamp(frame, extent);
        }
    }
    clip.source_duration = Some(length);
    if !fits(clips, &clip, Some(index)) || clip.start > 86400.0 {
        return false;
    }
    clips[index] = clip;
    true
}

pub fn split(s: &mut State, index: usize, seconds: f64) {
    let Some(clip) = data(s).videos.get(index) else {
        return;
    };
    let cut = snap(clip.source_in + seconds - clip.start, clip.fps) - clip.source_in;
    let frame = 1.0 / clip.fps;
    if cut < frame - 1e-9 || clip.duration - cut < frame - 1e-9 {
        return;
    }
    let mut right = clip.clone();
    let length = Some(clip.source_length());
    right.start += cut;
    right.source_in += cut;
    right.duration -= cut;
    right.source_duration = length;
    update(s, "Split video clip", |timeline| {
        let left = &mut timeline.videos[index];
        left.duration = cut;
        left.source_duration = length;
        timeline.videos.insert(index + 1, right);
    });
}

pub fn import(app: &mut App, path: PathBuf, start: f64, relink: Option<Video>) {
    let s = &mut app.state;
    if !(0.0..=86400.0).contains(&start) {
        s.message = Some("Choose a video start within the first 24 hours.".into());
        app.invalidate();
        return;
    }
    if s.playlist.scoring.video.import.is_some() {
        s.message = Some("Wait for the current video import to finish.".into());
        app.invalidate();
        return;
    }
    let arrangement = s.project.arrangement_index();
    if tool().is_err() {
        setup::offer(
            app,
            Some(setup::ImportFile {
                arrangement,
                path,
                start,
                relink,
            }),
        );
        return;
    }
    let (tx, rx) = mpsc::sync_channel(1);
    s.playlist.scoring.video.import = Some((arrangement, relink, rx));
    s.message = Some("Reading reference video...".into());
    std::thread::spawn(move || {
        let _ = tx.send(probe(path, start));
    });
    app.invalidate();
}

fn finish_import(
    s: &mut State,
    mut video: Video,
    relink: Option<Video>,
) -> Result<&'static str, &'static str> {
    let mut timeline = data(s).clone();
    let label = if let Some(original) = relink {
        let Some(i) = timeline.videos.iter().position(|v| *v == original) else {
            return Err("The video clip changed while the replacement was loading.");
        };
        if video.source_length() + 1e-9 < original.source_in + original.duration {
            return Err("The replacement video is too short for this clip.");
        }
        video.start = original.start;
        video.source_in = original.source_in;
        video.duration = original.duration;
        timeline.videos[i] = video;
        "Relink video clip"
    } else {
        if !fits(&timeline.videos, &video, None) {
            return Err("The video would overlap another clip. Import it at an open position.");
        }
        timeline.videos.push(video);
        "Import video"
    };
    timeline.normalize();
    apply(s, label, timeline);
    s.playlist.scoring.video.open = true;
    Lane::Video.show(true);
    Ok(if label == "Relink video clip" {
        "Video clip relinked."
    } else {
        "Reference video added to the Playlist."
    })
}

fn position(app: &App) -> f64 {
    let s = &app.state;
    if app.transport.song() && app.transport.state() != crate::transport::State::Stopped {
        app.transport.seconds()
    } else {
        s.project
            .tempo_map(s.tempo)
            .seconds(f64::from(s.playlist.pos))
    }
}

fn player_controls(app: &mut App) {
    for cmd in ml_video_player::take_requests() {
        let now = position(app);
        match cmd {
            Request::Import => {
                super::run(app, super::Command::ImportVideoAt(now));
            }
            Request::Toggle => {
                if app.transport.playing() || app.transport.song() {
                    app.state.song_mode = true;
                    app.act(Action::Play);
                } else {
                    app.play_song_from(app.state.playlist.pos);
                }
            }
            Request::Step(_) | Request::Seek(_) => {
                let s = &app.state;
                let target = match cmd {
                    Request::Step(delta) => {
                        let clip = data(s).videos.iter().find(|v| v.source_time(now).is_some());
                        let (fps, origin) =
                            clip.map_or((30.0, 0.0), |v| (v.fps, v.start - v.source_in));
                        // Pick the center so rounding to a Playlist tick stays on the intended frame.
                        origin
                            + (((now - origin) * fps + 1e-5).floor() + f64::from(delta) + 0.5) / fps
                    }
                    Request::Seek(target) => target,
                    _ => unreachable!(),
                };
                let tick = s.project.tempo_map(s.tempo).tick(target.max(0.0)).round() as u32;
                if app.transport.state() == crate::transport::State::Paused {
                    app.transport.stop();
                }
                super::seek(app, tick);
                app.invalidate();
            }
        }
    }
}

pub fn poll(app: &mut App) {
    setup::poll(app);
    player_controls(app);
    let s = &mut app.state;
    let player = &mut s.playlist.scoring.video;
    let result = player
        .import
        .as_ref()
        .and_then(|(_, _, rx)| match rx.try_recv() {
            Ok(v) => Some(v),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(Err("Video import stopped.".into())),
        });
    if let Some(result) = result {
        let (arrangement, relink, _) = player.import.take().unwrap();
        if s.project.arrangement_index() == arrangement {
            let message = match result {
                Ok(video) => match finish_import(s, video, relink) {
                    Ok(ok) | Err(ok) => ok.to_owned(),
                },
                Err(error) => error,
            };
            s.message = Some(message);
        }
        app.invalidate();
    }
    refresh_picture(app, ml_video_player::monitoring());
}

fn refresh_picture(app: &mut App, monitor: bool) {
    let s = &app.state;
    let overlay = overlay_visible(s) && s.shown(Win::Playlist);
    let visible = overlay || monitor;
    let playing = app.transport.playing() && app.transport.song();
    let seconds = if visible { position(app) } else { 0.0 };
    let s = &mut app.state;
    let clips = s
        .project
        .arrangement()
        .map_or(&[][..], |a| &a.timeline.videos[..]);
    let end = clips.last().map_or(0.0, Video::end);
    let active = visible
        .then(|| {
            clips
                .iter()
                .rev()
                .find_map(|v| v.source_time(seconds).map(|t| (v, t)))
        })
        .flatten();
    let player = &mut s.playlist.scoring.video;
    let same = match (&player.source, &active) {
        (None, None) => true,
        (Some((path, start, offset)), Some((v, _))) => {
            *path == v.path && *start == v.start && *offset == v.source_in
        }
        _ => false,
    };
    let mut repaint = !same;
    if !same {
        player.source = active.map(|(v, _)| (v.path.clone(), v.start, v.source_in));
        player.generation = player.generation.wrapping_add(1);
        player.pages.clear();
        player.requested = None;
        player.image = None;
        player.error = None;
        player.last = None;
    }
    if let Some((generation, first, rx)) = &player.decode {
        match rx.try_recv() {
            Ok(result) => {
                if *generation == player.generation {
                    match result {
                        Ok(page) => {
                            if player.pages.len() >= 3 {
                                player.pages.pop_front();
                            }
                            player.pages.push_back((*first, page));
                            player.error = None;
                        }
                        Err(error) => {
                            player.error = Some(error);
                            player.last = Some(Instant::now());
                        }
                    }
                    repaint = true;
                }
                player.decode = None;
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                player.decode = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }
    if let Some((v, local)) = active {
        let target = (local * v.fps + 1e-5).floor() as u64;
        let first = target / PAGE * PAGE;
        let page = player.pages.iter().find(|(start, _)| *start == first);
        // Container durations can extend past the final video sample. Hold its last picture.
        let image = page
            .and_then(|(_, frames)| {
                frames
                    .get((target - first) as usize)
                    .or_else(|| frames.last())
            })
            .cloned();
        if player.requested != Some(target) || (player.image.is_none() && image.is_some()) {
            player.requested = Some(target);
            player.image = image;
            repaint = true;
        }
        let wanted = if page.is_none() {
            Some(first)
        } else if playing
            && ((first + PAGE) as f64 / v.fps) < v.source_in + v.duration
            && !player.pages.iter().any(|(start, _)| *start == first + PAGE)
        {
            Some(first + PAGE)
        } else {
            None
        };
        if let Some(first) = wanted.filter(|_| {
            player.decode.is_none()
                && (player.error.is_none()
                    || player
                        .last
                        .is_none_or(|t| t.elapsed() >= Duration::from_secs(2)))
        }) {
            let (tx, rx) = mpsc::sync_channel(1);
            player.decode = Some((player.generation, first, rx));
            let path = v.path.clone();
            let fps = v.fps;
            std::thread::spawn(move || {
                let _ = tx.send(frames(&path, first, fps, PAGE));
            });
        }
    }
    if monitor {
        let message = if let Some(error) = &player.error {
            error.as_str()
        } else if end == 0.0 {
            "Open a video to add reference picture"
        } else if active.is_none() {
            "No video at this position"
        } else {
            "Reading video…"
        };
        ml_video_player::publish(Picture {
            pixels: player.image.clone(),
            seconds,
            end,
            playing,
            title: active
                .and_then(|(v, _)| Path::new(&v.path).file_name())
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
            fps: active.map_or(0.0, |(v, _)| v.fps),
            message: message.to_owned(),
        });
    }
    if repaint {
        app.damage_win(Win::Playlist);
    }
}

pub fn overlay_visible(s: &State) -> bool {
    Lane::Video.shown(s) && s.playlist.scoring.video.open && !data(s).videos.is_empty()
}

pub fn paint(ui: &mut Ui, s: &State, grid: Rect) {
    let player = &s.playlist.scoring.video;
    if !overlay_visible(s) || grid.w < 190.0 || grid.h < 150.0 {
        return;
    }
    let w = (grid.w * 0.36)
        .clamp(178.0, 416.0)
        .min((grid.h - 45.0) * 16.0 / 9.0);
    let h = w * 9.0 / 16.0;
    let image = Rect::new(grid.right() - w - 12.0, grid.bottom() - h - 12.0, w, h);
    let title = Rect::new(image.x, image.y - 23.0, w, 23.0);
    ui.hit(
        Rect::new(title.x, title.y, w, h + 23.0),
        Action::Swallow,
        "Reference video preview",
    );
    ui.c.fill_rect(title, ui.t.track_header);
    ui.c.fill_rect(image, os_ui::Color::rgb(0x121419));
    ui.c.text_clip(
        Font::new(12),
        title.x + 7.0,
        title.y + 4.0,
        title.w - 34.0,
        ui.t.text,
        "Reference video",
    );
    let close = Rect::new(title.right() - 23.0, title.y, 23.0, 23.0);
    ui.hit(close, Action::ScoringVideoClose, "Hide video preview");
    ui.c.text_center(Font::new(13), close, ui.t.text, "x");
    if let Some(pixels) = &player.image {
        ui.c.image(image, WIDTH, HEIGHT, pixels);
    } else if let Some(error) = &player.error {
        ui.c.text_clip(
            Font::new(12),
            image.x + 8.0,
            image.cy(),
            image.w - 16.0,
            ui.t.text,
            error,
        );
    } else {
        ui.c.text_center(
            Font::new(12),
            image,
            ui.t.text_dim,
            "Seek inside the video clip",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_reference_frame_when_fixture_is_supplied() {
        let Some(path) = std::env::var_os("ML_TEST_REFERENCE_VIDEO") else {
            return;
        };
        let video = probe(path.into(), 2.0).unwrap();
        assert_eq!(video.start, 2.0);
        assert_eq!(video.fps, 24.0);
        assert_ne!(
            frames(&video.path, 0, video.fps, 1).unwrap(),
            frames(&video.path, 30, video.fps, 1).unwrap()
        );
    }

    fn frame_id(image: &Frame) -> usize {
        (0..6).fold(0, |id, bit| {
            let x = (bit * 2 + 1) * WIDTH / 12;
            id | (usize::from(image[HEIGHT / 2 * WIDTH + x] & 255 > 128) << bit)
        })
    }

    #[test]
    fn video_player_numbered_frames_and_transport() {
        let Some(path) = std::env::var_os("ML_TEST_NUMBERED_VIDEO") else {
            return;
        };
        let mut video = probe(path.into(), 0.0).unwrap();
        for first in [0, 24, 12, 36, 0] {
            let batch = frames(&video.path, first, video.fps, PAGE).unwrap();
            assert_eq!(batch.len(), PAGE as usize);
            for (offset, image) in batch.iter().enumerate() {
                assert_eq!(frame_id(image), first as usize + offset);
            }
        }
        let mut app = App::new(None);
        app.state.project = crate::app::test_project();
        app.state.tempo = 120.;
        video.source_in = 0.25;
        video.duration = 1.5;
        finish_import(&mut app.state, video, None).unwrap();
        app.state.playlist.scoring.video.open = false;
        fn expect(app: &mut App, number: usize) {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                refresh_picture(app, true);
                if let Some(image) = &app.state.playlist.scoring.video.image {
                    assert_eq!(frame_id(image), number);
                    return;
                }
                assert!(
                    Instant::now() < deadline,
                    "timed out waiting for frame {number}"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        expect(&mut app, 6);
        app.transport.pretend_at(96., true);
        for (seconds, number) in [(0.5, 18), (1., 30), (0.5, 18)] {
            app.transport.pretend_seconds(seconds);
            expect(&mut app, number);
        }
        app.transport.pause();
        expect(&mut app, 18);
        app.transport.stop();
        expect(&mut app, 6);
        let tempo = app
            .state
            .project
            .tempo_map(120.)
            .warp(&[], 96, 1.0)
            .unwrap();
        update(&mut app.state, "Warp tempo", |map| map.tempo = tempo);
        app.state.playlist.pos = 96;
        expect(&mut app, 30);
        app.state.playlist.pos = 5000;
        refresh_picture(&mut app, true);
        assert!(app.state.playlist.scoring.video.image.is_none());
    }
}
