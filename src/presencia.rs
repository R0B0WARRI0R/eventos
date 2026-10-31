//! Quién está conectado al bus (desde 2026-10-31). Cada `ev mcp` late (`POST /latido`) cada pocos segundos y se
//! despide (`POST /adios`) cuando su sesión cierra la entrada; el servicio lo da por ido si deja de latir durante tres
//! periodos, porque cerrar la ventana de Claude Code mata el proceso sin despedida. Los hooks no sirven para contarlas:
//! esa misma ventana cerrada no deja `SessionEnd` (el 2026-10-31 salían 27 sesiones «abiertas» con una sola viva).
//! Vive en memoria: tras reiniciar el servicio, cada uno reaparece en su siguiente latido.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Periodo por defecto entre latidos, en segundos (`EV_LATIDO_SEGUNDOS` lo cambia en el cliente).
pub const PERIODO: u64 = 15;
/// Latidos perdidos seguidos tras los que se da por ido.
pub const PERDIDOS: u32 = 3;
/// Conectados a la vez como mucho: un cliente que inventara ids no puede llenar la memoria del servicio.
pub const MAXIMO: usize = 256;
const LARGO_ID: usize = 64;
const LARGO_TEXTO: usize = 300;

/// Lo que dice un `ev mcp` de sí mismo en cada latido.
#[derive(Debug, Clone, PartialEq)]
pub struct Latido {
    pub id: String,
    /// `sesion` (Claude Code interactivo, con canal) o `agente` (una regla del bus, `claude -p`, Agent SDK).
    pub clase: String,
    pub pid: Option<u64>,
    /// La sesión de Claude Code (`CLAUDE_CODE_SESSION_ID`), si la hay.
    pub sesion: Option<String>,
    /// La regla que lanzó al agente (`EV_REGLA`), si la hay.
    pub regla: Option<String>,
    /// El cliente MCP que se presentó en `initialize` (p. ej. `claude-code 2.1.316`).
    pub cliente: Option<String>,
    pub directorio: Option<String>,
    pub periodo: u64,
}

fn texto(v: &Value, clave: &str) -> Option<String> {
    v[clave].as_str().map(str::trim).filter(|s| !s.is_empty()).map(|s| s.chars().take(LARGO_TEXTO).collect())
}

impl Latido {
    pub fn desde_json(v: &Value) -> Result<Latido, String> {
        let id = v["id"].as_str().map(str::trim).unwrap_or("");
        if id.is_empty() || id.len() > LARGO_ID || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Err(format!("«id» tiene que ser de 1 a {LARGO_ID} letras, cifras, - o _"));
        }
        let clase = v["clase"].as_str().unwrap_or("");
        if !matches!(clase, "sesion" | "agente") {
            return Err(format!("«clase» tiene que ser sesion o agente, no «{clase}»"));
        }
        let periodo = match &v["periodo"] {
            Value::Null => PERIODO,
            p => p.as_u64().filter(|p| (1..=300).contains(p)).ok_or("«periodo» tiene que ser de 1 a 300 segundos")?,
        };
        Ok(Latido {
            id: id.into(),
            clase: clase.into(),
            pid: v["pid"].as_u64(),
            sesion: texto(v, "sesion"),
            regla: texto(v, "regla"),
            cliente: texto(v, "cliente"),
            directorio: texto(v, "directorio"),
            periodo,
        })
    }
}

struct Entrada {
    latido: Latido,
    /// Primer latido, en UTC.
    desde: String,
    visto: Instant,
}

impl Entrada {
    fn caducada(&self, ahora: Instant) -> bool {
        ahora.saturating_duration_since(self.visto) > Duration::from_secs(self.latido.periodo * PERDIDOS as u64)
    }
}

#[derive(Default)]
pub struct Registro {
    vivos: HashMap<String, Entrada>,
}

impl Registro {
    /// Anota un latido; `desde` es la hora UTC de ahora, que solo se guarda en el primero.
    pub fn latir(&mut self, l: Latido, desde: String, ahora: Instant) -> Result<(), String> {
        self.purgar(ahora);
        if let Some(e) = self.vivos.get_mut(&l.id) {
            e.latido = l;
            e.visto = ahora;
            return Ok(());
        }
        if self.vivos.len() >= MAXIMO {
            return Err(format!("ya hay {MAXIMO} conectados; no caben más"));
        }
        self.vivos.insert(l.id.clone(), Entrada { latido: l, desde, visto: ahora });
        Ok(())
    }

    /// Quita a quien se despide; dice si estaba.
    pub fn adios(&mut self, id: &str) -> bool {
        self.vivos.remove(id).is_some()
    }

    fn purgar(&mut self, ahora: Instant) {
        self.vivos.retain(|_, e| !e.caducada(ahora));
    }

    /// Los conectados, del más antiguo al más reciente.
    pub fn lista(&mut self, ahora: Instant) -> Vec<Value> {
        self.purgar(ahora);
        let mut entradas: Vec<&Entrada> = self.vivos.values().collect();
        entradas.sort_by(|a, b| a.desde.cmp(&b.desde).then_with(|| a.latido.id.cmp(&b.latido.id)));
        entradas
            .into_iter()
            .map(|e| {
                let l = &e.latido;
                json!({
                    "id": l.id, "clase": l.clase, "pid": l.pid, "sesion": l.sesion, "regla": l.regla,
                    "cliente": l.cliente, "directorio": l.directorio, "desde": e.desde,
                    "ultimo_latido_hace_s": ahora.saturating_duration_since(e.visto).as_secs(), "periodo": l.periodo,
                })
            })
            .collect()
    }

    /// Cuántos hay: en total, sesiones y agentes.
    pub fn cuenta(&mut self, ahora: Instant) -> Value {
        self.purgar(ahora);
        let sesiones = self.vivos.values().filter(|e| e.latido.clase == "sesion").count();
        json!({ "total": self.vivos.len(), "sesiones": sesiones, "agentes": self.vivos.len() - sesiones })
    }
}
