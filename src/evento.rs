use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

/// Un evento del bus. Sobre tipo CloudEvents con los dos relojes del modelo de logs de OpenTelemetry.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Evento {
    /// ULID; lo pone el servicio al guardar.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// Orden de llegada (rowid); lo pone el servicio. Sirve de cursor para `esperar`.
    #[serde(default, skip_serializing_if = "es_cero")]
    pub seq: i64,
    pub fuente: String,
    pub tipo: String,
    /// Cuándo ocurrió, si la fuente lo sabe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub t_evento: Option<String>,
    /// Cuándo llegó al bus; lo pone el servicio.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub t_visto: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gravedad: Option<String>,
    /// Texto o JSON. Se guarda tal cual llega (salvo la redacción de secretos).
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub cuerpo: Value,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub atributos: Value,
    /// Agrupa los eventos de una misma ejecución o sesión.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlacion: Option<String>,
    /// Id del evento que provocó este (cadena causal).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub causa: Option<String>,
    /// Cuántos agentes hay entre este evento y uno que nadie provocó. Limita los bucles.
    #[serde(default)]
    pub profundidad: u32,
    /// Procedencia: local, agente, externa o desconocida.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confianza: Option<String>,
}

fn es_cero(n: &i64) -> bool {
    *n == 0
}

impl Evento {
    pub fn nuevo(fuente: impl Into<String>, tipo: impl Into<String>, cuerpo: Value) -> Self {
        Evento { fuente: fuente.into(), tipo: tipo.into(), cuerpo, ..Default::default() }
    }

    /// El cuerpo como texto: la cadena si lo es, el JSON compacto si no.
    pub fn cuerpo_texto(&self) -> String {
        match &self.cuerpo {
            Value::Null => String::new(),
            Value::String(s) => s.clone(),
            otro => otro.to_string(),
        }
    }

    pub fn atributo(&self, clave: &str) -> Option<&Value> {
        self.atributos.as_object().and_then(|m| m.get(clave))
    }
}

/// Hora UTC en RFC 3339 con milisegundos: se ordena igual como texto que como tiempo.
pub fn ahora() -> String {
    formatear(SystemTime::now())
}

pub fn formatear(t: SystemTime) -> String {
    let ms = t.duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
    let (segundos, milis) = (ms.div_euclid(1000), ms.rem_euclid(1000));
    let (dias, s) = (segundos.div_euclid(86_400), segundos.rem_euclid(86_400));
    let (y, m, d) = civil_desde_dias(dias);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{milis:03}Z", s / 3600, s % 3600 / 60, s % 60)
}

/// Días desde 1970-01-01 a fecha civil (algoritmo de Howard Hinnant).
fn civil_desde_dias(dias: i64) -> (i64, u32, u32) {
    let z = dias + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}
