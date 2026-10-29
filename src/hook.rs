use crate::cliente::Cliente;
use crate::evento::{ahora, Evento};
use crate::texto::recortar;
use serde_json::{json, Map, Value};
use std::io::{self, Read};
use std::time::Duration;

const LIMITE_ENTRADA: u64 = 4 << 20;

/// Lo único que se guarda de cada hook: qué sesión hace qué, dónde y en qué estado está. El hook va en todas las
/// sesiones y el bus es otra copia en disco que leen agentes, así que lo que ella escribe (`prompt`), lo que entra y
/// sale de las herramientas y la ruta de la transcripción no se copian: de lo demás queda solo el nombre en `omitidos`.
const CAMPOS: &[&str] = &[
    "hook_event_name",
    "session_id",
    "cwd",
    "permission_mode",
    "source",
    "reason",
    "notification_type",
    "message",
    "tool_name",
    "stop_hook_active",
    "agent_type",
];

fn lo_necesario(carga: Value) -> Value {
    let Value::Object(mapa) = carga else {
        return json!({ "omitidos": ["(la entrada no es un objeto JSON)"] });
    };
    let mut guardado = Map::new();
    let mut omitidos = Vec::new();
    for (clave, valor) in mapa {
        // Solo valores simples: un campo permitido que llegara como objeto o lista podría traer texto de ella.
        let simple = matches!(valor, Value::String(_) | Value::Bool(_) | Value::Number(_) | Value::Null);
        if simple && CAMPOS.contains(&clave.as_str()) {
            let valor = match valor {
                Value::String(s) => Value::String(recortar(&s, 500)),
                otro => otro,
            };
            guardado.insert(clave, valor);
        } else {
            omitidos.push(clave);
        }
    }
    if !omitidos.is_empty() {
        guardado.insert("omitidos".into(), json!(omitidos));
    }
    Value::Object(guardado)
}

/// `ev hook`: adaptador para los hooks de Claude Code. Lee el JSON del hook por la entrada estándar y lo publica como
/// `claude.<evento del hook>`, con solo lo necesario (ver `CAMPOS`). Nunca bloquea a Claude Code: no escribe nada en la
/// salida y siempre termina con 0.
pub fn recibir() -> i32 {
    let mut entrada = String::new();
    if let Err(e) = io::stdin().lock().take(LIMITE_ENTRADA).read_to_string(&mut entrada) {
        eprintln!("ev hook: no pude leer la entrada: {e}");
        return 0;
    }
    let carga: Value = serde_json::from_str(&entrada).unwrap_or(Value::Null);
    let nombre = carga.get("hook_event_name").and_then(Value::as_str).unwrap_or("hook").to_string();
    let sesion = carga.get("session_id").and_then(Value::as_str).map(str::to_string);
    let e = Evento {
        fuente: format!("hook:claude/{}", sesion.as_deref().unwrap_or("?")),
        tipo: format!("claude.{nombre}"),
        cuerpo: lo_necesario(carga),
        correlacion: sesion,
        t_evento: Some(ahora()),
        confianza: Some("local".into()),
        ..Default::default()
    };
    if let Err(err) = Cliente::nuevo().con_espera(Duration::from_secs(2)).publicar(&[e]) {
        eprintln!("ev hook: {err}");
    }
    0
}
