use crate::cliente::Cliente;
use crate::evento::{ahora, Evento};
use crate::texto::recortar;
use serde_json::{json, Map, Value};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

const LIMITE_ENTRADA: u64 = 4 << 20;

/// Más carpetas cuyas sesiones no se publican, separadas por `;`; se suman a la de por defecto. Una entrada `-` quita
/// la de por defecto (`EV_HOOK_IGNORAR=-` no ignora nada).
pub const VARIABLE_IGNORAR: &str = "EV_HOOK_IGNORAR";

/// Dentro de la carpeta temporal del sistema, donde Constructo abre sus `claude -p` internas
/// (`lector-ia-skills\sesion-<pid>-<azar>`). El 2026-10-31 eran 216 de los 226 `SessionStart` del día: ruido que tapaba
/// a la app y a las sesiones de verdad.
pub const CARPETA_CONSTRUCTO: &str = "lector-ia-skills";

/// Las carpetas cuyas sesiones no se publican: la de Constructo dentro de `temporal`, salvo que la variable traiga `-`,
/// y las que traiga la variable (`EV_HOOK_IGNORAR`). Las entradas vacías no cuentan y se quitan espacios y comillas.
pub fn carpetas_ignoradas(variable: Option<&str>, temporal: &Path) -> Vec<String> {
    let entradas: Vec<&str> = variable
        .unwrap_or("")
        .split(';')
        .map(|e| e.trim().trim_matches('"').trim())
        .filter(|e| !e.is_empty())
        .collect();
    let mut carpetas = Vec::new();
    if !entradas.contains(&"-") {
        carpetas.push(temporal.join(CARPETA_CONSTRUCTO).display().to_string());
    }
    carpetas.extend(entradas.into_iter().filter(|e| *e != "-").map(str::to_string));
    carpetas
}

/// La carpeta temporal como la ve Constructo (`os.tmpdir()` de Node): en Windows manda `TEMP`, que no siempre es la
/// primera que mira `std::env::temp_dir()` (esa empieza por `TMP`).
fn temporal() -> PathBuf {
    let temp = if cfg!(windows) { std::env::var_os("TEMP").filter(|t| !t.is_empty()) } else { None };
    temp.map(PathBuf::from).unwrap_or_else(std::env::temp_dir)
}

/// Los componentes de una ruta, para compararla: sin huecos ni `.`; en Windows, sin distinguir mayúsculas y con `/` y
/// `\` equivalentes. Solo se compara el texto: no se resuelven nombres cortos (8.3), enlaces ni `..`.
fn componentes(ruta: &str) -> Vec<String> {
    let (ruta, separadores): (String, &[char]) =
        if cfg!(windows) { (ruta.to_lowercase(), &['\\', '/']) } else { (ruta.to_string(), &['/']) };
    ruta.split(separadores).filter(|c| !c.is_empty() && *c != ".").map(str::to_string).collect()
}

/// Si `ruta` es `carpeta` o está dentro de ella, componente a componente: `…\lector-ia-skills2` no está dentro de
/// `…\lector-ia-skills`. Una carpeta sin componentes (vacía o solo separadores) no contiene nada.
pub fn dentro_de(ruta: &str, carpeta: &str) -> bool {
    let (r, c) = (componentes(ruta), componentes(carpeta));
    !c.is_empty() && r.starts_with(&c)
}

/// Si la sesión que manda el hook trabaja en una carpeta ignorada (ver `carpetas_ignoradas`).
fn ignorada(carga: &Value) -> bool {
    let Some(cwd) = carga.get("cwd").and_then(Value::as_str) else { return false };
    let variable = std::env::var_os(VARIABLE_IGNORAR).map(|v| v.to_string_lossy().into_owned());
    carpetas_ignoradas(variable.as_deref(), &temporal()).iter().any(|c| dentro_de(cwd, c))
}

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
/// `claude.<evento del hook>`, con solo lo necesario (ver `CAMPOS`). Las sesiones que trabajan en una carpeta ignorada
/// (las internas de las apps, ver `carpetas_ignoradas`) no se publican: ni al bus ni a la cola, y sin decir nada.
/// Nunca bloquea a Claude Code: no escribe nada en la salida y siempre termina con 0.
pub fn recibir() -> i32 {
    let mut entrada = String::new();
    if let Err(e) = io::stdin().lock().take(LIMITE_ENTRADA).read_to_string(&mut entrada) {
        eprintln!("ev hook: no pude leer la entrada: {e}");
        return 0;
    }
    let carga: Value = serde_json::from_str(&entrada).unwrap_or(Value::Null);
    if ignorada(&carga) {
        return 0;
    }
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
