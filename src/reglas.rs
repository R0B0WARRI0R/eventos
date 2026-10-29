use crate::evento::Evento;
use crate::Resultado;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Fichero de reglas (TOML): `max_agentes` y una lista `[[regla]]`.
#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Fichero {
    #[serde(default)]
    pub max_agentes: Option<usize>,
    #[serde(default, rename = "regla")]
    pub reglas: Vec<Regla>,
}

/// Cuando llega un evento que casa, llama a un arnés con un encargo y unos límites.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Regla {
    pub nombre: String,
    /// Tipo exacto del evento.
    pub tipo: String,
    /// Patrón de la fuente; `*` vale por cualquier texto.
    #[serde(default)]
    pub fuente: Option<String>,
    /// Atributos que tienen que valer exactamente esto.
    #[serde(default)]
    pub atributos: BTreeMap<String, Value>,
    /// Atributos que tienen que existir y valer otra cosa.
    #[serde(default)]
    pub atributos_distinto: BTreeMap<String, Value>,
    /// `claude` o `eco` (este último, de prueba: publica `publica` sin lanzar nada).
    pub arnes: String,
    #[serde(default)]
    pub modelo: Option<String>,
    /// En la v0 solo `lectura`.
    #[serde(default = "lectura")]
    pub permisos: String,
    #[serde(default)]
    pub encargo: String,
    #[serde(default)]
    pub directorio: Option<PathBuf>,
    /// Tipo del evento que publica el arnés `eco`.
    #[serde(default)]
    pub publica: Option<String>,
    #[serde(default = "uno")]
    pub a_la_vez: usize,
    #[serde(default = "seis")]
    pub por_hora: usize,
    /// Un evento con esta profundidad o más ya no dispara la regla.
    #[serde(default = "dos")]
    pub profundidad: u32,
    /// Tiempo máximo del agente, en minutos (admite fracciones: 0.5 son 30 s).
    #[serde(default = "diez")]
    pub minutos: f64,
}

fn lectura() -> String {
    "lectura".into()
}
fn uno() -> usize {
    1
}
fn seis() -> usize {
    6
}
fn dos() -> u32 {
    2
}
fn diez() -> f64 {
    10.0
}

pub fn cargar(ruta: &Path) -> Resultado<Fichero> {
    let texto = std::fs::read_to_string(ruta).map_err(|e| format!("no puedo leer {}: {e}", ruta.display()))?;
    let fichero: Fichero = toml::from_str(&texto).map_err(|e| format!("{}: {e}", ruta.display()))?;
    validar(&fichero)?;
    Ok(fichero)
}

pub fn validar(f: &Fichero) -> Resultado<()> {
    let mut nombres = HashSet::new();
    for r in &f.reglas {
        if r.nombre.trim().is_empty() || !nombres.insert(r.nombre.as_str()) {
            return Err(format!("regla sin nombre o con nombre repetido: «{}»", r.nombre).into());
        }
        match r.arnes.as_str() {
            "claude" => {
                if r.permisos != "lectura" {
                    return Err(format!("regla «{}»: la v0 solo lanza agentes de lectura (permisos = \"lectura\")", r.nombre).into());
                }
                if r.encargo.trim().is_empty() {
                    return Err(format!("regla «{}»: falta el encargo", r.nombre).into());
                }
            }
            "eco" => {
                if r.publica.as_deref().map_or(true, |p| p.trim().is_empty()) {
                    return Err(format!("regla «{}»: el arnés eco necesita «publica»", r.nombre).into());
                }
            }
            otro => return Err(format!("regla «{}»: arnés desconocido «{otro}» (claude o eco)", r.nombre).into()),
        }
        if r.a_la_vez == 0 || r.por_hora == 0 {
            return Err(format!("regla «{}»: a_la_vez y por_hora tienen que ser al menos 1", r.nombre).into());
        }
        if !(r.minutos.is_finite() && r.minutos > 0.0 && r.minutos <= 24.0 * 60.0) {
            return Err(format!("regla «{}»: minutos tiene que estar entre 0 y 1440", r.nombre).into());
        }
    }
    if f.max_agentes == Some(0) {
        return Err("max_agentes tiene que ser al menos 1".into());
    }
    Ok(())
}

pub fn coincide(r: &Regla, e: &Evento) -> bool {
    if r.tipo != e.tipo {
        return false;
    }
    if let Some(p) = &r.fuente {
        if !comodin(p, &e.fuente) {
            return false;
        }
    }
    if r.atributos.iter().any(|(k, v)| e.atributo(k) != Some(v)) {
        return false;
    }
    r.atributos_distinto.iter().all(|(k, v)| matches!(e.atributo(k), Some(x) if x != v))
}

/// Coincidencia con `*` como comodín de cualquier secuencia (también vacía).
pub fn comodin(patron: &str, texto: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (patron.chars().collect(), texto.chars().collect());
    let (mut i, mut j) = (0, 0);
    let (mut estrella, mut marca) = (None, 0);
    while j < t.len() {
        if i < p.len() && p[i] != '*' && p[i] == t[j] {
            i += 1;
            j += 1;
        } else if i < p.len() && p[i] == '*' {
            estrella = Some(i);
            marca = j;
            i += 1;
        } else if let Some(e) = estrella {
            i = e + 1;
            marca += 1;
            j = marca;
        } else {
            return false;
        }
    }
    p[i..].iter().all(|c| *c == '*')
}

#[derive(Debug, Clone, PartialEq)]
pub enum Veredicto {
    Disparar,
    /// No hay hueco (a_la_vez o max_agentes): espera en la cola y se lanza cuando termine otro agente.
    EnEspera,
    Omitir(String),
}

const HORA: Duration = Duration::from_secs(3600);

/// Disparos que pueden esperar hueco a la vez; los demás se omiten.
pub const MAX_PENDIENTES: usize = 100;

/// Decide qué reglas se disparan con cada evento y lleva la cuenta de los límites.
pub struct Motor {
    reglas: Vec<Regla>,
    max_agentes: usize,
    estado: Mutex<EstadoMotor>,
}

#[derive(Default)]
struct EstadoMotor {
    activos: usize,
    por_regla: HashMap<String, usize>,
    disparos: HashMap<String, VecDeque<Instant>>,
    pendientes: VecDeque<(Regla, Evento)>,
}

/// Lo que sale de la cola al liberarse un hueco: lo que ya se puede lanzar (con su hueco ocupado) y lo que se
/// descarta porque, mientras esperaba, se le acabó el cupo por hora.
#[derive(Default)]
pub struct Salida {
    pub lanzar: Vec<(Regla, Evento)>,
    pub descartar: Vec<(Regla, Evento, String)>,
}

impl EstadoMotor {
    fn ocupados(&self, regla: &str) -> usize {
        self.por_regla.get(regla).copied().unwrap_or(0)
    }

    /// Disparos de la regla en la última hora (y olvida los anteriores).
    fn en_la_hora(&mut self, regla: &str, ahora: Instant) -> usize {
        let cola = self.disparos.entry(regla.to_string()).or_default();
        while cola.front().is_some_and(|t| ahora.duration_since(*t) >= HORA) {
            cola.pop_front();
        }
        cola.len()
    }

    fn ocupar(&mut self, regla: &str, ahora: Instant) {
        self.disparos.entry(regla.to_string()).or_default().push_back(ahora);
        self.activos += 1;
        *self.por_regla.entry(regla.to_string()).or_default() += 1;
    }
}

impl Motor {
    pub fn new(f: Fichero) -> Self {
        Motor { max_agentes: f.max_agentes.unwrap_or(1), reglas: f.reglas, estado: Mutex::default() }
    }

    pub fn reglas(&self) -> &[Regla] {
        &self.reglas
    }

    pub fn activos(&self) -> usize {
        self.estado.lock().expect("estado del motor").activos
    }

    /// Disparos que esperan hueco.
    pub fn pendientes(&self) -> usize {
        self.estado.lock().expect("estado del motor").pendientes.len()
    }

    /// Las reglas que casan con el evento y qué pasa con cada una. Las que se disparan ocupan su hueco hasta
    /// `terminar`. Pausa, profundidad y cupo por hora omiten; la falta de hueco solo hace esperar: un agente publica
    /// mientras aún ocupa el suyo, y si eso omitiera, nada de lo que publica un agente despertaría a otro.
    /// Los eventos `regla.*` (los que publica el propio bus sobre sus reglas) no se evalúan: así no hay bucles.
    pub fn evaluar(&self, e: &Evento, pausado: bool) -> Vec<(Regla, Veredicto)> {
        if e.tipo.starts_with("regla.") {
            return Vec::new();
        }
        let mut est = self.estado.lock().expect("estado del motor");
        let ahora = Instant::now();
        let mut salida = Vec::new();
        for r in self.reglas.iter().filter(|r| coincide(r, e)) {
            let en_la_hora = est.en_la_hora(&r.nombre, ahora);
            let ocupados = est.ocupados(&r.nombre);
            let veredicto = if pausado {
                Veredicto::Omitir("el bus está en pausa".into())
            } else if e.profundidad >= r.profundidad {
                Veredicto::Omitir(format!("profundidad {} alcanza el límite de la regla ({})", e.profundidad, r.profundidad))
            } else if en_la_hora >= r.por_hora {
                Veredicto::Omitir(format!("{en_la_hora} disparos en la última hora (por_hora = {})", r.por_hora))
            } else if ocupados >= r.a_la_vez || est.activos >= self.max_agentes {
                if est.pendientes.len() >= MAX_PENDIENTES {
                    Veredicto::Omitir(format!("la cola de espera está llena ({MAX_PENDIENTES})"))
                } else {
                    est.pendientes.push_back((r.clone(), e.clone()));
                    Veredicto::EnEspera
                }
            } else {
                est.ocupar(&r.nombre, ahora);
                Veredicto::Disparar
            };
            salida.push((r.clone(), veredicto));
        }
        salida
    }

    /// Libera el hueco de un disparo terminado y devuelve lo que la cola puede lanzar ya.
    pub fn terminar(&self, nombre: &str, pausado: bool) -> Salida {
        let mut est = self.estado.lock().expect("estado del motor");
        est.activos = est.activos.saturating_sub(1);
        if let Some(n) = est.por_regla.get_mut(nombre) {
            *n = n.saturating_sub(1);
        }
        self.sacar(&mut est, pausado)
    }

    /// Lo que la cola puede lanzar ahora (por ejemplo, al reanudar tras una pausa).
    pub fn pendientes_listos(&self, pausado: bool) -> Salida {
        let mut est = self.estado.lock().expect("estado del motor");
        self.sacar(&mut est, pausado)
    }

    fn sacar(&self, est: &mut EstadoMotor, pausado: bool) -> Salida {
        let mut salida = Salida::default();
        if pausado {
            return salida;
        }
        let ahora = Instant::now();
        let mut quedan = VecDeque::new();
        while let Some((r, e)) = est.pendientes.pop_front() {
            let en_la_hora = est.en_la_hora(&r.nombre, ahora);
            if en_la_hora >= r.por_hora {
                let motivo = format!("esperaba hueco y se agotó el cupo: {en_la_hora} disparos en la última hora (por_hora = {})", r.por_hora);
                salida.descartar.push((r, e, motivo));
            } else if est.ocupados(&r.nombre) < r.a_la_vez && est.activos < self.max_agentes {
                est.ocupar(&r.nombre, ahora);
                salida.lanzar.push((r, e));
            } else {
                quedan.push_back((r, e));
            }
        }
        est.pendientes = quedan;
        salida
    }
}
