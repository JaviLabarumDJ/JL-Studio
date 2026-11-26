// SPDX-License-Identifier: GPL-3.0-or-later
//! In-DAW AI Producer Copilot: multi-turn chat + tool-calling pilot with automatic undo checkpoints.

use serde_json::{json, Value};
use std::sync::mpsc;
use std::time::Duration;

use crate::app::App;
use crate::dialog::Body;

const MAX_TOOL_STEPS: usize = 16;

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
            Backend::Gemini => "gemini-2.5-flash",
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

pub struct CopilotState {
    pub backend: Backend,
    pub model: String,
    pub api_key: String,
    pub base_url: String,
    pub input: String,
    pub cursor: usize,
    pub editing_config: bool,
    pub config_field: u8, // 0 = model, 1 = api_key, 2 = base_url
    pub messages: Vec<ChatMessage>,
    pub busy: bool,
    pub status: String,
    pub scroll: f32,
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

        Self {
            backend,
            model,
            api_key,
            base_url,
            input: String::new(),
            cursor: 0,
            editing_config: false,
            config_field: 0,
            messages: vec![ChatMessage {
                role: ChatRole::Assistant,
                text: "Soy tu Piloto de Producción IA de JL Studio. Puedo analizar el proyecto y tus niveles en tiempo real, componer acordes/bajos/arpegios/baterías, cargar instrumentos y efectos, rutear canales y hacer sidechain. Todos mis cambios se guardan con punto de control para deshacer en un clic.".into(),
            }],
            busy: false,
            status: "Listo. Escribe una orden o pulsa Analizar.".into(),
            scroll: 0.0,
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

pub fn open(app: &mut App) {
    if matches!(app.state.dialogs.last().map(|m| &m.body), Some(Body::Copilot)) {
        return;
    }
    app.show_form(Body::Copilot);
}

pub fn save_config(s: &CopilotState) {
    crate::settings_file::write_all(&[
        ("ai_copilot_backend".into(), s.backend.as_str().to_string()),
        ("ai_copilot_model".into(), s.model.clone()),
        ("ai_copilot_key".into(), s.api_key.clone()),
        ("ai_copilot_url".into(), s.base_url.clone()),
    ]);
}

pub fn cycle_backend(app: &mut App) {
    if app.state.copilot.busy {
        return;
    }
    let next = app.state.copilot.backend.next();
    app.state.copilot.backend = next;
    app.state.copilot.model = next.default_model().to_string();
    app.state.copilot.base_url = match next {
        Backend::Ollama => "http://127.0.0.1:11434".into(),
        Backend::OpenAi => "https://api.openai.com/v1".into(),
        Backend::Gemini => "https://generativelanguage.googleapis.com".into(),
    };
    if next == Backend::Gemini && app.state.copilot.api_key.is_empty() {
        if let Ok(k) = std::env::var("GEMINI_API_KEY") {
            app.state.copilot.api_key = k;
        }
    } else if next == Backend::OpenAi && app.state.copilot.api_key.is_empty() {
        if let Ok(k) = std::env::var("OPENAI_API_KEY") {
            app.state.copilot.api_key = k;
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
            let p = app.state.copilot.input.trim().to_string();
            app.state.copilot.input.clear();
            app.state.copilot.cursor = 0;
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
    let model = app.state.copilot.model.clone();
    let api_key = app.state.copilot.api_key.clone();
    let base_url = app.state.copilot.base_url.clone();
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
    "Eres el Piloto de Producción Musical de JL Studio (un DAW compatible con FL Studio). \
Siempre empieza leyendo el proyecto con `lemon_state` y `lemon_audio_analyze` antes de editar. \
Para componer acordes, bajos, arpegios o ritmos usa `lemon_compose` (es mucho más rápido y limpio que añadir notas sueltas). \
Para rutear grupos, sidechain o gain staging usa `lemon_mix_setup`. \
Para cargar instrumentos/efectos usa `lemon_plugins`. \
Si el usuario pide un análisis completo, revisa tempo, pistas sin rutear, niveles/clipping, Master Assistant y densidad de notas, y da recomendaciones concretas (o aplícalas si te lo pidió). \
Responde siempre en español, de forma clara y concisa indicando exactamente qué analizaste o modificaste."
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

fn openai_tools() -> Vec<Value> {
    crate::ai_commands::tools()
        .into_iter()
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

fn gemini_tools() -> Vec<Value> {
    let decls: Vec<Value> = crate::ai_commands::tools()
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

fn run_agent_loop(
    backend: Backend,
    model: &str,
    api_key: &str,
    base_url: &str,
    history_msgs: &[(ChatRole, String)],
    tool_tx: &mpsc::Sender<ToolReq>,
    event_tx: &mpsc::Sender<WorkerEvent>,
) -> Result<String, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(90))
        .build();

    match backend {
        Backend::Ollama | Backend::OpenAi => {
            let mut msgs = vec![json!({"role": "system", "content": system_prompt()})];
            for (r, txt) in history_msgs {
                let role = match r {
                    ChatRole::User => "user",
                    _ => "assistant",
                };
                msgs.push(json!({"role": role, "content": txt}));
            }
            let tools = openai_tools();
            let url = if backend == Backend::Ollama {
                format!("{}/api/chat", base_url.trim_end_matches('/'))
            } else {
                format!("{}/chat/completions", base_url.trim_end_matches('/'))
            };

            for _ in 0..MAX_TOOL_STEPS {
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

                let mut req = agent.post(&url).set("Content-Type", "application/json");
                if backend == Backend::OpenAi && !api_key.is_empty() {
                    req = req.set("Authorization", &format!("Bearer {api_key}"));
                }
                let raw_resp = match req.send_string(&body.to_string()) {
                    Ok(r) => r,
                    Err(ureq::Error::Status(code, r)) => {
                        let detail = r.into_string().unwrap_or_default();
                        return Err(format!("Error HTTP {code}: {detail}"));
                    }
                    Err(e) => return Err(format!("Error de conexión: {e}")),
                };
                let resp: Value = serde_json::from_reader(raw_resp.into_reader())
                    .map_err(|e| format!("JSON inválido del modelo: {e}"))?;

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
                    let out = invoke_ui_tool(tool_tx, event_tx, &name, args);
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
            let tools = gemini_tools();

            for _ in 0..MAX_TOOL_STEPS {
                let _ = event_tx.send(WorkerEvent::Status("Consultando Gemini...".into()));
                let body = json!({
                    "systemInstruction": {"parts": [{"text": system_prompt()}]},
                    "contents": contents,
                    "tools": tools,
                });
                let raw_resp = match agent
                    .post(&url)
                    .set("Content-Type", "application/json")
                    .set("x-goog-api-key", api_key.trim())
                    .send_string(&body.to_string())
                {
                    Ok(r) => r,
                    Err(ureq::Error::Status(code, r)) => {
                        let detail = r.into_string().unwrap_or_default();
                        return Err(format!("Error Gemini ({code}): {detail}"));
                    }
                    Err(e) => return Err(format!("Error de conexión Gemini: {e}")),
                };
                let resp: Value = serde_json::from_reader(raw_resp.into_reader())
                    .map_err(|e| format!("JSON inválido de Gemini: {e}"))?;

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
                    let out_str = invoke_ui_tool(tool_tx, event_tx, &name, args);
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
