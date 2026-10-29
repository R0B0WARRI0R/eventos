//! Una línea por tarea: el estado de cada cosa que se está haciendo, sacado de los eventos del bus. Lo usa el panel de
//! tareas del visor. Una tarea es un grupo de eventos con la misma correlación: una tarea de La Fábrica, una sesión de
//! Claude Code, una orden lanzada con `ev run` o un agente que despertó una regla.

use crate::evento::{formatear, Evento};
use crate::texto::{recortar, redactar};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Los tipos de evento que mueven el estado de una tarea, y la nota que deja un agente para retomarla.
pub const TIPOS: &[&str] = &[
    "fabrica.tarea.inicio",
    "fabrica.tarea.fin",
    "fabrica.puerta.fallo",
    "claude.SessionStart",
    "claude.UserPromptSubmit",
    "claude.Notification",
    "claude.Stop",
    "claude.SessionEnd",
    "proceso.inicio",
    "proceso.fin",
    "proceso.error",
    "agente.lanzado",
    "agente.fin",
    "agente.error",
    "nota.reanudacion",
];

/// Los cinco estados cerrados.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Estado {
    Trabajando,
    NecesitaEntrada,
    ListoParaRevisar,
    Fallido,
    Parado,
}

impl Estado {
    /// Orden en el panel: primero lo que pide atención.
    fn prioridad(self) -> u8 {
        match self {
            Estado::NecesitaEntrada => 0,
            Estado::Fallido => 1,
            Estado::Trabajando => 2,
            Estado::ListoParaRevisar => 3,
            Estado::Parado => 4,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Tarea {
    /// La correlación que agrupa sus eventos.
    pub clave: String,
    /// De dónde sale: `fabrica`, `claude`, `proceso` o `agente`.
    pub origen: String,
    pub titulo: String,
    pub estado: Estado,
    /// Qué pasó en el último cambio de estado.
    pub detalle: String,
    /// Cuándo llegó ese cambio al bus (UTC).
    pub desde: String,
    /// Id del evento de ese cambio, para abrirlo en el visor.
    pub evento: String,
    /// La nota que dejó un agente para retomarla, si la hay.
    pub nota: Option<String>,
    /// Orden de llegada de ese cambio: desempata dentro de un mismo estado (dos eventos pueden compartir milisegundo).
    #[serde(skip)]
    seq: i64,
    /// Orden de llegada de la última vez que se puso a trabajar: una nota de un fallo anterior ya no le vale.
    #[serde(skip)]
    seq_trabajo: i64,
}

/// Las horas que mira `/tareas`: 24 por defecto y una semana como mucho.
pub fn horas(parametro: Option<&str>) -> u64 {
    parametro.and_then(|h| h.parse().ok()).unwrap_or(24).min(24 * 7)
}

/// Desde cuándo mira `/tareas`: `horas` antes de `ahora`, en el formato de `t_visto`.
pub fn desde(horas: u64, ahora: SystemTime) -> String {
    formatear(ahora.checked_sub(Duration::from_secs(horas * 3600)).unwrap_or(UNIX_EPOCH))
}

/// Lo que un evento dice de su tarea.
struct Cambio {
    origen: &'static str,
    titulo: Option<String>,
    estado: Option<(Estado, String)>,
}

fn texto(v: &Value, clave: &str) -> Option<String> {
    v.get(clave).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string)
}

/// Un texto, o una lista de textos unida con espacios: `ev run` publica la orden como lista de argumentos.
fn texto_o_lista(v: &Value, clave: &str) -> Option<String> {
    match v.get(clave)? {
        Value::Array(partes) => Some(partes.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" ")),
        _ => texto(v, clave),
    }
    .filter(|s| !s.is_empty())
}

/// La última carpeta de una ruta, con barras de cualquier tipo.
fn carpeta(ruta: &str) -> String {
    let limpia = ruta.trim_end_matches(['\\', '/']);
    limpia.rsplit(['\\', '/']).next().unwrap_or(limpia).to_string()
}

fn cambio(e: &Evento) -> Option<Cambio> {
    use Estado::*;
    let (a, c) = (&e.atributos, &e.cuerpo);
    let cuerpo = || recortar(&e.cuerpo_texto(), 200);
    let (origen, titulo, estado) = match e.tipo.as_str() {
        "fabrica.tarea.inicio" => {
            let proyecto = texto(a, "proyecto").unwrap_or_default();
            let agente = texto(a, "agente").unwrap_or_default();
            let titulo = format!("La Fábrica · {proyecto}: {}", recortar(&e.cuerpo_texto(), 120));
            ("fabrica", Some(titulo), Some((Trabajando, format!("en curso con {agente}"))))
        }
        "fabrica.tarea.fin" => {
            let estado = if a.get("motivo").is_some_and(|m| !m.is_null()) {
                Parado
            } else if texto(a, "estado").as_deref() == Some("hecha") && a.get("puertas_ok") != Some(&Value::Bool(false)) {
                ListoParaRevisar
            } else {
                Fallido
            };
            ("fabrica", None, Some((estado, cuerpo())))
        }
        // No cambia el estado (ya lo hizo su final), pero la nota del agente se engancha a la tarea por él.
        "fabrica.puerta.fallo" => ("fabrica", None, None),
        "claude.SessionStart" | "claude.UserPromptSubmit" | "claude.Notification" | "claude.Stop" | "claude.SessionEnd" => {
            // Con el principio de la sesión, para distinguir dos sesiones abiertas en la misma carpeta.
            let sesion: String = e.correlacion.as_deref().unwrap_or("").chars().take(6).collect();
            let titulo = texto(c, "cwd").map(|r| format!("Claude Code · {} · {sesion}", carpeta(&r)));
            let estado = match e.tipo.as_str() {
                "claude.SessionStart" => (Parado, "sesión abierta".to_string()),
                "claude.UserPromptSubmit" => (Trabajando, "trabajando".to_string()),
                "claude.Notification" => (NecesitaEntrada, texto(c, "message").unwrap_or_else(|| "te necesita".into())),
                "claude.Stop" => (ListoParaRevisar, "turno terminado".to_string()),
                _ => (Parado, texto(c, "reason").map_or("sesión cerrada".into(), |m| format!("sesión cerrada ({m})"))),
            };
            ("claude", titulo, Some(estado))
        }
        "proceso.inicio" | "proceso.fin" | "proceso.error" => {
            // Redactada: la orden en lista no pasa por la redacción del bus, que mira cada argumento suelto.
            let titulo = texto_o_lista(a, "orden").map(|o| format!("Orden · {}", recortar(&redactar(&o), 120)));
            let estado = match (e.tipo.as_str(), a.get("codigo").and_then(Value::as_i64)) {
                ("proceso.inicio", _) => (Trabajando, "en marcha".to_string()),
                ("proceso.error", _) => (Fallido, cuerpo()),
                (_, Some(0)) => (ListoParaRevisar, "terminó bien".to_string()),
                (_, Some(n)) => (Fallido, format!("terminó con código {n}")),
                (_, None) => (Fallido, "terminó sin código".to_string()),
            };
            ("proceso", titulo, Some(estado))
        }
        "agente.lanzado" | "agente.fin" | "agente.error" => {
            let titulo = texto(a, "regla").map(|r| format!("Agente · {r}"));
            let estado = match e.tipo.as_str() {
                "agente.lanzado" => (Trabajando, "en marcha".to_string()),
                "agente.fin" if a.get("error").and_then(Value::as_bool) != Some(true) => (ListoParaRevisar, "terminó".to_string()),
                _ => (Fallido, cuerpo()),
            };
            ("agente", titulo, Some(estado))
        }
        _ => return None,
    };
    Some(Cambio { origen, titulo, estado })
}

fn titulo_por_defecto(origen: &str, clave: &str) -> String {
    match origen {
        "fabrica" => format!("La Fábrica · {}", clave.trim_start_matches("fabrica:")),
        "claude" => format!("Claude Code · sesión {}", clave.chars().take(8).collect::<String>()),
        "proceso" => format!("Orden · {clave}"),
        _ => format!("Agente · {clave}"),
    }
}

/// Las tareas de estos eventos (en orden de llegada), una por correlación, con lo que pide atención primero y, dentro
/// de cada estado, lo más reciente primero. Una correlación sin ningún cambio de estado no es una tarea.
pub fn derivar(eventos: &[Evento]) -> Vec<Tarea> {
    let mut tareas: HashMap<String, Tarea> = HashMap::new();
    // De cada evento de tarea: a qué tarea pertenece y en qué orden llegó.
    let mut de_evento: HashMap<&str, (String, i64)> = HashMap::new();
    for e in eventos {
        // La nota la publica el agente con su propia correlación: se engancha por su causa, el evento que lo despertó,
        // y solo si la tarea no ha vuelto a trabajar desde entonces (el agente puede tardar más que un reintento).
        if e.tipo == "nota.reanudacion" {
            if let Some((clave, seq_causa)) = e.causa.as_deref().and_then(|c| de_evento.get(c)) {
                if let Some(t) = tareas.get_mut(clave).filter(|t| *seq_causa > t.seq_trabajo) {
                    t.nota = Some(recortar(&e.cuerpo_texto(), 1000));
                }
            }
            continue;
        }
        let Some(c) = cambio(e) else { continue };
        let Some(clave) = e.correlacion.clone().filter(|k| !k.is_empty()) else { continue };
        de_evento.insert(&e.id, (clave.clone(), e.seq));
        let t = tareas.entry(clave.clone()).or_insert_with(|| Tarea {
            clave,
            origen: c.origen.into(),
            titulo: String::new(),
            estado: Estado::Parado,
            detalle: String::new(),
            desde: String::new(),
            evento: String::new(),
            nota: None,
            seq: 0,
            seq_trabajo: 0,
        });
        if let Some(titulo) = c.titulo {
            t.titulo = titulo;
        }
        if let Some((estado, detalle)) = c.estado {
            // Si vuelve a trabajar (un reintento), la nota del fallo anterior ya no vale.
            if estado == Estado::Trabajando {
                t.nota = None;
                t.seq_trabajo = e.seq;
            }
            (t.estado, t.detalle, t.desde, t.evento, t.seq) = (estado, detalle, e.t_visto.clone(), e.id.clone(), e.seq);
        }
    }
    let mut lista: Vec<Tarea> = tareas.into_values().filter(|t| !t.evento.is_empty()).collect();
    for t in &mut lista {
        if t.titulo.is_empty() {
            t.titulo = titulo_por_defecto(&t.origen, &t.clave);
        }
    }
    lista.sort_by(|a, b| a.estado.prioridad().cmp(&b.estado.prioridad()).then_with(|| b.seq.cmp(&a.seq)));
    lista
}
