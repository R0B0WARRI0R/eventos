//! Suscripciones de una sesión (desde 2026-10-30): el agente dice qué quiere que el bus le avise y `ev mcp` se lo
//! empuja a la sesión por el canal de Claude Code (`notifications/claude/channel`). Viven en el proceso `ev mcp` de esa
//! sesión y mueren con ella. Casan con filtros exactos, como las reglas; nada de modelos que decidan qué avisar.

use crate::evento::Evento;
use crate::reglas::comodin;
use crate::texto::recortar;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

const HORA: Duration = Duration::from_secs(3600);
/// Eventos que se enseñan en un aviso; del resto solo se da la cuenta.
pub const POR_AVISO: usize = 5;

#[derive(Debug, Clone)]
pub struct Suscripcion {
    pub id: String,
    /// Para qué la quiere el agente; vuelve en cada aviso.
    pub motivo: String,
    /// Tipo, fuente y correlación admiten `*` como comodín.
    pub tipo: Option<String>,
    pub fuente: Option<String>,
    pub correlacion: Option<String>,
    /// Si no está vacía, la gravedad del evento tiene que ser una de estas.
    pub gravedad: Vec<String>,
    /// Atributos que tienen que valer exactamente esto.
    pub atributos: BTreeMap<String, Value>,
    /// Texto que tiene que aparecer en el cuerpo (sin distinguir mayúsculas).
    pub texto: Option<String>,
    pub caduca: Instant,
    pub minutos: u64,
    pub por_hora: usize,
    avisos: VecDeque<Instant>,
    pub avisados: usize,
    /// Eventos que casaron con el tope lleno.
    pub callados: usize,
    sin_contar: usize,
    tope_avisado: bool,
}

/// Qué hacer con lo que casó en una vuelta.
#[derive(Debug, PartialEq)]
pub enum Admision {
    /// Avisar; lleva cuántos se callaron antes por el tope.
    Avisar(usize),
    /// El tope se acaba de llenar: un aviso de eso y nada más.
    Tope,
    Callar,
}

fn cadena(args: &Value, clave: &str) -> Option<String> {
    args[clave].as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

impl Suscripcion {
    pub fn desde_args(id: String, args: &Value, ahora: Instant) -> Result<Suscripcion, String> {
        let (tipo, fuente, correlacion, texto) =
            (cadena(args, "tipo"), cadena(args, "fuente"), cadena(args, "correlacion"), cadena(args, "texto"));
        // Sin nada que acote, avisaría de todo lo que pasa en el ordenador (cada hook de cada sesión).
        if [&tipo, &fuente, &correlacion, &texto].iter().all(|c| c.as_deref().map_or(true, |v| v.chars().all(|ch| ch == '*'))) {
            return Err("una suscripción necesita al menos tipo, fuente, correlación o texto (y no solo «*»)".into());
        }
        let gravedad = match &args["gravedad"] {
            Value::Null => Vec::new(),
            Value::String(s) => vec![s.clone()],
            Value::Array(a) => a.iter().filter_map(|g| g.as_str().map(str::to_string)).collect(),
            otro => return Err(format!("«gravedad» tiene que ser texto o lista: {otro}")),
        };
        if let Some(mala) = gravedad.iter().find(|g| !["info", "aviso", "error"].contains(&g.as_str())) {
            return Err(format!("gravedad desconocida «{mala}» (info, aviso o error)"));
        }
        let atributos = match &args["atributos"] {
            Value::Null => BTreeMap::new(),
            Value::Object(m) => m.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            otro => return Err(format!("«atributos» tiene que ser un objeto: {otro}")),
        };
        let minutos = args["minutos"].as_u64().unwrap_or(120).clamp(1, 24 * 60);
        let por_hora = args["por_hora"].as_u64().unwrap_or(6).clamp(1, 60) as usize;
        Ok(Suscripcion {
            id,
            motivo: cadena(args, "motivo").map(|m| recortar(&m, 300)).unwrap_or_default(),
            tipo,
            fuente,
            correlacion,
            gravedad,
            atributos,
            texto: texto.map(|t| t.to_lowercase()),
            caduca: ahora + Duration::from_secs(minutos * 60),
            minutos,
            por_hora,
            avisos: VecDeque::new(),
            avisados: 0,
            callados: 0,
            sin_contar: 0,
            tope_avisado: false,
        })
    }

    pub fn coincide(&self, e: &Evento) -> bool {
        let patron = |p: &Option<String>, valor: Option<&str>| p.as_ref().map_or(true, |p| valor.is_some_and(|v| comodin(p, v)));
        patron(&self.tipo, Some(&e.tipo))
            && patron(&self.fuente, Some(&e.fuente))
            && patron(&self.correlacion, e.correlacion.as_deref())
            && (self.gravedad.is_empty() || e.gravedad.as_ref().is_some_and(|g| self.gravedad.contains(g)))
            && self.atributos.iter().all(|(k, v)| e.atributo(k) == Some(v))
            && self.texto.as_ref().map_or(true, |t| e.cuerpo_texto().to_lowercase().contains(t))
    }

    pub fn caducada(&self, ahora: Instant) -> bool {
        ahora >= self.caduca
    }

    /// Cuenta un aviso contra el tope por hora (una ventana que se desliza).
    pub fn admitir(&mut self, ahora: Instant, eventos: usize) -> Admision {
        while self.avisos.front().is_some_and(|t| ahora.duration_since(*t) >= HORA) {
            self.avisos.pop_front();
        }
        if self.avisos.len() < self.por_hora {
            self.avisos.push_back(ahora);
            self.avisados += 1;
            self.tope_avisado = false;
            return Admision::Avisar(std::mem::take(&mut self.sin_contar));
        }
        self.callados += eventos;
        self.sin_contar += eventos;
        if self.tope_avisado {
            Admision::Callar
        } else {
            self.tope_avisado = true;
            Admision::Tope
        }
    }

    /// El filtro en una línea, para la lista y los avisos.
    pub fn filtro(&self) -> String {
        let mut partes = Vec::new();
        for (nombre, valor) in [("tipo", &self.tipo), ("fuente", &self.fuente), ("correlación", &self.correlacion), ("texto", &self.texto)] {
            if let Some(v) = valor {
                partes.push(format!("{nombre} {v}"));
            }
        }
        if !self.gravedad.is_empty() {
            partes.push(format!("gravedad {}", self.gravedad.join("|")));
        }
        for (k, v) in &self.atributos {
            partes.push(format!("{k}={v}"));
        }
        partes.join(", ")
    }

    pub fn resumen(&self, ahora: Instant) -> Value {
        json!({
            "id": self.id,
            "motivo": self.motivo,
            "filtro": self.filtro(),
            "minutos_restantes": self.caduca.saturating_duration_since(ahora).as_secs().div_ceil(60),
            "por_hora": self.por_hora,
            "avisados": self.avisados,
            "callados": self.callados,
        })
    }

    fn meta(&self) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("suscripcion".into(), json!(self.id));
        if !self.motivo.is_empty() {
            m.insert("motivo".into(), json!(recortar(&self.motivo, 120)));
        }
        m
    }

    /// Los parámetros de `notifications/claude/channel` para lo que casó en una vuelta (en orden de llegada).
    pub fn aviso(&self, eventos: &[&Evento], callados_antes: usize) -> Value {
        let mut meta = self.meta();
        if let Some(primero) = eventos.first() {
            meta.insert("tipo".into(), json!(recortar(&primero.tipo, 80)));
            meta.insert("fuente".into(), json!(recortar(&primero.fuente, 80)));
            meta.insert("evento".into(), json!(primero.id));
            if let Some(c) = &primero.correlacion {
                meta.insert("correlacion".into(), json!(recortar(c, 120)));
            }
        }
        let peor = ["error", "aviso", "info"].into_iter().find(|g| eventos.iter().any(|e| e.gravedad.as_deref() == Some(*g)));
        if let Some(g) = peor {
            meta.insert("gravedad".into(), json!(g));
        }
        meta.insert("eventos".into(), json!(eventos.len().to_string()));
        if callados_antes > 0 {
            meta.insert("callados".into(), json!(callados_antes.to_string()));
        }
        let mut texto = String::new();
        for e in eventos.iter().take(POR_AVISO) {
            let hora = e.t_visto.get(11..19).unwrap_or("");
            let gravedad = e.gravedad.as_deref().map(|g| format!(" · {g}")).unwrap_or_default();
            texto += &format!("[{hora} UTC] {} · {}{gravedad}\n", e.tipo, e.fuente);
            let cuerpo = e.cuerpo_texto();
            if !cuerpo.is_empty() {
                texto += &recortar(&cuerpo, 400);
                texto.push('\n');
            }
        }
        if eventos.len() > POR_AVISO {
            texto += &format!("… y {} más con el mismo filtro (búscalos con buscar).\n", eventos.len() - POR_AVISO);
        }
        if callados_antes > 0 {
            texto += &format!("Antes de este aviso, el tope por hora calló {callados_antes} evento(s) de esta suscripción.\n");
        }
        json!({ "content": texto.trim_end(), "meta": meta })
    }

    pub fn aviso_de_tope(&self) -> Value {
        let mut meta = self.meta();
        meta.insert("tope".into(), json!(self.por_hora.to_string()));
        let texto = format!(
            "La suscripción {} ha llegado a su tope de {} avisos por hora. Lo que case a partir de ahora se cuenta sin \
             avisar hasta que se libere la ventana; está en el bus (buscar).",
            self.id, self.por_hora
        );
        json!({ "content": texto, "meta": meta })
    }

    pub fn aviso_de_caducidad(&self) -> Value {
        let mut meta = self.meta();
        meta.insert("caducada".into(), json!("si"));
        let texto = format!(
            "La suscripción {} ({}) ha caducado tras {} minutos: {} aviso(s), {} callado(s) por el tope. Si aún la \
             necesitas, vuelve a suscribirte.",
            self.id,
            self.filtro(),
            self.minutos,
            self.avisados,
            self.callados
        );
        json!({ "content": texto, "meta": meta })
    }
}
