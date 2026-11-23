// Copyright (C) 2026 Lemon Studio contributors
// SPDX-License-Identifier: GPL-3.0-or-later
//! Browser-only audition state; it never seeks or changes the user's song.
use std::sync::{Arc,Mutex};
use std::time::{Duration,Instant};
use os_ui::{Rect,Font};
use crate::app::{App,State,Ui,Action};
use crate::views::browser as view;
pub const HEIGHT:f32=130.;
const VOLUME_KEY: &str = "browser_preview_boost";
const MAX_DB: f32 = 18.;
type Loaded=Result<(Arc<os_engine::sample::Sample>,Vec<(f32,f32)>),String>;
#[derive(Default)]
pub struct Preview {
    pub path:Option<String>,
    sample:Option<Arc<os_engine::sample::Sample>>,
    peaks:Vec<(f32,f32)>,
    pending:Option<Arc<Mutex<Option<Loaded>>>>,
    pub duration:f64,
    pub position:f64,
    offset:f64,
    playing:bool,
    started:bool,
    requested:Option<Instant>,
    error:Option<String>,
    resume:bool,
}
impl Preview {pub fn busy(&self)->bool {self.pending.is_some()||self.playing}}
pub fn visible(s:&State)->bool {crate::workspace_visibility::shown("visible_browser-preview") && (matches!(s.browser_tab,0|3|4) || view::library::library_tab(s.browser_tab))&&view::opt(s,view::OPT_PANEL)&&s.browser_preview.path.is_some()}
fn overview(sample:&os_engine::sample::Sample)->Vec<(f32,f32)> {
    let step=sample.frames().div_ceil(512).max(1);
    sample.left.chunks(step).zip(sample.right.chunks(step)).map(|(l,r)|l.iter().zip(r).fold((0f32,0f32),|(lo,hi),(&a,&b)|(lo.min(a).min(b),hi.max(a).max(b)))).collect()
}
pub fn dismiss(app:&mut App) {
    if app.state.browser_preview.path.is_some() {
        app.transport.stop_preview();
        app.state.browser_preview=Preview::default();
        app.invalidate();
    }
}
pub fn select(app:&mut App,path:&str) {
    app.state.playlist.picker_ui.stop_previews();
    app.transport.stop_preview();
    let result=Arc::new(Mutex::new(None));
    app.state.browser_preview=Preview{resume:true,path:Some(path.into()),pending:Some(result.clone()),..Default::default()};
    let path=crate::plugins::resolve_sample(path);
    let (normal,reversed,trim)=(normalize(),reverse(),auto_trim());
    std::thread::spawn(move||{
        let loaded = os_engine::sample::load_shared(std::path::Path::new(&path))
            .map(|sample| {
                let sample = processed(sample,normal,reversed,trim);
                let peaks = overview(&sample);
                (sample,peaks)
            })
            .map_err(|e| format!("Cannot preview: {e}"));
        if let Ok(mut out)=result.lock(){*out=Some(loaded);}
    });
    app.invalidate();
}
pub(crate) fn project(path:&str,duration:f64,at:f64,tempo:f64)->os_model::Project {
    let mut p=crate::app::preview_project(path);
    p.tempo=Some(tempo);
    p.channels[0].kind=os_model::ChannelKind::AudioClip;
    p.channels[0].sound.sampler_flags=Some(os_model::ChannelSound::AUDIO_CLIP_FLAGS);
    let length=((duration-at).max(0.01)*tempo/60.*f64::from(p.ppq)).ceil().min(u32::MAX as f64) as u32;
    let mut clip=os_model::Clip::new(0,length,0,os_model::ClipSource::Channel(0));
    clip.window=Some((at*1000.,duration*1000.));
    p.patterns.clear();
    p.arrangements=vec![os_model::Arrangement{clips:vec![clip],..Default::default()}];

    p
}
fn processed(sample:Arc<os_engine::Sample>,normal:bool,reversed:bool,trim:bool)->Arc<os_engine::Sample>{
    if !normal&&!reversed&&!trim{return sample;}
    let mut copy=(*sample).clone();
    if trim {let (a,b)=trim_bounds(&copy);copy.left=copy.left[a..b].to_vec();copy.right=copy.right[a..b].to_vec();}
    let peak=copy.left.iter().chain(&copy.right).filter(|v|v.is_finite()).fold(0f32,|a,v|a.max(v.abs()));
    let gain=if normal&&peak>1e-9{1./peak}else{1.};
    for side in [&mut copy.left,&mut copy.right]{
        for v in side.iter_mut(){*v=if v.is_finite(){*v*gain}else{0.};}
        if reversed{side.reverse();}
    }
    Arc::new(copy)
}
pub fn retrigger(app: &mut App, path: &str) {
    if app.state.browser_preview.path.as_deref() == Some(path) && app.state.browser_preview.sample.is_some() {
        app.transport.stop_preview();
        app.state.browser_preview.position = 0.0;
        play(app);
    } else if app.state.browser_preview.path.as_deref() == Some(path) && app.state.browser_preview.pending.is_some() {
        app.state.browser_preview.resume = true;
        app.state.browser_preview.position = 0.0;
    } else {
        select(app, path);
    }
}

fn play(app:&mut App) {
    let preview=&mut app.state.browser_preview;
    preview.resume=true;
    let Some(sample) = preview.sample.clone().filter(|_| preview.path.is_some()) else { return };
    if preview.position>=preview.duration {preview.position=0.;}
    preview.offset=preview.position;
    preview.playing=true;preview.started=false;preview.requested=Some(Instant::now());
    let position = preview.position;
    let track = crate::preview_track::destination(&app.state.project, crate::prefs::number(&crate::prefs::BROWSER_PREVIEW_TRACK));
    let limit = (!view::opt(&app.state, view::OPT_PREVIEW)).then_some(crate::browser::PREVIEW_LIMIT);
    app.transport.set_browser_preview_gain(volume_gain(app.state.browser_preview_db));
    app.transport.preview_browser_sample(crate::rack::play_project(&app.state), track, position, limit, sample, clip_on());
}
pub fn tick(app:&mut App) {
    let result=app.state.browser_preview.pending.as_ref().and_then(|p|p.lock().ok()?.take());
    if let Some(result)=result {
        let preview=&mut app.state.browser_preview;
        preview.pending=None;
        match result {
            Ok((sample,peaks))=>{preview.duration=sample.frames() as f64/f64::from(sample.rate.max(1));preview.sample=Some(sample);preview.peaks=peaks;if preview.resume{play(app);}}
            Err(e)=>{preview.error=Some(e);preview.playing=false;}
        }
        app.invalidate();
    }
    let (active,seconds)=app.transport.browser_preview_status();
    let preview=&mut app.state.browser_preview;
    if preview.playing {
        if active {preview.started=true;preview.position=(preview.offset+seconds).min(preview.duration);}
        else if preview.started || seconds > 0.0 {
            preview.position = (preview.offset + seconds).min(preview.duration);
            preview.playing = false;
        }
        else if preview.requested.is_some_and(|t|t.elapsed()>Duration::from_secs(5)) {preview.playing=false;preview.error=Some("Preview could not start; check audio settings".into());}
        if visible(&app.state) {let r=panel_rect(&app.state);app.damage_rect(r);}
    }
}
// Roughly -90 dBFS: remove near-silent ends without cutting normal release tails.
pub const AUTO_TRIM_THRESHOLD:u32=1;
pub fn auto_trim()->bool {crate::prefs::text("browser_auto_trim").as_deref()==Some("1")}
pub(crate) fn trim_bounds(s:&os_engine::Sample)->(usize,usize){
    let n=s.frames().min(s.right.len());
    let audible=|i:usize| s.left[i].abs().max(s.right[i].abs())>=AUTO_TRIM_THRESHOLD as f32/32768.;
    let Some(a)=(0..n).find(|&i|audible(i)) else{return (0,n)};
    let b=(a..n).rfind(|&i|audible(i)).unwrap()+1;(a,b)
}
pub fn import_window(path:&str)->Option<(f64,f64)>{
    if !auto_trim(){return None;}
    let sample=os_engine::sample::load_shared(std::path::Path::new(&crate::plugins::resolve_sample(path))).ok()?;
    let (a,b)=trim_bounds(&sample);
    if a==0&&b==sample.frames(){return None;}
    let ms=1000./f64::from(sample.rate.max(1));Some((a as f64*ms,b as f64*ms))
}
fn volume_gain(db: f32) -> f32 { 10f32.powf(db / 20.) }

pub fn saved_volume() -> f32 {
    crate::prefs::text(VOLUME_KEY)
        .and_then(|v| v.parse::<f32>().ok())
        .filter(|db| db.is_finite())
        .unwrap_or(0.)
        .clamp(0., MAX_DB)
}

pub fn save_volume(s: &State) {
    if s.browser_preview_db != saved_volume() {
        crate::prefs::set_text(VOLUME_KEY, &s.browser_preview_db.to_string());
    }
}

fn volume_rect(panel: Rect) -> Rect {
    Rect::new(panel.x + 14., panel.y + 105., (panel.w - 116.).clamp(54., 128.), 21.)
}

fn volume_rail(slider: Rect) -> Rect {
    Rect::new(slider.x + 5., slider.cy() - 2., slider.w - 10., 4.)
}

pub fn drag_volume(app: &mut App, rail: Rect) {
    let position = ((app.mouse.0 - rail.x) / rail.w).clamp(0., 1.);
    let db = (position * MAX_DB * 10.).round() / 10.;
    if app.state.browser_preview_db != db {
        app.state.browser_preview_db = db;
        app.transport.set_browser_preview_gain(volume_gain(db));
        app.damage_rect(panel_rect(&app.state));
    }
}

fn clip_on()->bool{crate::prefs::text("browser_preview_clip").as_deref()==Some("1")}
pub fn normalize()->bool {crate::prefs::text("browser_preview_normalize").as_deref()==Some("1")}
pub fn reverse()->bool {crate::prefs::text("browser_preview_reverse").as_deref()==Some("1")}
pub fn act(app:&mut App,cmd:u8) {
    match cmd {
        0=>{if app.state.browser_preview.playing {app.transport.stop_preview();app.state.browser_preview.playing=false;} else {play(app);}},
        1=>{app.transport.stop_preview();let p=&mut app.state.browser_preview;p.playing=false;p.resume=false;p.position=0.;p.pending=None;},
        2=>{
            let r=wave_rect(panel_rect(&app.state));
            let p=&mut app.state.browser_preview;
            p.position=f64::from(((app.mouse.0-r.x)/r.w).clamp(0.,1.))*p.duration;
            play(app);
        },
        3..=5 | 7=>{
            if cmd==7 {crate::prefs::set_text("browser_preview_clip",if clip_on(){"0"}else{"1"});}else{
            let key=match cmd {3=>"browser_preview_normalize",4=>"browser_preview_reverse",_=>"browser_auto_trim"};
            let on=match cmd {3=>normalize(),4=>reverse(),_=>auto_trim()};
            crate::prefs::set_text(key,if on {"0"}else{"1"});}
            if let Some(path)=app.state.browser_preview.path.clone(){
                let position=app.state.browser_preview.position;let resume=app.state.browser_preview.playing || (app.state.browser_preview.pending.is_some() && app.state.browser_preview.resume);
                select(app,&path);app.state.browser_preview.position=position;app.state.browser_preview.resume=resume;
            }
        },
        _=>{},
    }
    app.invalidate();
}
fn panel_rect(s: &State) -> Rect {
    let r = crate::views::browser_content(s);
    let inset = if view::library::enabled() { view::library::content_inset(s) } else { 0. };
    Rect::new(r.x + inset, r.bottom() - view::FOOTER_H - HEIGHT, r.w - inset, HEIGHT)
}

#[test]
fn dock_browser_preview_seek_matches_waveform_on_both_sides() {
    let mut app=App::new(None);
    let mut c=os_ui::Canvas::new(1280,800,1.).unwrap();
    app.state.show_browser=true;
    app.state.browser_tab=0;
    app.state.browser_opts.insert(view::OPT_PANEL);
    app.state.browser_preview.path=Some("synthetic-preview.wav".into());
    app.state.browser_preview.duration=2.;
    app.state.browser_preview.peaks=vec![(-0.1,0.1);100];
    for right in [false,true] {
        if right {app.state.browser_opts.insert(view::OPT_RIGHT);} else {app.state.browser_opts.remove(view::OPT_RIGHT);}
        app.paint(&mut c,None);
        let wave=app.hits.iter().find(|h|h.action==Action::BrowserPreview(2)).unwrap().rect;
        app.mouse=(wave.cx(),wave.cy());
        act(&mut app,2);
        assert!((app.state.browser_preview.position-1.).abs()<0.01,"seeking must match the painted waveform on either side");
    }
}
fn wave_rect(r:Rect)->Rect {Rect::new(r.x+8.,r.y+25.,r.w-16.,48.)}
fn time(seconds:f64)->String {let n=seconds.max(0.) as u64;format!("{}:{:02}",n/60,n%60)}
pub fn paint(ui:&mut Ui,s:&State,r:Rect) {
    let p=&s.browser_preview;let t=ui.t;
    ui.c.fill_rect(r,t.browser_tabs);ui.hit(r,Action::Swallow,"");
    let title=p.path.as_ref().and_then(|p|std::path::Path::new(p).file_name()).map(|n|n.to_string_lossy().into_owned()).unwrap_or_else(||"Select audio to preview".into());
    let time_label = format!("{} / {}", time(p.position), time(p.duration));
    let time_font = Font::new(11);
    let time_width = ui.c.text_width(time_font, &time_label);
    ui.c.text_right(time_font, r.right() - 8., r.y + 5., t.text_dim, &time_label);
    ui.c.text_clip(Font::new(12),r.x+14.,r.y+4.,(r.w-30.-time_width).max(0.),t.text,&title);
    let wave=wave_rect(r);ui.c.fill_rrect(wave,3.,t.sunken);
    let plot = wave.inset(2.);
    let columns = plot.w.floor() as usize;
    if !p.peaks.is_empty() && columns > 0 {
        let gain = volume_gain(s.browser_preview_db);
        ui.c.push_clip(wave);
        for column in 0..columns {
            let first = column * p.peaks.len() / columns;
            let end = ((column + 1) * p.peaks.len() / columns).max(first + 1);
            let (lo,hi)=p.peaks[first..end].iter().fold((0f32,0f32),|(lo,hi),&(a,b)|(lo.min(a),hi.max(b)));
            let x = plot.x + column as f32;
            let top = wave.cy() - (hi * gain).min(1.) * plot.h * 0.5;
            let bottom = wave.cy() - (lo * gain).max(-1.) * plot.h * 0.5;
            ui.c.line(x, top, x, bottom, 1., t.accent);
        }
        let x = (wave.x + (p.position / p.duration.max(0.001)) as f32 * wave.w).clamp(plot.x, plot.right());
        ui.c.line(x,plot.y,x,plot.bottom(),1.,t.text);
        ui.c.pop_clip();
        ui.hit(wave,Action::BrowserPreview(2),"Click waveform to preview from here");
    } else {ui.c.text_center(Font::new(11),wave,t.text_dim,if p.pending.is_some(){"Loading audio..."}else{p.error.as_deref().unwrap_or("Audio preview")});}
    for (i,label) in [(0,if p.playing {"Pause"}else{"Play"}),(1,"Stop")] {
        let b=Rect::new(r.x+8.+i as f32*36.,r.y+80.,32.,23.);
        let hover=ui.hit(b,Action::BrowserPreview(i),label);
        ui.c.fill_rrect(b,3.,if hover {t.button_hi}else{t.sunken});ui.c.text_center(Font::new(11),b,t.text,label);
    }
    for (cmd,label,on,x,w,hint) in [(3,"Norm",normalize(),80.,36.,"Auto Normalize: preview only, peak target 0 dBFS"),(4,"Rev",reverse(),120.,28.,"Auto Reverse: preview only"),(5,"Trim",auto_trim(),152.,42.,"Auto-trim silence: previews and newly added samples; original files unchanged")] {
        let b=Rect::new(r.x+x,r.y+80.,w,23.);
        let hover=ui.hit(b,Action::BrowserPreview(cmd),hint);
        ui.c.fill_rrect(b,3.,if on{t.accent}else if hover{t.button_hi}else{t.sunken});
        ui.c.text_center(Font::new(11),b,if on{t.sunken}else{t.text},label);
    }
    let slider = volume_rect(r);
    let rail = volume_rail(slider);
    let db = s.browser_preview_db;
    let label = if db == 0. { "0.0 dB".into() } else { format!("+{db:.1} dB") };
    let hover = ui.hit(slider, Action::BrowserPreviewVolume(rail), format!("Preview volume: {label} · drag to adjust"));
    let thumb = rail.x + rail.w * db / MAX_DB;
    ui.c.fill_rrect(rail, 2., t.sunken);
    ui.c.fill_rrect(Rect::new(rail.x, rail.y, thumb - rail.x, rail.h), 2., t.accent);
    ui.c.fill_rrect(Rect::new(thumb - 4., rail.cy() - 6., 8., 12.), 2., if hover { t.text } else { t.accent });
    ui.c.text(Font::new(11), slider.right() + 2., r.y + 109., t.text, &label);
    let clip = Rect::new(slider.right() + 52., slider.y, 34., slider.h);
    let hover = ui.hit(clip,Action::BrowserPreview(7),"Clip boosted preview peaks");
    let on = clip_on();
    ui.c.fill_rrect(clip,3.,if on{t.accent}else if hover{t.button_hi}else{t.sunken});
    ui.c.text_center(Font::new(11),clip,if on{t.sunken}else{t.text},"Clip");
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_preview_volume_drag_saves_and_contains_loud_waveforms() {
        let previous = crate::prefs::text(VOLUME_KEY).unwrap_or_default();
        let previous_visibility = crate::prefs::text("visible_browser-preview").unwrap_or_default();
        crate::prefs::set_text(VOLUME_KEY, "6");
        crate::prefs::set_text("visible_browser-preview", "1");
        let mut app = App::new(None);
        assert_eq!(app.state.browser_preview_db, 6.);
        app.state.show_browser = true;
        app.state.browser_tab = 0;
        app.state.browser_opts.insert(view::OPT_PANEL);
        app.state.browser_preview.path = Some("loud-peak.wav".into());
        app.state.browser_preview.peaks = vec![(-0.4, 0.45); 512];
        app.state.browser_preview.duration = 3.;
        app.state.browser_preview.position = 2.;

        for (detached, right, scale) in [(false, false, 1.), (false, true, 1.25), (true, false, 1.25)] {
            app.state.native_view = detached.then_some(crate::app::Win::Browser);
            if right {
                app.state.browser_opts.insert(view::OPT_RIGHT);
            } else {
                app.state.browser_opts.remove(view::OPT_RIGHT);
            }
            let width = if detached { 320 } else { 1280 };
            let mut canvas = os_ui::Canvas::new((width as f32 * scale) as u32, (800. * scale) as u32, scale).unwrap();
            app.paint(&mut canvas, None);
            let rail = app.hits.iter().find_map(|h| match h.action {
                Action::BrowserPreviewVolume(rail) => Some(rail),
                _ => None,
            }).unwrap();

            app.mouse_move(rail.x, rail.cy());
            app.mouse_down();
            assert_eq!(app.state.browser_preview_db, 0.);
            app.paint(&mut canvas, None);
            let unboosted = canvas.pixmap.data().to_vec();
            app.mouse_move(rail.cx() + rail.w * 0.07, rail.cy());
            assert_eq!(app.state.browser_preview_db, 10.3, "the middle of a drag uses fractional dB");
            app.mouse_move(rail.right() + 15., rail.cy());
            assert_eq!(app.state.browser_preview_db, 18.);
            assert_ne!(saved_volume(), 18., "preferences are written only after release");
            app.mouse_up();
            assert_eq!(saved_volume(), 18.);
            assert_eq!(app.state.browser_preview.position, 2.);
            app.paint(&mut canvas, None);

            let panel = panel_rect(&app.state);
            let wave = wave_rect(panel);
            let stride = canvas.pixmap.width() as usize * 4;
            let mut changed_in_wave = false;
            for y in (panel.y * scale).ceil() as usize..((panel.y + 79.) * scale).floor() as usize {
                for x in (wave.x * scale).ceil() as usize..(wave.right() * scale).floor() as usize {
                    let offset = y * stride + x * 4;
                    if unboosted[offset..offset + 4] != canvas.pixmap.data()[offset..offset + 4] {
                        assert!(y >= (wave.y * scale).floor() as usize && y < (wave.bottom() * scale).ceil() as usize,
                            "boosted waveform pixel escaped the viewer at {x},{y} ({scale}x)");
                        changed_in_wave = true;
                    }
                }
            }
            assert!(changed_in_wave, "boosting repaints the waveform");
            crate::prefs::set_text(VOLUME_KEY, "6");
        }

        crate::prefs::set_text("visible_browser-preview", "0");
        let mut canvas = os_ui::Canvas::new(340, 800, 1.).unwrap();
        app.paint(&mut canvas, None);
        assert!(!app.hits.iter().any(|h| matches!(h.action, Action::BrowserPreviewVolume(_))));
        crate::prefs::set_text("visible_browser-preview", &previous_visibility);
        crate::prefs::set_text(VOLUME_KEY, &previous);
    }

    #[test] fn auto_trim_preview_and_new_imports_preserve_originals(){
        crate::prefs::set_text("browser_auto_trim","0");assert!(!auto_trim());
        let path=std::env::temp_dir().join(format!("lemon-auto-trim-{}.wav",std::process::id()));
        let rate=8000;let mut audio=vec![0.;16000];
        // Only the right channel sounds; its quiet tail must survive too.
        for i in 2000..4000 {audio[i*2+1]=if i<3900 {0.2}else{0.0001};}
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&path).unwrap(),rate,&audio).unwrap();
        let original=std::fs::read(&path).unwrap();let text=path.to_string_lossy();
        let sample=os_engine::sample::load_shared(&path).unwrap();
        assert_eq!(processed(sample.clone(),false,false,false).frames(),8000);
        let trimmed=processed(sample.clone(),false,false,true);assert_eq!(trimmed.frames(),2000);assert_eq!(trimmed.right[1999],0.0001);
        assert_eq!(sample.frames(),8000);assert_eq!(trim_bounds(&os_engine::Sample{rate,left:vec![0.;20],right:vec![0.;20]}),(0,20));
        let mut app=App::new(None);app.state.project=crate::app::test_project();
        let old=crate::playlist::add_audio_clip(&mut app.state,&text,0,0).unwrap();
        let old_clip=crate::playlist::arr_mut(&mut app.state).clips[old].clone();
        let old_trim=app.state.project.channels[0].sound.fx.trim;
        act(&mut app,5);assert!(auto_trim());assert_eq!(crate::prefs::text("browser_auto_trim").as_deref(),Some("1"));
        assert_eq!(app.state.project.channels[0].sound.fx.trim,old_trim);
        let new=crate::playlist::add_audio_clip(&mut app.state,&text,1,0).unwrap();
        assert_eq!(crate::playlist::arr_mut(&mut app.state).clips[new].window,Some((250.,500.)));
        assert_eq!(crate::playlist::arr_mut(&mut app.state).clips[old],old_clip);
        app.sample_to_channel(text.to_string(),None);assert_eq!(app.state.project.channels[app.state.selected_channel].sound.fx.trim,AUTO_TRIM_THRESHOLD);
        act(&mut app,5);assert!(!auto_trim());assert!(import_window(&text).is_none());
        assert_eq!(app.state.project.channels[app.state.selected_channel].sound.fx.trim,AUTO_TRIM_THRESHOLD);
        assert_eq!(std::fs::read(&path).unwrap(),original);
        app.state.browser_preview.path=Some(text.to_string());app.state.browser_opts.insert(view::OPT_PANEL);
        let mut canvas=os_ui::Canvas::new(1280,900,1.).unwrap();app.paint(&mut canvas,None);
        assert!(app.hits.iter().any(|h|h.action==Action::BrowserPreview(5)));
        let _=std::fs::remove_file(path);
    }
    #[test] fn batch2_preview_options_change_audio_without_changing_file_or_import(){
        let path=std::env::temp_dir().join(format!("lemon-preview-options-{}.wav",std::process::id()));
        let rate=8000;let audio:Vec<f32>=(0..rate*2).flat_map(|i|{let v=if i<rate/2{0.1}else{0.};[v,v]}).collect();
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&path).unwrap(),rate,&audio).unwrap();
        let original=std::fs::read(&path).unwrap();
        let render=|normal,reversed,at|{
            let p=project(&path.to_string_lossy(),2.,at,120.);
            let mut lib=os_engine::SampleLibrary::default();lib.insert_buffer(path.to_string_lossy().into_owned(),processed(os_engine::sample::load_shared(&path).unwrap(),normal,reversed,false));let mut engine=os_engine::Engine::new(&p,&mut lib,rate,os_engine::PlayMode::Song);
            let mut out=vec![0.;rate as usize*4];engine.process(&mut out);out
        };
        let plain=render(false,false,0.);let normal=render(true,false,0.);let rev=render(false,true,0.);let both=render(true,true,0.);
        let peak=|s:&[f32]|s.iter().fold(0f32,|a,v|a.max(v.abs()));
        assert!(peak(&normal)>peak(&plain)*5.);
        assert!(peak(&rev[..rate as usize*2])<0.001);assert!(peak(&rev[rate as usize*3..])>0.02);
        assert!(peak(&both)>peak(&rev)*5.);assert!(peak(&render(false,true,1.6)[..1000])>0.02);
        assert_eq!(std::fs::read(&path).unwrap(),original);
        assert_eq!(crate::app::preview_project(&path.to_string_lossy()).channels[0].sound.fx,os_model::SampleFx::default());
        let _=std::fs::remove_file(path);
    }
    #[test] fn community_play_cancels_pending_sample_preview() {
        let mut app=App::new(None);app.state.song_mode=false;
        app.state.browser_preview=Preview{path:Some("preview.wav".into()),resume:true,pending:Some(Arc::new(Mutex::new(None))),..Default::default()};
        app.start_playback();
        assert!(app.state.browser_preview.path.is_none());
        assert!(app.state.browser_preview.pending.is_none());
        assert!(!app.state.browser_preview.resume);
        app.transport.stop();
    }
    #[test] fn long_preview_and_seek_keep_song_independent() {
        let p=project("test.wav",600.,450.,120.);
        assert_eq!(p.arrangements[0].clips[0].window,Some((450000.,600000.)));
        assert_eq!(p.arrangements[0].clips[0].length,150*2*u32::from(p.ppq));
        assert!(p.patterns.is_empty());
        assert_eq!(time(600.),"10:00");
    }
    #[test]
    fn browser_preview_long_audio_and_seek_render() {
        let path=std::env::temp_dir().join(format!("lemon-long-preview-{}.wav",std::process::id()));
        let rate=8000;
        let audio:Vec<f32>=(0..rate*8).flat_map(|i| {let v=if i<rate*6 {0.0}else{0.4};[v,v]}).collect();
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&path).unwrap(),rate,&audio).unwrap();
        let render=|at| {
            let p=project(&path.to_string_lossy(),8.,at,120.);
            let mut lib=os_engine::SampleLibrary::default();
            let mut e=os_engine::Engine::new(&p,&mut lib,rate,os_engine::PlayMode::Song);
            let mut buf=vec![0.;rate as usize*2*8];e.process(&mut buf);buf
        };
        let full=render(0.);let seek=render(6.5);
        assert!(full[..rate as usize*2*5].iter().all(|v|v.abs()<0.001));
        assert!(full[rate as usize*2*7..].iter().any(|v|v.abs()>0.05),"audio beyond five seconds must play");
        assert!(seek[..rate as usize].iter().any(|v|v.abs()>0.05),"seek must start in the selected part");
        let _=std::fs::remove_file(path);
    }

    #[test]
    fn browser_preview_options_and_playlist_menu() {
        let mut app=App::new(None);
        app.state.show_browser=true;
        app.state.browser_opts.insert(view::OPT_PANEL);
        app.state.browser_opts.remove(view::OPT_TREE_PLUGINS);
        assert!(!view::rows(&app.state).iter().any(|r|r.label=="Plugin database"));
        app.browser_cmd(crate::browser::BCmd::Toggle(view::OPT_TREE_PLUGINS));
        let rows=view::rows(&app.state);
        for label in ["Plugin database","Generators","Effects"] {assert!(rows.iter().any(|r|r.label==label),"{label}");}
        assert!(!visible(&app.state));
        app.state.browser_preview.path=Some("test.wav".into());
        assert!(visible(&app.state));
        let mut canvas=os_ui::Canvas::new(1280,900,1.).unwrap();app.paint(&mut canvas,None);
        for cmd in [0,1] {assert!(app.hits.iter().any(|h|h.action==Action::BrowserPreview(cmd)));}
        let menu=crate::playlist::options_menu(&app.state,0.,0.);
        assert!(menu.items.iter().any(|i|i.label=="Clip options"));
        assert!(menu.items.iter().find(|i|i.label=="View").unwrap().children.iter().any(|i|i.label=="Waveform detail when zoomed" && i.children.len()==3));
        crate::playlist::run(&mut app,crate::playlist::Cmd::WaveDetail(2));
        assert!(crate::playlist::max_zoom()>100000.);
        let path=std::env::temp_dir().join(format!("lemon-wave-detail-{}.wav",std::process::id()));
        let mut audio=vec![0f32;16000];audio[16]=1.;audio[17]=1.;
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&path).unwrap(),8000,&audio).unwrap();
        let pathstr=path.to_string_lossy();
        crate::playlist::add_audio_clip(&mut app.state,&pathstr,0,0).unwrap();
        app.state.z=vec![crate::app::Win::Playlist];
        app.state.playlist_bar_w=100000.;app.state.playlist_scroll.0=0.;
        app.paint(&mut canvas,None);
        let wave=app.state.playlist.wave(&pathstr).unwrap();
        let started = std::time::Instant::now();
        while wave.fine.borrow().is_none() {
            assert!(started.elapsed() < std::time::Duration::from_secs(3), "deep zoom must load individual samples");
            app.state.playlist.poll_waves();
            std::thread::yield_now();
        }
        let _=std::fs::remove_file(path);

        app.state.playlist_bar_w=100000.;
        crate::playlist::run(&mut app,crate::playlist::Cmd::WaveDetail(0));
        assert_eq!(app.state.playlist_bar_w,600.);
    }

}
