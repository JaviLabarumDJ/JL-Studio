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
        .take(10)
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
    "Eres el Director Musical y Productor Ejecutivo de JL Studio (un DAW profesional compatible con FL Studio). \
Tienes un equipo de 4 especialistas bajo tu mando a los que puedes delegar bloques complejos con `lemon_delegate`: \
1) `drums`: busca samples REALES en las librerías del usuario con `lemon_samples` (Kick, Clap, Closed Hat, Open Hat, Perc, Riser/Crash — NUNCA uses osciladores crudos para batería si hay samples disponibles), los carga con `lemon_channel(op: 'load_sample')`, programa grooves profesionales con `lemon_compose(op: 'drum_pattern')` y humanización, y asigna cada uno a su pista del mixer. \
2) `melody`: carga sintetizadores reales (`Lemon 3x Osc`, `Citrus`, `Lemon Wave`, etc. con `lemon_plugins`), ajusta sus parámetros con `lemon_parameter` para que suenen cálidos, filtrados y con pegada (NUNCA dejes un sinte con el preset crudo por defecto si puedes moldear su filtro/envolvente), y compone líneas de bajo (`rolling`/`offbeat`), acordes/stabs y arpegios. \
3) `arrangement`: organiza el tema completo en la Playlist (`lemon_clips`) en formato Extended Mix profesional (Intro DJ, Build-up con snare roll y riser, Drop 1, Breakdown atmosférico, Build-up 2, Drop 2, Outro DJ) y crea automatizaciones de tensión (`lemon_automation`). \
4) `mixing`: hace el ruteo, `sidechain` (Kick -> Bajo y Kick -> Stabs con `lemon_mix_setup`), carga `Lemon Parametric EQ`, `Lemon Soft Clipper`, `Lemon Reverb` y `Lemon Delay` en los inserts adecuados, ajusta niveles con `gain_stage` dejando -4 a -6 dB de headroom, y verifica con `lemon_audio_analyze` que nada sature. \
\nREGLAS DE ORO: \
- Para cualquier petición de crear o producir un tema completo, empieza leyendo `lemon_state` y `lemon_transport(op: 'song')` + BPM, y luego coordina a tu equipo llamando a `lemon_delegate` en orden: `drums` -> `melody` -> `arrangement` -> `mixing`. \
- Si el usuario pide algo puntual (ej. solo analizar o solo un patrón), puedes hacerlo directo o delegar al especialista correspondiente. \
- Responde siempre en español, detallando con precisión los samples reales cargados, sintetizadores, patrones, efectos, ruteos y verificación final."
}

fn specialist_prompt(role: &str) -> &'static str {
    match role {
        "drums" => "Eres el Especialista de Batería y Groove de JL Studio. Tu única misión es construir una sección rítmica profesional y con pegada real: \
1) Usa `lemon_samples` para buscar samples reales (.wav/.flac/.mp3) de Kick, Clap, Closed Hat, Open Hat y Percusión en las carpetas del usuario (ej. query: 'kick', 'clap', 'closed hat', 'open hat', 'perc'). \
2) Cárgales con `lemon_channel(op: 'load_sample', path: ...)` en canales separados y asígnales pistas de mixer distintas (ej. tracks 1..5). \
3) Programa patrones separados o combinados con `lemon_compose(op: 'drum_pattern', style: 'kick_4x4' | 'clap_2_4' | 'offbeat_hat' | 'closed_hat_16th' | 'perc_groove' | 'snare_roll', humanize: 10)` en los patrones que indique el Director. \
Devuelve un resumen exacto de qué samples cargaste, en qué filas/pistas del mixer y en qué patrones.",
        "melody" => "Eres el Especialista de Bajo, Síntesis y Armonía de JL Studio. Tu misión es crear bajos con peso y melodías/stabs memorables: \
1) Lista plugins con `lemon_plugins(op: 'list')` y carga sintetizadores (`Lemon 3x Osc`, `Citrus`, `Lemon Wave`) en nuevos canales, asignándoles pistas del mixer libres. \
2) Lee sus parámetros con `lemon_parameter(op: 'read')` y ajusta cutoff/envolvente/nivel con `lemon_parameter(op: 'set')` para que el bajo y los stabs suenen limpios y musicales (nunca un pitido agudo sin filtrar). \
3) Usa `lemon_compose` (`bassline` estilo `rolling`/`offbeat`, `chord_progression`, `arp`) con `humanize: 8` en la tonalidad y patrones indicados. \
Devuelve un resumen exacto de canales, pistas de mixer, parámetros ajustados y patrones.",
        "arrangement" => "Eres el Especialista de Arreglo y Tensión de JL Studio. Tu misión es montar un arreglo profesional en la Playlist: \
1) Inspecciona canales y patrones con `lemon_state`. \
2) Coloca clips en la Playlist con `lemon_clips(op: 'add', ...)` construyendo secciones reales (Intro DJ, Build-up, Drop 1, Breakdown, Build-up 2, Drop 2, Outro DJ) sin solapar clips erróneamente y cubriendo toda la duración pedida. \
3) Si hay parámetros automatizables o volumen/filtro, crea curvas de tensión con `lemon_automation`. \
Devuelve la estructura exacta por compases.",
        "mixing" => "Eres el Ingeniero de Mezcla y Mastering de JL Studio. Tu misión es que el tema suene limpio, potente y sin clipping: \
1) Inspecciona canales, inserts y efectos con `lemon_state` y `lemon_audio_analyze`. \
2) Configura `sidechain` (`lemon_mix_setup` op: 'sidechain') del Kick hacia el Bajo y hacia los Synths/Stabs. \
3) Carga `Lemon Parametric EQ`, `Lemon Soft Clipper`, `Lemon Reverb` y `Lemon Delay` en las pistas apropiadas (`lemon_plugins` op: 'add_effect'). \
4) Ajusta los faders con `lemon_mix_setup(op: 'gain_stage')` para mantener balance y headroom (-4 a -6 dB), y vuelve a ejecutar `lemon_audio_analyze` al final para confirmar.",
        _ => system_prompt(),
    }
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
        Ok(Ok(s)) => s,
        Ok(Err(e)) => json!({"error": e}).to_string(),
        Err(_) => r#"{"error":"Tool execution timed out"}"#.into(),
    }
}

fn director_tools() -> Vec<Value> {
    let mut t = crate::ai_commands::tools();
    t.push(json!({
        "name": "lemon_delegate",
        "description": "Delegate a focused production block to a specialist AI sub-agent with direct DAW tool access. Roles: 'drums' (searches real samples & programs grooves), 'melody' (loads/shapes synths, bassline, chords, arp), 'arrangement' (builds Extended Mix in Playlist & automations), 'mixing' (sidechain, EQ, FX, gain staging, Master check).",
        "inputSchema": {
            "type": "object",
            "properties": {
                "role": {"type": "string", "enum": ["drums", "melody", "arrangement", "mixing"]},
                "task": {"type": "string"}
            },
            "required": ["role", "task"]
        }
    }));
    t
}

fn openai_tools(include_delegate: bool) -> Vec<Value> {
    let list = if include_delegate { director_tools() } else { crate::ai_commands::tools() };
    list.into_iter()
        .map(|t| {
            json!({
                "type": "function",
                "function": {
                    "name": t["name"],
                    "description": t["description"],
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

fn gemini_tools(include_delegate: bool) -> Vec<Value> {
    let list = if include_delegate { director_tools() } else { crate::ai_commands::tools() };
    let decls: Vec<Value> = list
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

fn post_json_with_retry(
    agent: &ureq::Agent,
    url: &str,
    auth_bearer: Option<&str>,
    goog_key: Option<&str>,
    body_str: &str,
    event_tx: &mpsc::Sender<WorkerEvent>,
) -> Result<Value, String> {
    let delays = [2u64, 4, 8];
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
                let transient = matches!(code, 429 | 500 | 502 | 503 | 504);
                if transient && attempt < delays.len() {
                    let wait = delays[attempt];
                    let _ = event_tx.send(WorkerEvent::Status(format!(
                        "Modelo ocupado ({code}), reintentando en {wait}s..."
                    )));
                    std::thread::sleep(Duration::from_secs(wait));
                    continue;
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
            false,
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
        true,
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
    allow_delegate: bool,
    max_steps: usize,
    tool_tx: &mpsc::Sender<ToolReq>,
    event_tx: &mpsc::Sender<WorkerEvent>,
) -> Result<String, String> {
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
            let tools = openai_tools(allow_delegate);
            let url = if backend == Backend::Ollama {
                format!("{}/api/chat", base_url.trim_end_matches('/'))
            } else {
                format!("{}/chat/completions", base_url.trim_end_matches('/'))
            };

            for _ in 0..max_steps {
                let _ = event_tx.send(WorkerEvent::Status("Esperando respuesta del modelo...".into()));
                let body = if backend == Backend::Ollama {
                    json!({
                        "model": model,
                        "messages": msgs,
                        "tools": tools,
                        "stream": false
                    })
                } else {
                    json!({
                        "model": model,
                        "messages": msgs,
                        "tools": tools,
                    })
                };

                let auth = (backend == Backend::OpenAi && !api_key.is_empty()).then_some(api_key);
                let resp = post_json_with_retry(&agent, &url, auth, None, &body.to_string(), event_tx)?;

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
                        model,
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
            let tools = gemini_tools(allow_delegate);

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
