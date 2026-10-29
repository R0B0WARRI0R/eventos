use crate::cliente::Cliente;
use crate::evento::{ahora, Evento};
use crate::texto::recortar;
use serde_json::Value;
use std::io::{self, Read};
use std::time::Duration;

const LIMITE_ENTRADA: u64 = 4 << 20;
const LIMITE_CUERPO: usize = 256 * 1024;

/// `ev hook`: adaptador para los hooks de Claude Code. Lee el JSON del hook por la entrada estándar y lo publica como
/// `claude.<evento del hook>`. Nunca bloquea a Claude Code: no escribe nada en la salida y siempre termina con 0.
pub fn recibir() -> i32 {
    let mut entrada = String::new();
    if let Err(e) = io::stdin().lock().take(LIMITE_ENTRADA).read_to_string(&mut entrada) {
        eprintln!("ev hook: no pude leer la entrada: {e}");
        return 0;
    }
    let carga: Value = match serde_json::from_str(&entrada) {
        Ok(v) => v,
        Err(_) => Value::String(entrada.clone()),
    };
    let nombre = carga.get("hook_event_name").and_then(Value::as_str).unwrap_or("hook").to_string();
    let sesion = carga.get("session_id").and_then(Value::as_str).map(str::to_string);
    let cuerpo = if entrada.len() > LIMITE_CUERPO { Value::String(recortar(&entrada, LIMITE_CUERPO)) } else { carga };
    let e = Evento {
        fuente: format!("hook:claude/{}", sesion.as_deref().unwrap_or("?")),
        tipo: format!("claude.{nombre}"),
        cuerpo,
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
