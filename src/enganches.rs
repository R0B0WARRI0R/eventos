//! Enganches, fase 1 (desde 2026-10-30): conectan al bus apps que no tienen salida propia, mirándolas desde fuera
//! (se abre, se cierra, deja de responder, se cae, cambia de título). Diseño en `docs/2026-10-30-diseno-enganches.md`.
//!
//! Aquí van las piezas puras, sin Win32, para probarlas sin escritorio: la configuración (`enganches.toml`, también
//! los `[[fichero]]` de registro que sigue `tail.rs` desde la fase 2a), la lectura
//! de las caídas del registro de Windows, el estado de cada app (`Vigia`) y los topes (`Limitador`). El proceso que las
//! alimenta con lo que ve Windows es `ev enganchar` (`enganchar.rs`).

use crate::evento::{ahora, Evento};
use crate::texto::recortar;
use crate::Resultado;
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use ulid::Ulid;

/// Windows no da una ventana por colgada hasta que lleva 5 s sin atender mensajes (`IsHungAppWindow`).
pub const GRACIA_WINDOWS: u64 = 5;
/// Respiro tras el último cambio de título antes de publicarlo: los títulos parpadean (progreso, relojes).
const RESPIRO_TITULO: Duration = Duration::from_millis(500);
/// Un evento idéntico al anterior de su fuente y tipo dentro de este margen no se repite.
const MARGEN_REPETIDO: Duration = Duration::from_secs(2);
/// Procesos ya cerrados que se recuerdan para casar con su arranque una caída que llega tarde.
const RECIENTES: usize = 32;
const HORA: Duration = Duration::from_secs(3600);
const SEGUNDO: Duration = Duration::from_secs(1);

/// `enganches.toml`. Nada se vigila si no está aquí.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Tope de eventos por segundo de todos los enganches juntos.
    #[serde(default = "cincuenta")]
    pub por_segundo_global: usize,
    /// Segundos seguidos que una ventana tiene que seguir colgada para avisar, además de los 5 de Windows.
    #[serde(default = "diez")]
    pub colgada_segundos: u64,
    #[serde(default, rename = "app")]
    pub apps: Vec<App>,
    /// Aviso en pantalla para algunos tipos (desde 2026-10-31: la ventanita de avisos de XANA).
    #[serde(default)]
    pub aviso: Option<Aviso>,
    /// Carpetas vigiladas (Grupo 2c): un `carpeta.fichero` por cada fichero nuevo. Las vigila `carpetas.rs`.
    #[serde(default, rename = "carpeta")]
    pub carpetas: Vec<Carpeta>,
    /// Ficheros de registro que se siguen línea a línea (fase 2a, `tail.rs`).
    #[serde(default, rename = "fichero")]
    pub ficheros: Vec<Fichero>,
}

/// Una carpeta vigilada: cada fichero nuevo da un `carpeta.fichero` cuando lleva `antirrebote_ms` sin cambiar de
/// tamaño. Nunca se lee su contenido.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Carpeta {
    /// Tiene que existir y ser una carpeta. Va tal cual en la fuente (`carpeta:<ruta>`).
    pub ruta: String,
    /// Mirar también las subcarpetas.
    #[serde(default)]
    pub recursiva: bool,
    /// Nombres de fichero que no se publican, con `*` y `?`, sin distinguir mayúsculas.
    #[serde(default = "ignorar_por_defecto")]
    pub ignorar: Vec<String>,
    #[serde(default = "dos_mil")]
    pub antirrebote_ms: u64,
    #[serde(default = "ciento_veinte")]
    pub por_hora: usize,
}

/// Lo que dejan a medias los navegadores y Office mientras escriben.
pub const IGNORAR_POR_DEFECTO: &[&str] = &["*.tmp", "*.crdownload", "*.part", "~$*"];

fn ignorar_por_defecto() -> Vec<String> {
    IGNORAR_POR_DEFECTO.iter().map(|p| p.to_string()).collect()
}
fn dos_mil() -> u64 {
    2000
}

/// Un fichero de registro que se sigue (`[[fichero]]`): cada línea nueva es un `registro.linea`.
#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Fichero {
    /// Ruta absoluta del fichero; puede no existir todavía.
    pub ruta: String,
    /// La app a la que pertenece (su slug); va en los atributos y, si la app vigilada está en marcha, en la correlación.
    #[serde(default)]
    pub app: Option<String>,
    /// Leer también lo que ya tenía al empezar; si no, solo lo que se añada después.
    #[serde(default)]
    pub desde_inicio: bool,
    /// Tope de líneas por segundo; lo que pase de ahí se descarta y se resume en un `registro.tope`.
    #[serde(default = "veinte")]
    pub por_segundo: usize,
}

/// Líneas por segundo de un fichero si no se dice otra cosa.
pub const POR_SEGUNDO_FICHERO: usize = 20;
/// Tope de `por_segundo` de un fichero.
pub const POR_SEGUNDO_FICHERO_MAX: usize = 1000;

fn veinte() -> usize {
    POR_SEGUNDO_FICHERO
}

/// Las rutas de los ficheros que se siguen: absolutas, sin repetir (sin distinguir mayúsculas ni el tipo de barra, como
/// Windows), sin comodines y que no sean carpetas; `por_segundo` entre 1 y 1000 y la app, si la hay, con forma de slug.
pub fn validar_ficheros(ficheros: &[Fichero]) -> Resultado<()> {
    let mut vistas = HashSet::new();
    for f in ficheros {
        let ruta = f.ruta.trim();
        if ruta.is_empty() {
            return Err("fichero: falta la ruta".into());
        }
        if ruta.contains(['*', '?']) {
            return Err(format!("fichero «{ruta}»: los comodines (* y ?) aún no se admiten; pon cada fichero aparte").into());
        }
        if !Path::new(ruta).is_absolute() {
            return Err(format!("fichero «{ruta}»: la ruta tiene que ser absoluta").into());
        }
        if Path::new(ruta).is_dir() {
            return Err(format!("fichero «{ruta}»: es una carpeta, no un fichero").into());
        }
        if !vistas.insert(ruta.replace('/', "\\").trim_end_matches('\\').to_lowercase()) {
            return Err(format!("fichero «{ruta}»: está repetido").into());
        }
        if !(1..=POR_SEGUNDO_FICHERO_MAX).contains(&f.por_segundo) {
            return Err(format!("fichero «{ruta}»: por_segundo tiene que estar entre 1 y {POR_SEGUNDO_FICHERO_MAX}").into());
        }
        if let Some(app) = &f.app {
            if !es_slug(app) {
                return Err(format!("fichero «{ruta}»: la app «{app}» no es un slug (a-z, 0-9, - o _)").into());
            }
        }
    }
    Ok(())
}

/// Nombre corto válido para una app: a-z, 0-9, - o _, sin vacío.
pub fn es_slug(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_')
}

/// Una orden que se lanza, sin consola y sin esperarla, con cada evento de estos tipos que sale de verdad (pasados los
/// topes). En la orden, `{cuerpo}`, `{app}` y `{tipo}` se cambian por los del evento; cada elemento es un argumento
/// aparte, sin pasar por ninguna consola, así que el texto del evento no puede convertirse en otra orden.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Aviso {
    pub tipos: Vec<String>,
    pub orden: Vec<String>,
}

/// Los tipos que publican los enganches de la fase 1.
pub const TIPOS_FASE_1: &[&str] =
    &["app.inicio", "app.fin", "app.sin_respuesta", "app.respuesta", "app.caida", "app.titulo", "enganche.tope"];

/// La orden del aviso para este evento, si su tipo lo pide.
pub fn orden_de_aviso(a: &Aviso, e: &Evento) -> Option<Vec<String>> {
    if !a.tipos.contains(&e.tipo) {
        return None;
    }
    let app = e.atributo("app").and_then(Value::as_str).unwrap_or_default();
    let cuerpo = e.cuerpo_texto();
    Some(a.orden.iter().map(|p| p.replace("{cuerpo}", &cuerpo).replace("{app}", app).replace("{tipo}", &e.tipo)).collect())
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct App {
    /// Nombre corto: va en la fuente (`app:<slug>`) y en la correlación de cada arranque.
    pub slug: String,
    pub nombre: String,
    /// Nombre del ejecutable, sin ruta (`Obsidian.exe`); no distingue mayúsculas.
    pub coincide_exe: String,
    /// Solo `fuera` por ahora; `upgrade` llega en la fase 3, con su verificación previa.
    #[serde(default = "fuera")]
    pub modo: String,
    /// Publicar los cambios de título de sus ventanas (`app.titulo`).
    #[serde(default)]
    pub titulo: bool,
    #[serde(default = "ciento_veinte")]
    pub por_hora: usize,
}

fn cincuenta() -> usize {
    50
}
fn diez() -> u64 {
    10
}
fn fuera() -> String {
    "fuera".into()
}
fn ciento_veinte() -> usize {
    120
}

pub fn cargar(ruta: &Path) -> Resultado<Config> {
    let texto = std::fs::read_to_string(ruta).map_err(|e| format!("no puedo leer {}: {e}", ruta.display()))?;
    let config: Config = toml::from_str(&texto).map_err(|e| format!("{}: {e}", ruta.display()))?;
    validar(&config)?;
    Ok(config)
}

pub fn validar(c: &Config) -> Resultado<()> {
    let mut slugs = HashSet::new();
    let mut exes = HashSet::new();
    for a in &c.apps {
        let slug_valido =
            !a.slug.is_empty() && a.slug.chars().all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_');
        if !slug_valido || !slugs.insert(a.slug.as_str()) {
            return Err(format!("app con slug vacío, repetido o con algo que no es a-z, 0-9, - o _: «{}»", a.slug).into());
        }
        if a.nombre.trim().is_empty() {
            return Err(format!("app «{}»: falta el nombre", a.slug).into());
        }
        let exe = a.coincide_exe.to_lowercase();
        if !exe.ends_with(".exe") || exe.contains(['\\', '/']) {
            return Err(format!("app «{}»: coincide_exe es el nombre del ejecutable, sin ruta y acabado en .exe", a.slug).into());
        }
        if !exes.insert(exe) {
            return Err(format!("app «{}»: otra app ya usa el ejecutable «{}»", a.slug, a.coincide_exe).into());
        }
        match a.modo.as_str() {
            "fuera" => {}
            "upgrade" => {
                return Err(format!("app «{}»: el modo upgrade aún no existe (fase 3, con verificación previa)", a.slug).into())
            }
            otro => return Err(format!("app «{}»: modo desconocido «{otro}» (fuera)", a.slug).into()),
        }
        if a.por_hora == 0 {
            return Err(format!("app «{}»: por_hora tiene que ser al menos 1", a.slug).into());
        }
    }
    if c.por_segundo_global == 0 {
        return Err("por_segundo_global tiene que ser al menos 1".into());
    }
    if !(1..=600).contains(&c.colgada_segundos) {
        return Err("colgada_segundos tiene que estar entre 1 y 600".into());
    }
    validar_ficheros(&c.ficheros)?;
    if let Some(a) = &c.aviso {
        if a.orden.first().is_none_or(|p| p.trim().is_empty()) {
            return Err("aviso: falta la orden (el programa y sus argumentos)".into());
        }
        if a.tipos.is_empty() {
            return Err("aviso: faltan los tipos que lo lanzan".into());
        }
        if let Some(malo) = a.tipos.iter().find(|t| !TIPOS_FASE_1.contains(&t.as_str())) {
            return Err(format!("aviso: tipo desconocido «{malo}» ({})", TIPOS_FASE_1.join(", ")).into());
        }
    }
    validar_carpetas(&c.carpetas)
}

fn validar_carpetas(carpetas: &[Carpeta]) -> Resultado<()> {
    // La ruta de verdad de cada carpeta y cómo la escribieron, para decir con cuál choca una repetida.
    let mut vistas: HashMap<String, &str> = HashMap::new();
    for k in carpetas {
        if k.ruta.trim().is_empty() {
            return Err("carpeta con la ruta vacía".into());
        }
        let ruta = Path::new(&k.ruta);
        match std::fs::metadata(ruta) {
            Err(e) => return Err(format!("carpeta «{}»: no existe o no puedo verla ({e})", k.ruta).into()),
            Ok(m) if !m.is_dir() => return Err(format!("carpeta «{}»: no es una carpeta", k.ruta).into()),
            Ok(_) => {}
        }
        let real = std::fs::canonicalize(ruta).unwrap_or_else(|_| crate::absoluta(ruta.to_path_buf()));
        let mut clave = real.to_string_lossy().into_owned();
        if cfg!(windows) {
            clave = clave.to_lowercase();
        }
        if let Some(otra) = vistas.insert(clave, &k.ruta) {
            return Err(format!("carpeta «{}»: repetida, es la misma que «{otra}»", k.ruta).into());
        }
        if k.por_hora == 0 {
            return Err(format!("carpeta «{}»: por_hora tiene que ser al menos 1", k.ruta).into());
        }
    }
    Ok(())
}

// --- Caídas: el registro Application de Windows ---

/// La consulta de la suscripción al canal Application: los cuelgues duros que anota Informe de errores de Windows.
/// Filtra por proveedor y no solo por Id: otros programas escriben avisos normales con el Id 1000 (el servicio del
/// sandbox de Codex, comprobado el 2026-10-30).
pub const CONSULTA_CAIDAS: &str = "*[System[Provider[@Name='Application Error'] and (EventID=1000)]]";

/// Lo que cuenta Windows de una caída (evento 1000 de `Application Error`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Caida {
    /// Ejecutable sin ruta (`AppName`).
    pub exe: String,
    pub version: String,
    /// Módulo donde saltó la excepción.
    pub modulo: String,
    /// Código de la excepción, con `0x` (`0xc0000005`).
    pub excepcion: String,
    pub desplazamiento: String,
    pub pid: Option<u32>,
    pub ruta: String,
    /// Id del informe de Windows.
    pub informe: String,
}

struct Patrones {
    proveedor: Regex,
    id: Regex,
    dato: Regex,
}

fn patrones() -> &'static Patrones {
    static P: OnceLock<Patrones> = OnceLock::new();
    P.get_or_init(|| Patrones {
        proveedor: Regex::new(r#"<Provider\s+Name=['"]([^'"]*)['"]"#).expect("patrón"),
        id: Regex::new(r"<EventID(?:\s[^>]*)?>\s*(\d+)\s*</EventID>").expect("patrón"),
        dato: Regex::new(r#"<Data\s+Name=['"]([^'"]+)['"]\s*(?:/>|>([^<]*)</Data>)"#).expect("patrón"),
    })
}

/// Lee el XML de un evento del registro (el de `EvtRender`). Solo da una caída si es del proveedor `Application Error`
/// con Id 1000, aunque la suscripción ya filtre así: un evento que no lo sea nunca se toma por caída.
pub fn caida_desde_xml(xml: &str) -> Option<Caida> {
    let p = patrones();
    if desescapar(p.proveedor.captures(xml)?.get(1)?.as_str()) != "Application Error" {
        return None;
    }
    if p.id.captures(xml)?.get(1)?.as_str() != "1000" {
        return None;
    }
    let datos: HashMap<String, String> = p
        .dato
        .captures_iter(xml)
        .map(|c| (c[1].to_string(), desescapar(c.get(2).map_or("", |m| m.as_str())).trim().to_string()))
        .collect();
    let dato = |k: &str| datos.get(k).cloned().unwrap_or_default();
    let exe = dato("AppName");
    if exe.is_empty() {
        return None;
    }
    let excepcion = dato("ExceptionCode");
    let pid = datos.get("ProcessId").and_then(|p| match p.strip_prefix("0x").or_else(|| p.strip_prefix("0X")) {
        Some(hex) => u32::from_str_radix(hex, 16).ok(),
        None => p.parse().ok(),
    });
    Some(Caida {
        exe,
        version: dato("AppVersion"),
        modulo: dato("ModuleName"),
        excepcion: if excepcion.is_empty() || excepcion.starts_with("0x") { excepcion } else { format!("0x{excepcion}") },
        desplazamiento: dato("FaultingOffset"),
        pid,
        ruta: dato("AppPath"),
        informe: dato("IntegratorReportId"),
    })
}

/// Las entidades de XML que puede traer el texto de un evento.
fn desescapar(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut salida = String::with_capacity(s.len());
    let mut resto = s;
    while let Some(i) = resto.find('&') {
        salida.push_str(&resto[..i]);
        let tras = &resto[i..];
        let Some(fin) = tras.find(';').filter(|&f| f <= 10) else {
            salida.push('&');
            resto = &tras[1..];
            continue;
        };
        let entidad = &tras[1..fin];
        let caracter = match entidad {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entidad
                .strip_prefix("#x")
                .or_else(|| entidad.strip_prefix("#X"))
                .map(|h| u32::from_str_radix(h, 16))
                .or_else(|| entidad.strip_prefix('#').map(str::parse))
                .and_then(Result::ok)
                .and_then(char::from_u32),
        };
        match caracter {
            Some(c) => {
                salida.push(c);
                resto = &tras[fin + 1..];
            }
            None => {
                salida.push('&');
                resto = &tras[1..];
            }
        }
    }
    salida.push_str(resto);
    salida
}

// --- El estado de cada app ---

/// Un código de salida que es una excepción de Windows (NTSTATUS de error o de aviso: `0xC0000005`, `0x80000003`).
/// `exit(-1)` da `0xFFFFFFFF`, que no lo es.
pub fn es_excepcion(codigo: u32) -> bool {
    matches!(codigo >> 28, 0x8 | 0xC)
}

/// Una ejecución de una app: desde que aparece la primera ventana de uno de sus procesos hasta que terminan todos.
#[derive(Debug)]
struct Arranque {
    id: String,
    pids: HashSet<u32>,
    /// Desde cuándo está colgada cada proceso que lo está.
    colgados: HashMap<u32, Instant>,
    /// Desde cuándo estaba colgada cuando se avisó; `None` si no hay aviso en curso.
    avisada: Option<Instant>,
    ultimo_titulo: Option<String>,
    titulo_pendiente: Option<(String, Instant)>,
}

/// El estado de las apps vigiladas. Recibe lo que ve el proceso de enganches (ventanas, procesos que terminan,
/// ventanas colgadas, caídas del registro) y devuelve los eventos que tocan.
pub struct Vigia {
    apps: Vec<App>,
    colgada: Duration,
    en_marcha: Vec<Option<Arranque>>,
    por_pid: HashMap<u32, usize>,
    /// Procesos ya cerrados: pid, app y arranque.
    recientes: VecDeque<(u32, usize, String)>,
}

impl Vigia {
    pub fn new(c: &Config) -> Self {
        Vigia {
            apps: c.apps.clone(),
            colgada: Duration::from_secs(c.colgada_segundos),
            en_marcha: c.apps.iter().map(|_| None).collect(),
            por_pid: HashMap::new(),
            recientes: VecDeque::new(),
        }
    }

    pub fn apps(&self) -> &[App] {
        &self.apps
    }

    /// La app de un ejecutable, por su nombre sin ruta y sin distinguir mayúsculas.
    pub fn app_de_exe(&self, ruta: &str) -> Option<usize> {
        let nombre = ruta.rsplit(['\\', '/']).next().unwrap_or(ruta).to_lowercase();
        self.apps.iter().position(|a| a.coincide_exe.to_lowercase() == nombre)
    }

    pub fn vigila(&self, pid: u32) -> bool {
        self.por_pid.contains_key(&pid)
    }

    pub fn pids(&self) -> Vec<u32> {
        self.por_pid.keys().copied().collect()
    }

    /// La correlación del arranque en curso de una app (`app:<slug>:<arranque>`), si está abierta. Sirve para atar las
    /// líneas de su registro a la misma ejecución que su `app.inicio`.
    pub fn correlacion_de(&self, slug: &str) -> Option<String> {
        let i = self.apps.iter().position(|a| a.slug == slug)?;
        self.en_marcha[i].as_ref().map(|a| format!("app:{slug}:{}", a.id))
    }

    /// Los eventos de arranque del propio enganche: uno por app, para que conste qué se vigila y cómo.
    pub fn tanteos(&self) -> Vec<Evento> {
        self.apps
            .iter()
            .map(|a| {
                let mut e = Evento::nuevo(
                    format!("enganche:{}", a.slug),
                    "enganche.tanteo",
                    Value::String(format!("Vigilo {} desde fuera: abrir, cerrar, cuelgues y caídas", a.nombre)),
                );
                e.gravedad = Some("info".into());
                e.atributos = json!({"app": a.slug, "exe": a.coincide_exe, "modo": a.modo, "titulo": a.titulo});
                e.t_evento = Some(ahora());
                e.confianza = Some("local".into());
                e
            })
            .collect()
    }

    /// Una ventana visible de nivel superior de un proceso. Si el ejecutable es de una app vigilada, adopta el proceso;
    /// si es el primero de la app, abre un arranque (`app.inicio`). `al_arrancar`: ya estaba abierta al empezar a mirar.
    pub fn ventana(&mut self, pid: u32, ruta: &str, al_arrancar: bool) -> Vec<Evento> {
        let Some(i) = self.app_de_exe(ruta) else { return Vec::new() };
        if self.por_pid.insert(pid, i).is_some() {
            return Vec::new();
        }
        if let Some(a) = self.en_marcha[i].as_mut() {
            a.pids.insert(pid);
            return Vec::new();
        }
        let a = Arranque {
            id: Ulid::new().to_string(),
            pids: HashSet::from([pid]),
            colgados: HashMap::new(),
            avisada: None,
            ultimo_titulo: None,
            titulo_pendiente: None,
        };
        let app = &self.apps[i];
        let cuerpo = if al_arrancar { format!("{} ya estaba en marcha", app.nombre) } else { format!("Se ha abierto {}", app.nombre) };
        let e = evento(app, Some(&a.id), "app.inicio", "info", cuerpo, json!({"pid": pid, "exe": ruta, "al_arrancar": al_arrancar}));
        self.en_marcha[i] = Some(a);
        vec![e]
    }

    /// Un proceso adoptado ha terminado. Cuando terminan todos los de la app, se cierra su arranque (`app.fin`).
    pub fn termina(&mut self, pid: u32, codigo: u32) -> Vec<Evento> {
        let Some(i) = self.por_pid.remove(&pid) else { return Vec::new() };
        let Some(a) = self.en_marcha[i].as_mut() else { return Vec::new() };
        a.pids.remove(&pid);
        a.colgados.remove(&pid);
        self.recientes.push_back((pid, i, a.id.clone()));
        while self.recientes.len() > RECIENTES {
            self.recientes.pop_front();
        }
        if !a.pids.is_empty() {
            return Vec::new();
        }
        let Some(a) = self.en_marcha[i].take() else { return Vec::new() };
        let app = &self.apps[i];
        let excepcion = es_excepcion(codigo);
        let cuerpo = if excepcion {
            format!("Se ha cerrado {} con una excepción (0x{codigo:08X})", app.nombre)
        } else {
            format!("Se ha cerrado {}", app.nombre)
        };
        let atributos = json!({
            "pid": pid,
            "codigo": codigo,
            "codigo_hex": format!("0x{codigo:08X}"),
            "cierre": if excepcion { "excepcion" } else { "normal" },
            "estaba_colgada": a.avisada.is_some(),
        });
        vec![evento(app, Some(&a.id), "app.fin", "info", cuerpo, atributos)]
    }

    /// Si las ventanas de un proceso adoptado están colgadas ahora. Avisa una vez cuando la app lleva `colgada_segundos`
    /// seguidos colgada (`app.sin_respuesta`) y otra cuando vuelve a responder (`app.respuesta`).
    pub fn respuesta(&mut self, pid: u32, colgada: bool, ahora: Instant) -> Vec<Evento> {
        let Some(&i) = self.por_pid.get(&pid) else { return Vec::new() };
        let umbral = self.colgada;
        let Some(a) = self.en_marcha[i].as_mut() else { return Vec::new() };
        if colgada {
            a.colgados.entry(pid).or_insert(ahora);
        } else {
            a.colgados.remove(&pid);
        }
        let desde = a.colgados.values().min().copied();
        let app = &self.apps[i];
        match (desde, a.avisada) {
            (Some(d), None) if ahora.duration_since(d) >= umbral => {
                a.avisada = Some(d);
                let segundos = ahora.duration_since(d).as_secs() + GRACIA_WINDOWS;
                let mut colgados: Vec<u32> = a.colgados.keys().copied().collect();
                colgados.sort_unstable();
                let cuerpo = format!("{} no responde (lleva al menos {segundos} s)", app.nombre);
                vec![evento(app, Some(&a.id), "app.sin_respuesta", "aviso", cuerpo, json!({"pids": colgados, "segundos": segundos}))]
            }
            (None, Some(d)) => {
                a.avisada = None;
                let segundos = ahora.duration_since(d).as_secs() + GRACIA_WINDOWS;
                let cuerpo = format!("{} vuelve a responder (estuvo unos {segundos} s sin responder)", app.nombre);
                vec![evento(app, Some(&a.id), "app.respuesta", "info", cuerpo, json!({"segundos": segundos}))]
            }
            _ => Vec::new(),
        }
    }

    /// El título de una ventana de un proceso adoptado ha cambiado. Se publica tras el respiro, en `tick`.
    pub fn titulo(&mut self, pid: u32, texto: &str, ahora: Instant) {
        let Some(&i) = self.por_pid.get(&pid) else { return };
        if !self.apps[i].titulo {
            return;
        }
        let Some(a) = self.en_marcha[i].as_mut() else { return };
        let texto = recortar(texto.trim(), 300);
        if texto.is_empty() {
            return;
        }
        a.titulo_pendiente = Some((texto, ahora));
    }

    /// Publica los títulos que llevan el respiro sin cambiar, si no son el último publicado.
    pub fn tick(&mut self, ahora: Instant) -> Vec<Evento> {
        let mut salida = Vec::new();
        for (app, arranque) in self.apps.iter().zip(self.en_marcha.iter_mut()) {
            let Some(a) = arranque.as_mut() else { continue };
            if !a.titulo_pendiente.as_ref().is_some_and(|(_, t)| ahora.duration_since(*t) >= RESPIRO_TITULO) {
                continue;
            }
            let Some((texto, _)) = a.titulo_pendiente.take() else { continue };
            if a.ultimo_titulo.as_deref() == Some(texto.as_str()) {
                continue;
            }
            let mut e = evento(app, Some(&a.id), "app.titulo", "info", texto.clone(), json!({}));
            // El título lo pone la app y a veces viene de fuera (el de una página web): es dato de terceros.
            e.confianza = Some("externa".into());
            salida.push(e);
            a.ultimo_titulo = Some(texto);
        }
        salida
    }

    /// Una caída del registro de Windows. Solo cuenta si es de una app vigilada; va al arranque de su proceso si se
    /// conoce (también si ya terminó: el informe de Windows puede llegar después del cierre).
    pub fn caida(&mut self, c: &Caida) -> Vec<Evento> {
        let Some(i) = self.app_de_exe(&c.exe) else { return Vec::new() };
        let arranque = match c.pid {
            Some(pid) if self.por_pid.get(&pid) == Some(&i) => self.en_marcha[i].as_ref().map(|a| a.id.clone()),
            Some(pid) => self.recientes.iter().rev().find(|(p, app, _)| *p == pid && *app == i).map(|(_, _, id)| id.clone()),
            None => self.en_marcha[i].as_ref().map(|a| a.id.clone()),
        };
        let app = &self.apps[i];
        let donde = if c.modulo.is_empty() { String::new() } else { format!(" en {}", c.modulo) };
        let cuerpo = format!("Se ha caído {}: excepción {}{donde}", app.nombre, c.excepcion);
        let atributos = json!({
            "exe": c.exe,
            "version": c.version,
            "modulo": c.modulo,
            "excepcion": c.excepcion,
            "desplazamiento": c.desplazamiento,
            "pid": c.pid,
            "ruta": c.ruta,
            "informe": c.informe,
        });
        vec![evento(app, arranque.as_deref(), "app.caida", "error", cuerpo, atributos)]
    }
}

fn evento(app: &App, arranque: Option<&str>, tipo: &str, gravedad: &str, cuerpo: String, atributos: Value) -> Evento {
    let mut atributos = atributos;
    if let Some(m) = atributos.as_object_mut() {
        m.insert("app".into(), Value::String(app.slug.clone()));
    }
    Evento {
        fuente: format!("app:{}", app.slug),
        tipo: tipo.into(),
        gravedad: Some(gravedad.into()),
        cuerpo: Value::String(cuerpo),
        atributos,
        correlacion: arranque.map(|a| format!("app:{}:{a}", app.slug)),
        t_evento: Some(ahora()),
        confianza: Some("local".into()),
        ..Default::default()
    }
}

// --- Topes ---

#[derive(Debug)]
struct Cupo {
    por_hora: usize,
    marcas: VecDeque<Instant>,
    callados: usize,
    avisado: bool,
}

/// Los topes de los enganches, como los de las suscripciones: por app y hora en ventana deslizante, un tope global por
/// segundo y sin repetir el mismo evento seguido. Al llenarse un tope se publica un `enganche.tope` y lo demás se
/// calla y se cuenta. Las caídas (`error`) son urgentes: solo se les quita la repetición.
pub struct Limitador {
    por_segundo: usize,
    global: VecDeque<Instant>,
    callados_global: usize,
    global_avisado: bool,
    cupos: HashMap<String, Cupo>,
    ultimos: HashMap<(String, String), (String, Instant)>,
}

impl Limitador {
    pub fn new(c: &Config) -> Self {
        let cupo = |por_hora| Cupo { por_hora, marcas: VecDeque::new(), callados: 0, avisado: false };
        let cupos = c
            .apps
            .iter()
            .map(|a| (format!("app:{}", a.slug), cupo(a.por_hora)))
            .chain(c.carpetas.iter().map(|k| (format!("carpeta:{}", k.ruta), cupo(k.por_hora))))
            .collect();
        Limitador {
            por_segundo: c.por_segundo_global,
            global: VecDeque::new(),
            callados_global: 0,
            global_avisado: false,
            cupos,
            ultimos: HashMap::new(),
        }
    }

    /// Lo que sale de verdad por un evento: el evento, un `enganche.tope` o nada.
    pub fn admitir(&mut self, e: Evento, ahora: Instant) -> Vec<Evento> {
        let clave = (e.fuente.clone(), e.tipo.clone());
        let cuerpo = e.cuerpo_texto();
        // Dos líneas iguales de un registro son dos cosas que han pasado, no un evento repetido.
        let repetible = TIPOS_REPETIBLES.contains(&e.tipo.as_str());
        if !repetible && self.ultimos.get(&clave).is_some_and(|(c, t)| *c == cuerpo && ahora.duration_since(*t) < MARGEN_REPETIDO) {
            return Vec::new();
        }
        if e.gravedad.as_deref() != Some("error") {
            while self.global.front().is_some_and(|t| ahora.duration_since(*t) >= SEGUNDO) {
                self.global.pop_front();
            }
            if self.global.len() >= self.por_segundo {
                self.callados_global += 1;
                if self.global_avisado {
                    return Vec::new();
                }
                self.global_avisado = true;
                let cuerpo = format!("Los enganches han llenado su tope de {} eventos por segundo; lo que siga se calla y se cuenta", self.por_segundo);
                return vec![tope("global", cuerpo, json!({"callados": self.callados_global, "por_segundo": self.por_segundo}))];
            }
            if let Some(c) = self.cupos.get_mut(&e.fuente) {
                while c.marcas.front().is_some_and(|t| ahora.duration_since(*t) >= HORA) {
                    c.marcas.pop_front();
                }
                if c.marcas.len() >= c.por_hora {
                    c.callados += 1;
                    if c.avisado {
                        return Vec::new();
                    }
                    c.avisado = true;
                    let slug = e.fuente.strip_prefix("app:").unwrap_or(&e.fuente);
                    let cuerpo = format!("{slug} ha llenado su tope de {} eventos por hora; lo que siga se calla y se cuenta", c.por_hora);
                    return vec![tope(slug, cuerpo, json!({"callados": c.callados, "por_hora": c.por_hora}))];
                }
                c.marcas.push_back(ahora);
                c.avisado = false;
            }
            self.global.push_back(ahora);
            self.global_avisado = false;
        }
        if !repetible {
            self.ultimos.insert(clave, (cuerpo, ahora));
        }
        vec![e]
    }
}

/// Tipos que el `Limitador` no trata como repetición aunque lleguen dos iguales seguidos (sí cuentan para el tope global).
pub const TIPOS_REPETIBLES: &[&str] = &["registro.linea"];

fn tope(slug: &str, cuerpo: String, atributos: Value) -> Evento {
    Evento {
        fuente: format!("enganche:{slug}"),
        tipo: "enganche.tope".into(),
        gravedad: Some("aviso".into()),
        cuerpo: Value::String(cuerpo),
        atributos,
        t_evento: Some(ahora()),
        confianza: Some("local".into()),
        ..Default::default()
    }
}
