// SPDX-License-Identifier: GPL-3.0-or-later
//! In-DAW AI Producer Copilot: multi-turn chat + tool-calling pilot with automatic undo checkpoints.

use serde_json::{json, Value};
use std::sync::mpsc;
use std::time::Duration;

use crate::app::{App, State};
use winit::keyboard::{Key, NamedKey};

const MAX_TOOL_STEPS: usize = 32;
const SPECIALIST_TOOL_STEPS: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    Ollama,
    Gemini,
    OpenAi,
}

impl Backend {
    pub fn label(self) -> &'static str {
        match self {
            Backend::Ollama => "Ollama (Local)",
            Backend::Gemini => "Gemini API",
            Backend::OpenAi => "OpenAI / Compat",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Backend::Ollama => Backend::Gemini,
            Backend::Gemini => Backend::OpenAi,
            Backend::OpenAi => Backend::Ollama,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Backend::Ollama => "ollama",
            Backend::Gemini => "gemini",
            Backend::OpenAi => "openai",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "gemini" => Backend::Gemini,
            "openai" => Backend::OpenAi,
            _ => Backend::Ollama,
        }
    }

    pub fn default_model(self) -> &'static str {
        match self {
            Backend::Ollama => "qwen2.5:7b",
            Backend::Gemini => "gemini-2.5-pro",
            Backend::OpenAi => "gpt-4o-mini",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatRole {
    User,
    Assistant,
    Tool,
    Error,
}

#[derive(Clone, Debug)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopilotHit {
    Send,
    Analyze,
    Rollback,
    Backend,
    Config,
    Field(u8),
    Mcp,
    Clear,
    FocusInput,
    Close,
}

pub struct CopilotState {
    pub open: bool,
    pub input_focused: bool,
    pub backend: Backend,
    pub model: crate::dialog::Field,
    pub api_key: crate::dialog::Field,
    pub base_url: crate::dialog::Field,
    pub input: crate::dialog::Field,
    pub editing_config: bool,
    pub config_field: u8, // 0 = model, 1 = api_key, 2 = base_url
    pub messages: Vec<ChatMessage>,
    pub busy: bool,
    pub status: String,
    pub scroll: f32,
    pub max_scroll: std::cell::Cell<f32>,
    pub last_checkpoint: Option<(usize, usize)>, // (history_done, tool_calls)
}

impl Default for CopilotState {
    fn default() -> Self {
        let saved_backend = crate::settings_file::read(|s| s.get("ai_copilot_backend").map(str::to_string));
        let env_gemini = std::env::var("GEMINI_API_KEY").ok().filter(|s| !s.is_empty());
        let env_openai = std::env::var("OPENAI_API_KEY").ok().filter(|s| !s.is_empty());
        let backend = saved_backend
            .as_deref()
            .map(Backend::from_str)
            .unwrap_or_else(|| if env_gemini.is_some() { Backend::Gemini } else { Backend::Ollama });
        let model = crate::settings_file::read(|s| s.get("ai_copilot_model").map(str::to_string))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| backend.default_model().to_string());
        let api_key = crate::settings_file::read(|s| s.get("ai_copilot_key").map(str::to_string))
            .filter(|s| !s.is_empty())
            .or_else(|| match backend {
                Backend::Gemini => env_gemini.clone(),
                Backend::OpenAi => env_openai.clone(),
                Backend::Ollama => None,
            })
            .unwrap_or_default();
        let base_url = crate::settings_file::read(|s| s.get("ai_copilot_url").map(str::to_string))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| match backend {
                Backend::Ollama => "http://127.0.0.1:11434".into(),
                Backend::OpenAi => "https://api.openai.com/v1".into(),
                Backend::Gemini => "https://generativelanguage.googleapis.com".into(),
            });

        let mut model_f = crate::dialog::Field::default();
        model_f.set(&model);
        let mut key_f = crate::dialog::Field::default();
        key_f.set(&api_key);
        let mut url_f = crate::dialog::Field::default();
        url_f.set(&base_url);

        Self {
            open: false,
            input_focused: true,
            backend,
            model: model_f,
            api_key: key_f,
            base_url: url_f,
            input: crate::dialog::Field::default(),
            editing_config: false,
            config_field: 0,
            messages: vec![ChatMessage {
                role: ChatRole::Assistant,
                text: "Soy tu Piloto de Producción IA de JL Studio. Puedo analizar el proyecto y tus niveles en tiempo real, componer acordes/bajos/arpegios/baterías, cargar instrumentos y efectos, rutear canales y hacer sidechain. Todos mis cambios se guardan con punto de control para deshacer en un clic.".into(),
            }],
            busy: false,
            status: "Listo. Escribe una orden o pulsa Analizar.".into(),
            scroll: 0.0,
            max_scroll: std::cell::Cell::new(0.0),
            last_checkpoint: None,
        }
    }
}

pub struct ToolReq {
    pub name: String,
    pub args: Value,
    pub reply: mpsc::Sender<Result<String, String>>,
}

pub enum WorkerEvent {
    Status(String),
    ToolLog(String),
    ModelChanged(String),
    Done(Result<String, String>),
}

pub struct CopilotRuntime {
    pub tool_rx: mpsc::Receiver<ToolReq>,
    pub event_rx: mpsc::Receiver<WorkerEvent>,
}

pub fn panel_width(s: &State) -> f32 {
    if s.copilot.open {
        410.0f32.min((s.screen.0 * 0.38).max(320.0))
    } else {
        0.0
    }
}

pub fn panel_rect(s: &State) -> os_ui::Rect {
    let pw = panel_width(s);
    let top = crate::views::work_y(s);
    os_ui::Rect::new((s.screen.0 - pw).max(0.0), top, pw, (s.screen.1 - top).max(0.0))
}

pub fn open(app: &mut App) {
    app.state.copilot.open = !app.state.copilot.open;
    if app.state.copilot.open {
        app.state.copilot.input_focused = true;
    }
    app.invalidate();
}

pub fn command(app: &mut App, hit: CopilotHit) {
    match hit {
        CopilotHit::Send => send_prompt(app, None),
        CopilotHit::Analyze => send_prompt(
            app,
            Some("Analiza todo el proyecto ahora: tempo, pistas sin rutear, niveles/clipping, Master Assistant y densidad de notas, y dime qué mejorarías."),
        ),
        CopilotHit::Rollback => rollback(app),
        CopilotHit::Backend => cycle_backend(app),
        CopilotHit::Config => {
            app.state.copilot.editing_config = !app.state.copilot.editing_config;
            app.state.copilot.input_focused = true;
            if !app.state.copilot.editing_config {
                save_config(&app.state.copilot);
            }
        }
        CopilotHit::Field(idx) => {
            app.state.copilot.config_field = idx;
            app.state.copilot.input_focused = true;
        }
        CopilotHit::Mcp => {
            crate::ai_control::open(app);
        }
        CopilotHit::Clear => {
            if !app.state.copilot.busy {
                app.state.copilot.messages.truncate(1);
                app.state.copilot.scroll = 0.0;
            }
        }
        CopilotHit::FocusInput => {
            app.state.copilot.input_focused = true;
        }
        CopilotHit::Close => {
            app.state.copilot.open = false;
            app.state.copilot.input_focused = false;
        }
    }
    app.invalidate();
}

pub fn key(app: &mut App, key: &Key, ctrl: bool, alt: bool) {
    let shift = app.shift;
    let cp = &mut app.state.copilot;
    match key {
        Key::Named(NamedKey::Escape) => {
            if cp.editing_config {
                cp.editing_config = false;
                save_config(cp);
            } else {
                cp.input_focused = false;
            }
        }
        Key::Named(NamedKey::Enter) if !cp.editing_config && !shift => {
            send_prompt(app, None);
        }
        Key::Named(NamedKey::Enter) if !cp.editing_config && shift => {
            cp.input.insert("\n");
        }
        Key::Named(NamedKey::Enter) if cp.editing_config => {
            cp.editing_config = false;
            save_config(cp);
        }
        _ => {
            let field = if cp.editing_config {
                match cp.config_field {
                    0 => &mut cp.model,
                    1 => &mut cp.api_key,
                    _ => &mut cp.base_url,
                }
            } else {
                &mut cp.input
            };
            field.key_mod(key, ctrl, alt, shift);
        }
    }
    app.invalidate();
}

pub fn save_config(s: &CopilotState) {
    crate::settings_file::write_all(&[
        ("ai_copilot_backend".into(), s.backend.as_str().to_string()),
        ("ai_copilot_model".into(), s.model.as_str().to_string()),
        ("ai_copilot_key".into(), s.api_key.as_str().to_string()),
        ("ai_copilot_url".into(), s.base_url.as_str().to_string()),
    ]);
}

pub fn cycle_backend(app: &mut App) {
    if app.state.copilot.busy {
        return;
    }
    let next = app.state.copilot.backend.next();
    app.state.copilot.backend = next;
    app.state.copilot.model.set(next.default_model());
    app.state.copilot.base_url.set(match next {
        Backend::Ollama => "http://127.0.0.1:11434",
        Backend::OpenAi => "https://api.openai.com/v1",
        Backend::Gemini => "https://generativelanguage.googleapis.com",
    });
    if next == Backend::Gemini && app.state.copilot.api_key.is_empty() {
        if let Ok(k) = std::env::var("GEMINI_API_KEY") {
            app.state.copilot.api_key.set(&k);
        }
    } else if next == Backend::OpenAi && app.state.copilot.api_key.is_empty() {
        if let Ok(k) = std::env::var("OPENAI_API_KEY") {
            app.state.copilot.api_key.set(&k);
        }
    }
    save_config(&app.state.copilot);
    app.invalidate();
}

pub fn rollback(app: &mut App) {
    let Some((cp, _)) = app.state.copilot.last_checkpoint.take() else {
        app.state.copilot.status = "No hay cambios recientes del piloto para deshacer.".into();
        app.invalidate();
        return;
    };
    let undone = crate::history::undo_to(&mut app.state, cp);
    app.sync_live();
    app.state.piano.dirty = true;
    app.state.copilot.messages.push(ChatMessage {
        role: ChatRole::Tool,
        text: format!("Rollback aplicado: se deshicieron {undone} paso(s) del piloto."),
    });
    app.state.copilot.status = format!("Deshechos {undone} paso(s).");
    app.invalidate();
}

pub fn send_prompt(app: &mut App, custom_prompt: Option<&str>) {
    if app.state.copilot.busy {
        return;
    }
    let prompt = match custom_prompt {
        Some(p) => p.trim().to_string(),
        None => {
            let p = app.state.copilot.input.as_str().trim().to_string();
            app.state.copilot.input.clear();
            p
        }
    };
    if prompt.is_empty() {
        return;
    }

    let cp = crate::history::done_count(&app.state);
    app.state.copilot.last_checkpoint = Some((cp, 0));
    app.state.copilot.messages.push(ChatMessage {
        role: ChatRole::User,
        text: prompt.clone(),
    });
    app.state.copilot.busy = true;
    app.state.copilot.status = "Analizando proyecto y planificando acciones...".into();
    app.state.copilot.scroll = f32::MAX;
    app.invalidate();

    let (tool_tx, tool_rx) = mpsc::channel();
    let (event_tx, event_rx) = mpsc::channel();
    app.copilot_runtime = Some(CopilotRuntime { tool_rx, event_rx });

    let backend = app.state.copilot.backend;
    let model = app.state.copilot.model.as_str().to_string();
    let api_key = app.state.copilot.api_key.as_str().to_string();
    let base_url = app.state.copilot.base_url.as_str().to_string();
    let history_msgs: Vec<(ChatRole, String)> = app
        .state
        .copilot
        .messages
        .iter()
        .rev()
        .take(4)
        .rev()
        .filter(|m| matches!(m.role, ChatRole::User | ChatRole::Assistant))
        .map(|m| (m.role, m.text.clone()))
        .collect();

    std::thread::spawn(move || {
        let res = run_agent_loop(backend, &model, &api_key, &base_url, &history_msgs, &tool_tx, &event_tx);
        let _ = event_tx.send(WorkerEvent::Done(res));
    });
}

pub fn tick(app: &mut App) {
    let mut finished = false;
    let mut tool_calls = Vec::new();
    let mut events = Vec::new();
    if let Some(rt) = app.copilot_runtime.as_ref() {
        while let Ok(req) = rt.tool_rx.try_recv() {
            tool_calls.push(req);
        }
        while let Ok(ev) = rt.event_rx.try_recv() {
            events.push(ev);
        }
    }

    for req in tool_calls {
        let res = crate::ai_commands::call_copilot(app, &req.name, &req.args)
            .map(|v| serde_json::to_string(&v).unwrap_or_else(|_| "{}".into()));
        if res.is_ok() {
            if let Some((_, count)) = app.state.copilot.last_checkpoint.as_mut() {
                *count += 1;
            }
        }
        let _ = req.reply.send(res);
    }

    for ev in events {
        match ev {
            WorkerEvent::Status(s) => {
                app.state.copilot.status = s;
                app.invalidate();
            }
            WorkerEvent::ToolLog(log) => {
                app.state.copilot.messages.push(ChatMessage {
                    role: ChatRole::Tool,
                    text: log,
                });
                app.state.copilot.scroll = f32::MAX;
                app.invalidate();
            }
            WorkerEvent::ModelChanged(new_model) => {
                app.state.copilot.model.set(&new_model);
                save_config(&app.state.copilot);
                app.invalidate();
            }
            WorkerEvent::Done(res) => {
                finished = true;
                app.state.copilot.busy = false;
                match res {
                    Ok(reply) => {
                        app.state.copilot.messages.push(ChatMessage {
                            role: ChatRole::Assistant,
                            text: reply,
                        });
                        let n = app.state.copilot.last_checkpoint.map_or(0, |(_, c)| c);
                        app.state.copilot.status = if n > 0 {
                            format!("Completado ({n} acciones). Usa 'Deshacer piloto' si no te convence.")
                        } else {
                            "Completado.".into()
                        };
                    }
                    Err(err) => {
                        app.state.copilot.messages.push(ChatMessage {
                            role: ChatRole::Error,
                            text: err,
                        });
                        app.state.copilot.status = "Error al consultar el modelo.".into();
                    }
                }
                app.state.copilot.scroll = f32::MAX;
                app.invalidate();
            }
        }
    }

    if finished {
        app.copilot_runtime = None;
    }
}

fn system_prompt() -> &'static str {
    "Eres el Director Musical de JL Studio (DAW). Tienes 4 especialistas via `lemon_delegate`: \
1) `drums`: busca samples reales (`lemon_samples`), carga con `lemon_channel(load_sample)`, programa `lemon_compose(drum_pattern)` y asigna mixer. \
2) `melody`: carga sintes (`lemon_plugins`), moldea filtro/envolvente (`lemon_parameter`) y compone bajo/stabs/arp (`lemon_compose`). \
3) `arrangement`: arma la Playlist (`lemon_clips`) en Extended Mix (Intro, Build-up, Drop 1, Breakdown, Build-up 2, Drop 2, Outro) y `lemon_automation`. \
4) `mixing`: sidechain (`lemon_mix_setup`), EQ/Clipper/Reverb/Delay, `gain_stage` (-4 a -6 dB) y `lemon_audio_analyze`. \
Para temas completos, lee `lemon_state` y `lemon_transport`, y delega en orden: `drums` -> `melody` -> `arrangement` -> `mixing`. Responde siempre en español con detalles exactos."
}

fn specialist_prompt(role: &str) -> &'static str {
    match role {
        "drums" => "Especialista de Batería JL Studio: 1) Busca samples reales con `lemon_samples` (kick, clap, closed hat, open hat, perc). \
2) Cárgalos con `lemon_channel(op:'load_sample')` en canales distintos y pistas mixer 1..5. \
3) Programa patrones con `lemon_compose(op:'drum_pattern', style:'kick_4x4'|'clap_2_4'|'offbeat_hat'|'closed_hat_16th'|'perc_groove'|'snare_roll', humanize:10)`. Resume samples, pistas y patrones.",
        "melody" => "Especialista de Bajo/Melodía JL Studio: 1) Lista/carga sintes (`Lemon 3x Osc`, `Citrus`, `Lemon Wave`) con `lemon_plugins` en pistas mixer libres. \
2) Ajusta cutoff/envolvente con `lemon_parameter` para sonido cálido y musical. \
3) Compone con `lemon_compose` (`bassline` rolling/offbeat, `chord_progression`, `arp`, `humanize:8`). Resume canales, mixer y patrones.",
        "arrangement" => "Especialista de Arreglo JL Studio: 1) Lee `lemon_state`. \
2) Coloca clips en Playlist con `lemon_clips(op:'add')` formando Intro, Build-up, Drop 1, Breakdown, Build-up 2, Drop 2, Outro sin solapamientos erróneos. \
3) Añade automatizaciones de tensión con `lemon_automation`. Resume estructura por compases.",
        "mixing" => "Ingeniero de Mezcla JL Studio: 1) Lee `lemon_state` y `lemon_audio_analyze`. \
2) Haz `sidechain` (`lemon_mix_setup`) del Kick al Bajo y Synths/Stabs. \
3) Añade `Lemon Parametric EQ`, `Lemon Soft Clipper`, `Lemon Reverb`, `Lemon Delay` (`lemon_plugins`). \
4) Haz `gain_stage` (-4 a -6 dB headroom) y confirma con `lemon_audio_analyze`.",
        _ => system_prompt(),
    }
}

fn compact_tool_output(name: &str, raw: String) -> String {
    let compacted = if name == "lemon_state" {
        if let Ok(mut v) = serde_json::from_str::<Value>(&raw) {
            let sel_mix = v["selected_mixer"].as_u64().unwrap_or(0);
            let used_tracks: std::collections::HashSet<u64> = v["channels"]
                .as_array()
                .map(|arr| arr.iter().filter_map(|c| c["mixer_track"].as_u64()).collect())
                .unwrap_or_default();
            if let Some(obj) = v.as_object_mut() {
                obj.remove("limitations");
                if let Some(pl_count) = obj.remove("playlist_tracks").and_then(|p| p.as_array().map(|a| a.len())) {
                    obj.insert("playlist_track_count".to_string(), json!(pl_count));
                }
            }
            if let Some(mixer) = v["mixer"].as_array().cloned() {
                let compact_mixer: Vec<Value> = mixer
                    .into_iter()
                    .filter(|m| {
                        let t = m["track"].as_u64().unwrap_or(0);
                        let has_slots = m["slots"].as_array().is_some_and(|s| !s.is_empty());
                        t == 0 || t == sel_mix || used_tracks.contains(&t) || has_slots
                    })
                    .take(16)
                    .map(|mut m| {
                        if let Some(obj) = m.as_object_mut() {
                            obj.remove("strip");
                            obj.remove("eq");
                            obj.remove("strip_bypassed");
                            obj.remove("effects");
                        }
                        m
                    })
                    .collect();
                v["mixer"] = Value::Array(compact_mixer);
            }
            v.to_string()
        } else {
            raw
        }
    } else {
        raw
    };
    if compacted.len() > 3_200 {
        let end = compacted.floor_char_boundary(3_200);
        return format!("{}...(truncated)", &compacted[..end]);
    }
    compacted
}

fn invoke_ui_tool(
    tool_tx: &mpsc::Sender<ToolReq>,
    event_tx: &mpsc::Sender<WorkerEvent>,
    name: &str,
    args: Value,
) -> String {
    let _ = event_tx.send(WorkerEvent::Status(format!("Ejecutando {name}...")));
    let _ = event_tx.send(WorkerEvent::ToolLog(format!("⚙ {name}")));
    let (reply_tx, reply_rx) = mpsc::channel();
    if tool_tx
        .send(ToolReq {
            name: name.to_string(),
            args,
            reply: reply_tx,
        })
        .is_err()
    {
        return r#"{"error":"DAW UI closed"}"#.into();
    }
    match reply_rx.recv_timeout(Duration::from_secs(15)) {
        Ok(Ok(s)) => compact_tool_output(name, s),
        Ok(Err(e)) => json!({"error": e}).to_string(),
        Err(_) => r#"{"error":"Tool execution timed out"}"#.into(),
    }
}

fn tools_for_role(role: Option<&str>) -> Vec<Value> {
    let all = crate::ai_commands::tools();
    match role {
        Some("drums") => all.into_iter().filter(|t| matches!(t["name"].as_str(),
            Some("lemon_state" | "lemon_samples" | "lemon_channel" | "lemon_compose"))).collect(),
        Some("melody") => all.into_iter().filter(|t| matches!(t["name"].as_str(),
            Some("lemon_state" | "lemon_plugins" | "lemon_parameter" | "lemon_compose" | "lemon_channel"))).collect(),
        Some("arrangement") => all.into_iter().filter(|t| matches!(t["name"].as_str(),
            Some("lemon_state" | "lemon_clips" | "lemon_automation" | "lemon_transport"))).collect(),
        Some("mixing") => all.into_iter().filter(|t| matches!(t["name"].as_str(),
            Some("lemon_state" | "lemon_mix_setup" | "lemon_mixer" | "lemon_plugins" | "lemon_audio_analyze"))).collect(),
        _ => {
            let mut list: Vec<Value> = all.into_iter().filter(|t| matches!(t["name"].as_str(),
                Some("lemon_state" | "lemon_transport" | "lemon_audio_analyze" | "lemon_compose" | "lemon_checkpoint"))).collect();
            list.push(json!({
                "name": "lemon_delegate",
                "description": "Delegate to specialist: 'drums' (samples & grooves), 'melody' (synths, bass, chords, arp), 'arrangement' (Playlist Extended Mix & automations), 'mixing' (sidechain, EQ, FX, gain staging).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "role": {"type": "string", "enum": ["drums", "melody", "arrangement", "mixing"]},
                        "task": {"type": "string"}
                    },
                    "required": ["role", "task"]
                }
            }));
            list
        }
    }
}

fn openai_tools(role: Option<&str>) -> Vec<Value> {
    tools_for_role(role)
        .into_iter()
        .map(|t| {
            let desc = t["description"].as_str().unwrap_or("");
            let short_desc = if desc.len() > 120 {
                let end = desc.floor_char_boundary(120);
                &desc[..end]
            } else {
                desc
            };
            json!({
                "type": "function",
                "function": {
                    "name": t["name"],
                    "description": short_desc,
                    "parameters": t["inputSchema"],
                }
            })
        })
        .collect()
}

fn sanitize_gemini_schema(v: Value) -> Value {
    match v {
        Value::Object(mut map) => {
            map.remove("additionalProperties");
            if let Some(Value::Object(props)) = map.get("properties") {
                if props.is_empty() {
                    map.remove("properties");
                    map.remove("required");
                }
            }
            let cleaned = map
                .into_iter()
                .map(|(k, val)| (k, sanitize_gemini_schema(val)))
                .collect();
            Value::Object(cleaned)
        }
        Value::Array(arr) => Value::Array(arr.into_iter().map(sanitize_gemini_schema).collect()),
        other => other,
    }
}

fn gemini_tools(role: Option<&str>) -> Vec<Value> {
    let decls: Vec<Value> = tools_for_role(role)
        .into_iter()
        .map(|t| {
            let schema = sanitize_gemini_schema(t["inputSchema"].clone());
            let has_props = schema.get("properties").is_some();
            if has_props {
                json!({
                    "name": t["name"],
                    "description": t["description"],
                    "parameters": schema,
                })
            } else {
                json!({
                    "name": t["name"],
                    "description": t["description"],
                })
            }
        })
        .collect();
    vec![json!({ "functionDeclarations": decls })]
}

fn parse_retry_after_secs(detail: &str) -> Option<u64> {
    let idx = detail.find("try again in ")?;
    let rest = &detail[idx + "try again in ".len()..];
    let num: String = rest.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    num.parse::<f64>().ok().map(|s| (s.ceil() as u64 + 1).clamp(2, 30))
}

fn post_json_with_retry(
    agent: &ureq::Agent,
    url: &str,
    auth_bearer: Option<&str>,
    goog_key: Option<&str>,
    body_str: &str,
    event_tx: &mpsc::Sender<WorkerEvent>,
) -> Result<Value, String> {
    let delays = [3u64, 6, 10, 15];
    for attempt in 0..=delays.len() {
        let mut req = agent.post(url).set("Content-Type", "application/json");
        if let Some(tok) = auth_bearer {
            req = req.set("Authorization", &format!("Bearer {tok}"));
        }
        if let Some(key) = goog_key {
            req = req.set("x-goog-api-key", key);
        }
        match req.send_string(body_str) {
            Ok(r) => {
                return serde_json::from_reader(r.into_reader())
                    .map_err(|e| format!("JSON inválido del modelo: {e}"));
            }
            Err(ureq::Error::Status(code, r)) => {
                let detail = r.into_string().unwrap_or_default();
                if code == 429 && (detail.contains("PerDay") || detail.contains("FreeTier")) {
                    return Err("Cuota diaria gratuita de Gemini agotada para este modelo. Cambia a gemini-2.5-flash-lite / gemini-2.0-flash en Config, activa facturacion en AI Studio, o usa Ollama (Local) sin limites.".into());
                }
                let transient = matches!(code, 429 | 500 | 502 | 503 | 504)
                    || (code == 413 && detail.contains("rate_limit_exceeded") && attempt == 0);
                if transient && attempt < delays.len() {
                    let wait = if code == 413 {
                        10
                    } else {
                        parse_retry_after_secs(&detail).unwrap_or(delays[attempt])
                    };
                    let _ = event_tx.send(WorkerEvent::Status(format!(
                        "Limite por minuto ({code}), esperando {wait}s..."
                    )));
                    std::thread::sleep(Duration::from_secs(wait));
                    continue;
                }
                if code == 413 {
                    return Err(format!(
                        "Error HTTP (413): La petición supera el límite de tokens del modelo gratuito. Prueba limpiar el chat o usar un prompt más corto. Detalle: {detail}"
                    ));
                }
                return Err(format!("Error HTTP ({code}): {detail}"));
            }
            Err(e) => {
                if attempt < delays.len() {
                    let wait = delays[attempt];
                    let _ = event_tx.send(WorkerEvent::Status(format!(
                        "Error de red, reintentando en {wait}s..."
                    )));
                    std::thread::sleep(Duration::from_secs(wait));
                    continue;
                }
                return Err(format!("Error de conexión: {e}"));
            }
        }
    }
    unreachable!()
}

fn dispatch_tool(
    backend: Backend,
    model: &str,
    api_key: &str,
    base_url: &str,
    allow_delegate: bool,
    tool_tx: &mpsc::Sender<ToolReq>,
    event_tx: &mpsc::Sender<WorkerEvent>,
    name: &str,
    args: Value,
) -> String {
    if allow_delegate && name == "lemon_delegate" {
        let role = args["role"].as_str().unwrap_or("drums");
        let task = args["task"].as_str().unwrap_or("");
        let badge = match role {
            "drums" => "🥁 Especialista de Batería",
            "melody" => "🎹 Especialista de Bajo/Melodía",
            "arrangement" => "🎛️ Especialista de Arreglo",
            "mixing" => "🎚️ Ingeniero de Mezcla",
            _ => "🎧 Especialista",
        };
        let _ = event_tx.send(WorkerEvent::Status(format!("{badge} trabajando...")));
        let _ = event_tx.send(WorkerEvent::ToolLog(format!("▸ Delegando en {badge}")));
        let sub_history = vec![(ChatRole::User, task.to_string())];
        return match run_agent_with_prompt(
            backend,
            model,
            api_key,
            base_url,
            &sub_history,
            specialist_prompt(role),
            Some(role),
            SPECIALIST_TOOL_STEPS,
            tool_tx,
            event_tx,
        ) {
            Ok(summary) => json!({"role": role, "status": "completed", "summary": summary}).to_string(),
            Err(err) => json!({"role": role, "status": "error", "error": err}).to_string(),
        };
    }
    invoke_ui_tool(tool_tx, event_tx, name, args)
}

fn detect_openai_model(agent: &ureq::Agent, base_url: &str, api_key: &str) -> Option<String> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let mut req = agent.get(&url);
    if !api_key.is_empty() {
        req = req.set("Authorization", &format!("Bearer {api_key}"));
    }
    let resp = req.call().ok()?;
    let val: Value = serde_json::from_reader(resp.into_reader()).ok()?;
    let list = val["data"].as_array()?;
    let ids: Vec<&str> = list.iter().filter_map(|m| m["id"].as_str()).collect();
    let usable = |id: &str| {
        let l = id.to_ascii_lowercase();
        !l.contains("whisper") && !l.contains("tts") && !l.contains("guard") && !l.contains("embed") && !l.contains("moderation")
    };
    let prefs = ["gpt-oss", "llama-3.3", "llama-3.1", "llama-4", "qwen", "gpt-4o", "deepseek", "mixtral"];
    for p in prefs {
        if let Some(&m) = ids.iter().find(|&&id| usable(id) && id.to_ascii_lowercase().contains(p)) {
            return Some(m.to_string());
        }
    }
    ids.into_iter().find(|&id| usable(id)).map(str::to_string)
}

fn run_agent_loop(
    backend: Backend,
    model: &str,
    api_key: &str,
    base_url: &str,
    history_msgs: &[(ChatRole, String)],
    tool_tx: &mpsc::Sender<ToolReq>,
    event_tx: &mpsc::Sender<WorkerEvent>,
) -> Result<String, String> {
    run_agent_with_prompt(
        backend,
        model,
        api_key,
        base_url,
        history_msgs,
        system_prompt(),
        None,
        MAX_TOOL_STEPS,
        tool_tx,
        event_tx,
    )
}

fn run_agent_with_prompt(
    backend: Backend,
    model: &str,
    api_key: &str,
    base_url: &str,
    history_msgs: &[(ChatRole, String)],
    sys_prompt: &str,
    specialist_role: Option<&str>,
    max_steps: usize,
    tool_tx: &mpsc::Sender<ToolReq>,
    event_tx: &mpsc::Sender<WorkerEvent>,
) -> Result<String, String> {
    let allow_delegate = specialist_role.is_none();
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(180))
        .build();

    match backend {
        Backend::Ollama | Backend::OpenAi => {
            let mut msgs = vec![json!({"role": "system", "content": sys_prompt})];
            for (r, txt) in history_msgs {
                let role = match r {
                    ChatRole::User => "user",
                    _ => "assistant",
                };
                msgs.push(json!({"role": role, "content": txt}));
            }
            let tools = openai_tools(specialist_role);
            let url = if backend == Backend::Ollama {
                format!("{}/api/chat", base_url.trim_end_matches('/'))
            } else {
                format!("{}/chat/completions", base_url.trim_end_matches('/'))
            };

            let mut active_model = model.to_string();
            let mut schema_retries = 0usize;
            for _ in 0..max_steps {
                let _ = event_tx.send(WorkerEvent::Status("Esperando respuesta del modelo...".into()));
                let make_body = |m: &str| {
                    if backend == Backend::Ollama {
                        json!({
                            "model": m,
                            "messages": msgs,
                            "tools": tools,
                            "stream": false
                        })
                    } else {
                        json!({
                            "model": m,
                            "messages": msgs,
                            "tools": tools,
                        })
                    }
                };

                let auth = (backend == Backend::OpenAi && !api_key.is_empty()).then_some(api_key);
                let resp = match post_json_with_retry(&agent, &url, auth, None, &make_body(&active_model).to_string(), event_tx) {
                    Ok(r) => r,
                    Err(e) if backend == Backend::OpenAi && e.contains("(404)") => {
                        if let Some(found) = detect_openai_model(&agent, base_url, api_key) {
                            let _ = event_tx.send(WorkerEvent::ToolLog(format!("Modelo '{active_model}' no disponible; cambiando a '{found}'")));
                            let _ = event_tx.send(WorkerEvent::ModelChanged(found.clone()));
                            active_model = found;
                            post_json_with_retry(&agent, &url, auth, None, &make_body(&active_model).to_string(), event_tx)?
                        } else {
                            return Err(e);
                        }
                    }
                    Err(e) if backend == Backend::OpenAi && e.contains("tool_use_failed") && schema_retries < 2 => {
                        schema_retries += 1;
                        let _ = event_tx.send(WorkerEvent::Status("Corrigiendo parámetros de herramienta...".into()));
                        msgs.push(json!({
                            "role": "user",
                            "content": format!("Tu última llamada a herramienta falló la validación de esquema JSON ({e}). Corrige los tipos y campos obligatorios (por ejemplo 'row' entero 0-based en lemon_compose) y vuelve a intentarlo.")
                        }));
                        continue;
                    }
                    Err(e) => return Err(e),
                };

                let msg = if backend == Backend::Ollama {
                    resp["message"].clone()
                } else {
                    resp["choices"][0]["message"].clone()
                };

                let calls = msg["tool_calls"].as_array().cloned().unwrap_or_default();
                if calls.is_empty() {
                    let content = msg["content"].as_str().unwrap_or("").trim().to_string();
                    return Ok(if content.is_empty() {
                        "Acciones completadas.".into()
                    } else {
                        content
                    });
                }

                msgs.push(msg);
                for (idx, tc) in calls.into_iter().enumerate() {
                    let name = tc["function"]["name"].as_str().unwrap_or("").to_string();
                    let raw_args = &tc["function"]["arguments"];
                    let args = if let Some(s) = raw_args.as_str() {
                        serde_json::from_str(s).unwrap_or_else(|_| json!({}))
                    } else if raw_args.is_object() {
                        raw_args.clone()
                    } else {
                        json!({})
                    };
                    let out = dispatch_tool(
                        backend,
                        &active_model,
                        api_key,
                        base_url,
                        allow_delegate,
                        tool_tx,
                        event_tx,
                        &name,
                        args,
                    );
                    let id = tc["id"].as_str().map(str::to_string).unwrap_or_else(|| format!("call_{idx}"));
                    msgs.push(json!({
                        "role": "tool",
                        "tool_call_id": id,
                        "name": name,
                        "content": out,
                    }));
                }
            }
            Ok("Se alcanzó el límite de pasos por turno. Revisa los cambios aplicados.".into())
        }
        Backend::Gemini => {
            if api_key.trim().is_empty() {
                return Err("Falta GEMINI_API_KEY. Configúrala en el panel o usa Ollama local.".into());
            }
            let url = format!(
                "{}/v1beta/models/{}:generateContent",
                base_url.trim_end_matches('/'),
                model.trim()
            );
            let mut contents: Vec<Value> = history_msgs
                .iter()
                .map(|(r, txt)| {
                    json!({
                        "role": if *r == ChatRole::User { "user" } else { "model" },
                        "parts": [{"text": txt}]
                    })
                })
                .collect();
            let tools = gemini_tools(specialist_role);

            let mut schema_retries = 0usize;
            for _ in 0..max_steps {
                let _ = event_tx.send(WorkerEvent::Status("Consultando Gemini...".into()));
                let body = json!({
                    "systemInstruction": {"parts": [{"text": sys_prompt}]},
                    "contents": contents,
                    "tools": tools,
                });
                let resp = post_json_with_retry(
                    &agent,
                    &url,
                    None,
                    Some(api_key.trim()),
                    &body.to_string(),
                    event_tx,
                )?;

                let candidate_content = resp["candidates"][0]["content"].clone();
                let parts = candidate_content["parts"].as_array().cloned().unwrap_or_default();
                let mut fn_calls = Vec::new();
                let mut text_out = String::new();
                for p in &parts {
                    if let Some(fc) = p.get("functionCall") {
                        fn_calls.push(fc.clone());
                    }
                    if let Some(t) = p.get("text").and_then(Value::as_str) {
                        if !text_out.is_empty() {
                            text_out.push('\n');
                        }
                        text_out.push_str(t);
                    }
                }
                if fn_calls.is_empty() {
                    return Ok(if text_out.trim().is_empty() {
                        "Acciones completadas.".into()
                    } else {
                        text_out
                    });
                }

                contents.push(candidate_content);
                let mut response_parts = Vec::new();
                for fc in fn_calls {
                    let name = fc["name"].as_str().unwrap_or("").to_string();
                    let args = fc.get("args").cloned().unwrap_or_else(|| json!({}));
                    let out_str = dispatch_tool(
                        backend,
                        model,
                        api_key,
                        base_url,
                        allow_delegate,
                        tool_tx,
                        event_tx,
                        &name,
                        args,
                    );
                    let out_json: Value = serde_json::from_str(&out_str).unwrap_or_else(|_| json!({"result": out_str}));
                    response_parts.push(json!({
                        "functionResponse": {
                            "name": name,
                            "response": out_json,
                        }
                    }));
                }
                contents.push(json!({
                    "role": "user",
                    "parts": response_parts,
                }));
            }
            Ok("Se alcanzó el límite de pasos por turno. Revisa los cambios aplicados.".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::ai_commands::call_copilot as call;
    use crate::app::App;
    use serde_json::json;

    #[test]
    fn copilot_compose_analyze_and_rollback_work() {
        let mut app = App::new(None);
        let cp = call(&mut app, "lemon_checkpoint", &json!({"op": "mark"})).unwrap()["checkpoint"]
            .as_u64()
            .unwrap();

        let composed = call(
            &mut app,
            "lemon_compose",
            &json!({
                "op": "chord_progression",
                "row": 0,
                "root": "F",
                "scale": "minor",
                "bars": 2,
                "chords": ["i", "VI"]
            }),
        )
        .unwrap();
        assert_eq!(composed["added"].as_u64().unwrap(), 6);

        let analyzed = call(&mut app, "lemon_audio_analyze", &json!({})).unwrap();
        assert_eq!(analyzed["channel_density"][0]["notes_in_pattern"].as_u64().unwrap(), 6);

        let rolled = call(
            &mut app,
            "lemon_checkpoint",
            &json!({"op": "rollback", "checkpoint": cp}),
        )
        .unwrap();
        assert!(rolled["undone_steps"].as_u64().unwrap() >= 1);

        let after = call(&mut app, "lemon_audio_analyze", &json!({})).unwrap();
        assert_eq!(after["channel_density"][0]["notes_in_pattern"].as_u64().unwrap(), 0);
    }
}
