//! Seguir ficheros de registro (enganches, grupo 2a, desde 2026-10-31): cada línea nueva que una app escribe en su
//! registro llega al bus como `registro.linea`. Lo usan `ev tail <ruta>` (lo que XANA lanza en segundo plano mientras
//! trabaja con Victoria en una app, sin tocar configuración) y `ev enganchar` (las entradas `[[fichero]]` de
//! `enganches.toml`). Diseño en `docs/2026-10-30-diseno-enganches.md`, §4.1 y §8.
//!
//! Cómo se lee:
//! - Por defecto se empieza tras la última línea completa: lo que ya había no se publica. Con `desde_inicio`, sí.
//! - Solo salen líneas completas; lo que va detrás del último salto espera en un búfer a que llegue el suyo.
//! - Si el fichero encoge o cambia lo ya leído (su principio o los bytes de justo antes de donde íbamos), se ha rotado
//!   o truncado: se vuelve a leer desde 0. Si aún no existe, o desaparece, se espera a que aparezca y se lee desde 0.
//! - Cada línea se decodifica como en `ev run`: UTF-8 y, si no lo es, la página OEM del sistema (cp850).
//! - El fichero se abre, se lee y se cierra en cada pasada, compartido para leer, escribir y borrar: no estorba a la
//!   app, que puede rotarlo o borrarlo. El tamaño se pide al fichero abierto, que en Windows es el de verdad aunque la
//!   app lo tenga abierto; el que guarda la carpeta se actualiza tarde.
//! - `notify` avisa de los cambios en la carpeta para leer en el acto. Además se sondea cada segundo, porque Windows
//!   puede no avisar mientras la app mantiene el fichero abierto y solo vacía su búfer.
//! - Hay un tope de líneas por segundo por fichero: lo que pasa de ahí se descarta y se resume en un `registro.tope`
//!   con cuántas líneas fueron y cuáles (sus números), para poder ir a leerlas al fichero.

use crate::enganches::{validar_ficheros, Fichero, POR_SEGUNDO_FICHERO};
use crate::enganchar::enviar_en_lotes;
use crate::evento::{ahora, Evento};
use crate::texto::decodificar;
use crate::{absoluta, puerto, Resultado};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde_json::{json, Value};
use std::collections::{HashSet, VecDeque};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

/// Cada cuánto se miran todos los ficheros aunque `notify` no avise.
pub const SONDEO: Duration = Duration::from_secs(1);
/// Lo más que espera una vuelta del bucle antes de mirar si hay que parar.
const VUELTA: Duration = Duration::from_millis(250);
/// Bytes de una línea que se guardan, como en `ev run`; del resto solo se cuenta cuántos eran.
const MAX_LINEA_BYTES: usize = 64 * 1024;
/// Bytes que se leen de un fichero por pasada; si queda más, la siguiente pasada va sin esperar.
const MAX_LECTURA: u64 = 4 << 20;
/// Bytes que se comparan para saber si el fichero sigue siendo el mismo: los primeros y los de justo antes de donde
/// íbamos.
const HUELLA: usize = 64;
const SEGUNDO: Duration = Duration::from_secs(1);

// --- Un fichero ---

/// Una línea completa de un fichero.
#[derive(Debug, Clone, PartialEq)]
pub struct Linea {
    /// Número de línea en el fichero, desde 1. Las vacías no se publican, pero cuentan.
    pub numero: u64,
    pub texto: String,
    /// Se quedó sin su salto porque el fichero se rotó, se truncó o desapareció: es lo último que tenía.
    pub incompleta: bool,
}

/// Por qué se vuelve a leer un fichero desde el principio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motivo {
    /// Es más corto que lo ya leído (truncado, o rotado y vuelto a empezar).
    Encogido,
    /// Lo ya leído ya no está igual: su principio o lo de justo antes de donde íbamos (lo han reescrito o es otro).
    Cambiado,
    /// Ya no está; cuando vuelva a aparecer, se leerá desde 0.
    Desaparecido,
}

impl Motivo {
    pub fn como_texto(self) -> &'static str {
        match self {
            Motivo::Encogido => "encogido",
            Motivo::Cambiado => "cambiado",
            Motivo::Desaparecido => "desaparecido",
        }
    }
}

/// Lo que pasa en una pasada por un fichero, en orden.
#[derive(Debug, Clone, PartialEq)]
pub enum Suceso {
    Linea(Linea),
    Rotado(Motivo),
    /// No se ha podido leer (y no es que no exista). Sale una vez por cada error distinto.
    Error(String),
}

/// Una pasada por un fichero.
#[derive(Debug, Default)]
pub struct Lectura {
    pub sucesos: Vec<Suceso>,
    /// Queda por leer: la siguiente pasada no debe esperar.
    pub atrasado: bool,
}

impl Lectura {
    /// Solo las líneas, para las pruebas.
    pub fn lineas(&self) -> Vec<&Linea> {
        self.sucesos.iter().filter_map(|s| if let Suceso::Linea(l) = s { Some(l) } else { None }).collect()
    }
}

/// Sigue un fichero: por dónde va, lo que espera en el búfer y cómo reconocer que sigue siendo el mismo.
pub struct Seguidor {
    ruta: PathBuf,
    /// Hasta dónde se ha leído; `None` si no está abierto (aún no existe o ha desaparecido).
    pos: Option<u64>,
    /// Líneas completas antes de `pos`.
    lineas: u64,
    /// Los primeros bytes ya leídos, y los de justo antes de `pos`.
    cabeza: Vec<u8>,
    cola: Vec<u8>,
    /// La línea a medias: sus primeros `MAX_LINEA_BYTES` y cuántos bytes lleva en total.
    pendiente: Vec<u8>,
    pendiente_total: usize,
    /// Existía al empezar y no se quiere lo viejo: al abrirlo, hay que ponerse al final.
    saltar_lo_viejo: bool,
    existia: bool,
    /// Líneas que ya tenía al empezar y no se publican.
    saltadas: u64,
    ultimo_error: Option<String>,
}

impl Seguidor {
    /// Empieza a seguir un fichero. Si existe y no se pide `desde_inicio`, se pone ya tras su última línea completa.
    pub fn new(ruta: impl Into<PathBuf>, desde_inicio: bool) -> Seguidor {
        let ruta = ruta.into();
        let existia = ruta.is_file();
        let mut s = Seguidor {
            ruta,
            pos: None,
            lineas: 0,
            cabeza: Vec::new(),
            cola: Vec::new(),
            pendiente: Vec::new(),
            pendiente_total: 0,
            saltar_lo_viejo: existia && !desde_inicio,
            existia,
            saltadas: 0,
            ultimo_error: None,
        };
        // Si ahora no se deja abrir (otro programa lo tiene en exclusiva), se intenta en la primera pasada.
        if s.saltar_lo_viejo {
            if let Ok(mut f) = File::open(&s.ruta) {
                if s.ponerse_al_final(&mut f).is_ok() {
                    s.saltar_lo_viejo = false;
                }
            }
        }
        s
    }

    /// Si el fichero existía al empezar a seguirlo.
    pub fn existia(&self) -> bool {
        self.existia
    }

    /// Las líneas que ya tenía al empezar y no se publican.
    pub fn saltadas(&self) -> u64 {
        self.saltadas
    }

    /// Lee lo nuevo desde la pasada anterior.
    pub fn pasada(&mut self) -> Lectura {
        let mut l = Lectura::default();
        match File::open(&self.ruta) {
            Ok(mut f) => match self.leer(&mut f, &mut l) {
                Ok(()) => self.ultimo_error = None,
                Err(e) => self.error(e, &mut l),
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                self.ultimo_error = None;
                if self.pos.is_some() {
                    self.olvidar(&mut l);
                    l.sucesos.push(Suceso::Rotado(Motivo::Desaparecido));
                }
            }
            Err(e) => self.error(e, &mut l),
        }
        l
    }

    fn error(&mut self, e: io::Error, l: &mut Lectura) {
        let texto = e.to_string();
        if self.ultimo_error.as_deref() != Some(texto.as_str()) {
            l.sucesos.push(Suceso::Error(texto.clone()));
            self.ultimo_error = Some(texto);
        }
    }

    fn leer(&mut self, f: &mut File, l: &mut Lectura) -> io::Result<()> {
        if self.pos.is_none() && self.saltar_lo_viejo {
            self.ponerse_al_final(f)?;
            self.saltar_lo_viejo = false;
        }
        let largo = f.metadata()?.len();
        if let Some(pos) = self.pos {
            let motivo = if largo < pos {
                Some(Motivo::Encogido)
            } else if !self.mismo(f)? {
                Some(Motivo::Cambiado)
            } else {
                None
            };
            if let Some(m) = motivo {
                self.olvidar(l);
                l.sucesos.push(Suceso::Rotado(m));
            }
        }
        let pos = *self.pos.get_or_insert(0);
        if largo > pos {
            let datos = leer_en(f, pos, (largo - pos).min(MAX_LECTURA) as usize)?;
            self.trocear(&datos, l);
            let nueva = pos + datos.len() as u64;
            self.pos = Some(nueva);
            l.atrasado = nueva < largo;
        }
        self.refrescar_huella(f)
    }

    /// Se pone tras la última línea completa: lo que ya había no se publica, y la línea que esté a medias se publicará
    /// entera cuando llegue su salto.
    fn ponerse_al_final(&mut self, f: &mut File) -> io::Result<()> {
        let largo = f.metadata()?.len();
        f.seek(SeekFrom::Start(0))?;
        let mut lector = (&mut *f).take(largo);
        let mut buf = vec![0u8; 64 * 1024];
        let (mut leidos, mut saltos, mut tras_ultimo) = (0u64, 0u64, 0u64);
        loop {
            let n = lector.read(&mut buf)?;
            if n == 0 {
                break;
            }
            for (i, _) in buf[..n].iter().enumerate().filter(|(_, &b)| b == b'\n') {
                saltos += 1;
                tras_ultimo = leidos + i as u64 + 1;
            }
            leidos += n as u64;
        }
        self.pos = Some(tras_ultimo);
        self.lineas = saltos;
        self.saltadas = saltos;
        self.cabeza.clear();
        self.refrescar_huella(f)
    }

    /// Si lo ya leído sigue igual: los primeros bytes y los de justo antes de `pos`.
    fn mismo(&self, f: &mut File) -> io::Result<bool> {
        let pos = self.pos.unwrap_or(0);
        Ok(leer_en(f, 0, self.cabeza.len())? == self.cabeza
            && leer_en(f, pos - self.cola.len() as u64, self.cola.len())? == self.cola)
    }

    fn refrescar_huella(&mut self, f: &mut File) -> io::Result<()> {
        let pos = self.pos.unwrap_or(0);
        let n = (HUELLA as u64).min(pos);
        if (self.cabeza.len() as u64) < n {
            self.cabeza = leer_en(f, 0, n as usize)?;
        }
        self.cola = leer_en(f, pos - n, n as usize)?;
        Ok(())
    }

    /// Vuelve a empezar: lo que esperaba en el búfer era lo último del fichero de antes y sale como línea incompleta.
    fn olvidar(&mut self, l: &mut Lectura) {
        if self.pendiente_total > 0 {
            self.terminar(l, true);
        }
        self.pos = None;
        self.lineas = 0;
        self.cabeza.clear();
        self.cola.clear();
    }

    fn trocear(&mut self, datos: &[u8], l: &mut Lectura) {
        let mut resto = datos;
        while let Some(i) = resto.iter().position(|&b| b == b'\n') {
            self.anadir(&resto[..i]);
            self.terminar(l, false);
            resto = &resto[i + 1..];
        }
        self.anadir(resto);
    }

    fn anadir(&mut self, bytes: &[u8]) {
        let cabe = MAX_LINEA_BYTES.saturating_sub(self.pendiente.len()).min(bytes.len());
        self.pendiente.extend_from_slice(&bytes[..cabe]);
        self.pendiente_total += bytes.len();
    }

    fn terminar(&mut self, l: &mut Lectura, incompleta: bool) {
        self.lineas += 1;
        let total = std::mem::take(&mut self.pendiente_total);
        let bytes = std::mem::take(&mut self.pendiente);
        // La marca de orden de bytes de UTF-8 al principio del fichero no es texto.
        let inicio = if self.lineas == 1 && bytes.starts_with(b"\xEF\xBB\xBF") { 3 } else { 0 };
        let mut fin = bytes.len();
        if fin == total && fin > inicio && bytes[fin - 1] == b'\r' {
            fin -= 1;
        }
        let texto = texto_de(&bytes[inicio..fin], total - inicio - (bytes.len() - fin));
        if !texto.trim().is_empty() {
            l.sucesos.push(Suceso::Linea(Linea { numero: self.lineas, texto, incompleta }));
        }
    }
}

/// Lee hasta `n` bytes desde `desde` (menos si el fichero se acaba antes).
fn leer_en(f: &mut File, desde: u64, n: usize) -> io::Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(n);
    if n > 0 {
        f.seek(SeekFrom::Start(desde))?;
        (&mut *f).take(n as u64).read_to_end(&mut buf)?;
    }
    Ok(buf)
}

/// Decodifica una línea como `ev run`. Si se guardaron menos bytes de los que tenía (`total`), se corta sin partir un
/// carácter UTF-8 (si no, toda la línea se tomaría por cp850) y se avisa del corte.
fn texto_de(bytes: &[u8], total: usize) -> String {
    if total <= bytes.len() {
        return decodificar(bytes);
    }
    let mut trozo = bytes;
    if let Err(e) = std::str::from_utf8(trozo) {
        if e.error_len().is_none() {
            trozo = &trozo[..e.valid_up_to()];
        }
    }
    format!("{}… [recortado: {total} bytes en total]", decodificar(trozo))
}

// --- El tope ---

/// Las líneas que se descartaron seguidas por pasar del tope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Descarte {
    pub descartadas: u64,
    pub desde_linea: u64,
    pub hasta_linea: u64,
}

/// Tope de líneas por segundo de un fichero, en ventana deslizante. Lo que pasa de ahí se descarta y se cuenta. Cuando
/// la racha acaba (vuelve a caber una línea, o pasa un segundo sin descartar), sale su resumen, una vez.
pub struct TopeLineas {
    por_segundo: usize,
    marcas: VecDeque<Instant>,
    racha: Option<(Descarte, Instant)>,
}

impl TopeLineas {
    pub fn new(por_segundo: usize) -> Self {
        TopeLineas { por_segundo: por_segundo.max(1), marcas: VecDeque::new(), racha: None }
    }

    /// Si la línea cabe. Si cabe y antes hubo una racha de descartes, también su resumen, que va antes que ella.
    pub fn admitir(&mut self, linea: u64, ahora: Instant) -> (bool, Option<Descarte>) {
        while self.marcas.front().is_some_and(|t| ahora.duration_since(*t) >= SEGUNDO) {
            self.marcas.pop_front();
        }
        if self.marcas.len() < self.por_segundo {
            self.marcas.push_back(ahora);
            return (true, self.cerrar());
        }
        match &mut self.racha {
            Some((d, t)) => {
                d.descartadas += 1;
                d.hasta_linea = linea;
                *t = ahora;
            }
            None => self.racha = Some((Descarte { descartadas: 1, desde_linea: linea, hasta_linea: linea }, ahora)),
        }
        (false, None)
    }

    /// El resumen de una racha que lleva al menos un segundo sin descartar nada.
    pub fn resumen(&mut self, ahora: Instant) -> Option<Descarte> {
        if self.racha.as_ref().is_some_and(|(_, t)| ahora.duration_since(*t) >= SEGUNDO) {
            self.cerrar()
        } else {
            None
        }
    }

    /// El resumen de la racha en curso, si la hay, ya mismo (antes de una rotación, que reinicia los números).
    pub fn cerrar(&mut self) -> Option<Descarte> {
        self.racha.take().map(|(d, _)| d)
    }
}

// --- Varios ficheros, con notify y sondeo ---

struct Seguido {
    ruta: String,
    app: Option<String>,
    desde_inicio: bool,
    por_segundo: usize,
    /// La ruta tal como se compara con lo que avisa `notify`.
    clave: String,
    carpeta: Option<PathBuf>,
    seguidor: Seguidor,
    tope: TopeLineas,
}

/// Las rutas se comparan como Windows: sin distinguir mayúsculas ni el tipo de barra.
fn clave_de(ruta: &str) -> String {
    if cfg!(windows) {
        ruta.replace('/', "\\").to_lowercase()
    } else {
        ruta.to_string()
    }
}

impl Seguido {
    fn new(f: &Fichero) -> Seguido {
        let ruta = f.ruta.trim().to_string();
        let carpeta = Path::new(&ruta).parent().filter(|c| !c.as_os_str().is_empty()).map(Path::to_path_buf);
        Seguido {
            clave: clave_de(&ruta),
            seguidor: Seguidor::new(&ruta, f.desde_inicio),
            tope: TopeLineas::new(f.por_segundo),
            app: f.app.clone(),
            desde_inicio: f.desde_inicio,
            por_segundo: f.por_segundo,
            carpeta,
            ruta,
        }
    }

    fn evento(&self, tipo: &str, gravedad: &str, confianza: &str, cuerpo: String, atributos: Value) -> Evento {
        let mut atributos = atributos;
        if let Some(m) = atributos.as_object_mut() {
            m.insert("ruta".into(), Value::String(self.ruta.clone()));
            if let Some(app) = &self.app {
                m.insert("app".into(), Value::String(app.clone()));
            }
        }
        Evento {
            fuente: format!("fichero:{}", self.ruta),
            tipo: tipo.into(),
            gravedad: Some(gravedad.into()),
            cuerpo: Value::String(cuerpo),
            atributos,
            t_evento: Some(ahora()),
            confianza: Some(confianza.into()),
            ..Default::default()
        }
    }

    fn inicio(&self) -> Evento {
        let s = &self.seguidor;
        let cuerpo = match (s.existia(), self.desde_inicio) {
            (false, _) => format!("Espero a que aparezca {}; lo leeré desde el principio", self.ruta),
            (true, true) => format!("Sigo {} desde el principio", self.ruta),
            (true, false) => format!("Sigo {} desde el final (las {} líneas que ya tenía no se publican)", self.ruta, s.saltadas()),
        };
        let atributos = json!({
            "existe": s.existia(),
            "desde_inicio": self.desde_inicio,
            "saltadas": s.saltadas(),
            "por_segundo": self.por_segundo,
        });
        self.evento("registro.inicio", "info", "local", cuerpo, atributos)
    }

    fn linea(&self, l: Linea) -> Evento {
        let mut atributos = json!({"linea": l.numero});
        if l.incompleta {
            atributos["incompleta"] = Value::Bool(true);
        }
        // Una línea de registro puede traer texto de terceros (una web, un mensaje, un nombre de fichero): es dato.
        self.evento("registro.linea", "info", "externa", l.texto, atributos)
    }

    fn tope(&self, d: Descarte) -> Evento {
        let cuerpo = format!(
            "Descarto {} líneas de {} (de la {} a la {}): pasaban de {} por segundo",
            d.descartadas, self.ruta, d.desde_linea, d.hasta_linea, self.por_segundo
        );
        let atributos = json!({
            "descartadas": d.descartadas,
            "desde_linea": d.desde_linea,
            "hasta_linea": d.hasta_linea,
            "por_segundo": self.por_segundo,
        });
        self.evento("registro.tope", "aviso", "local", cuerpo, atributos)
    }

    fn rotado(&self, m: Motivo) -> Evento {
        let cuerpo = match m {
            Motivo::Encogido => format!("{} ha encogido (truncado o rotado): lo leo desde el principio", self.ruta),
            Motivo::Cambiado => format!("{} ha cambiado lo que ya había leído (reescrito o rotado): lo leo desde el principio", self.ruta),
            Motivo::Desaparecido => format!("{} ha desaparecido: cuando vuelva, lo leeré desde el principio", self.ruta),
        };
        self.evento("registro.rotado", "info", "local", cuerpo, json!({"motivo": m.como_texto()}))
    }

    fn error(&self, e: &str) -> Evento {
        self.evento("registro.error", "aviso", "local", format!("No puedo leer {}: {e}", self.ruta), json!({"error": e}))
    }

    /// Los eventos de una pasada, en orden, pasando las líneas por el tope.
    fn convertir(&mut self, l: Lectura, ahora: Instant, salida: &mut Vec<Evento>) {
        for s in l.sucesos {
            match s {
                Suceso::Linea(linea) => {
                    let (cabe, resumen) = self.tope.admitir(linea.numero, ahora);
                    if let Some(d) = resumen {
                        salida.push(self.tope(d));
                    }
                    if cabe {
                        salida.push(self.linea(linea));
                    }
                }
                Suceso::Rotado(m) => {
                    if let Some(d) = self.tope.cerrar() {
                        salida.push(self.tope(d));
                    }
                    salida.push(self.rotado(m));
                }
                Suceso::Error(e) => salida.push(self.error(&e)),
            }
        }
    }
}

type Aviso = notify::Result<notify::Event>;

/// Sigue varios ficheros: lee en el acto los que `notify` dice que han cambiado y todos cada `sondeo`.
pub struct Vigilante {
    seguidos: Vec<Seguido>,
    sondeo: Duration,
    proximo: Instant,
    observador: Option<RecommendedWatcher>,
    avisos: Receiver<Aviso>,
    /// Se guarda para que el canal no se cierre aunque no haya observador: así la espera dura lo que tiene que durar.
    _emisor: Sender<Aviso>,
    /// Carpetas que ya vigila `notify`; las que aún no existen se vuelven a probar en cada sondeo.
    vigiladas: HashSet<PathBuf>,
    atrasado: bool,
}

impl Vigilante {
    pub fn new(ficheros: &[Fichero]) -> Self {
        Self::con(ficheros, SONDEO, true)
    }

    /// Con otro sondeo, y con o sin los avisos de `notify` (para las pruebas: cada camino por separado).
    pub fn con(ficheros: &[Fichero], sondeo: Duration, avisos: bool) -> Self {
        let (emisor, receptor) = mpsc::channel();
        let observador = if avisos {
            match notify::recommended_watcher(emisor.clone()) {
                Ok(o) => Some(o),
                Err(e) => {
                    eprintln!("ev: notify no arranca ({e}); miro los ficheros cada {} ms", sondeo.as_millis());
                    None
                }
            }
        } else {
            None
        };
        let mut v = Vigilante {
            seguidos: ficheros.iter().map(Seguido::new).collect(),
            sondeo,
            proximo: Instant::now() + sondeo,
            observador,
            avisos: receptor,
            _emisor: emisor,
            vigiladas: HashSet::new(),
            atrasado: false,
        };
        v.vigilar_carpetas();
        v
    }

    /// Un `registro.inicio` por fichero: qué se sigue y desde dónde. Sale cuando ya está decidido desde dónde se lee,
    /// así que lo que se escriba después de verlo en el bus se publicará.
    pub fn inicios(&self) -> Vec<Evento> {
        self.seguidos.iter().map(Seguido::inicio).collect()
    }

    fn vigilar_carpetas(&mut self) {
        let Some(o) = self.observador.as_mut() else { return };
        for s in &self.seguidos {
            let Some(c) = &s.carpeta else { continue };
            if !self.vigiladas.contains(c) && o.watch(c, RecursiveMode::NonRecursive).is_ok() {
                self.vigiladas.insert(c.clone());
            }
        }
    }

    /// Marca los ficheros de los que avisa `notify`. Devuelve si hay que mirarlos todos (un error, o `notify` pide
    /// volver a mirar porque perdió avisos).
    fn marcar(&self, aviso: Aviso, tocados: &mut [bool]) -> bool {
        match aviso {
            Ok(ev) if !ev.need_rescan() => {
                for p in &ev.paths {
                    let clave = clave_de(&p.to_string_lossy());
                    for (i, s) in self.seguidos.iter().enumerate() {
                        tocados[i] |= s.clave == clave;
                    }
                }
                false
            }
            _ => true,
        }
    }

    /// Espera como mucho `plazo` a que pase algo (un aviso de `notify` o el siguiente sondeo) y lee lo nuevo.
    pub fn vuelta(&mut self, plazo: Duration) -> Vec<Evento> {
        let espera = if self.atrasado { Duration::ZERO } else { plazo.min(self.proximo.saturating_duration_since(Instant::now())) };
        let mut tocados = vec![false; self.seguidos.len()];
        let mut todos = false;
        if let Ok(aviso) = self.avisos.recv_timeout(espera) {
            todos |= self.marcar(aviso, &mut tocados);
            while let Ok(aviso) = self.avisos.try_recv() {
                todos |= self.marcar(aviso, &mut tocados);
            }
        }
        let ahora = Instant::now();
        if self.atrasado || ahora >= self.proximo {
            todos = true;
            self.proximo = ahora + self.sondeo;
            self.vigilar_carpetas();
        }
        self.atrasado = false;
        let mut salida = Vec::new();
        for (i, s) in self.seguidos.iter_mut().enumerate() {
            if todos || tocados[i] {
                let lectura = s.seguidor.pasada();
                self.atrasado |= lectura.atrasado;
                s.convertir(lectura, ahora, &mut salida);
            }
            if let Some(d) = s.tope.resumen(ahora) {
                salida.push(s.tope(d));
            }
        }
        salida
    }
}

/// Lee los ficheros del vigilante y manda sus eventos por `tx` hasta que se encienda `parar` o llegue `hasta`.
pub fn bucle(v: &mut Vigilante, tx: &Sender<Evento>, parar: &AtomicBool, hasta: Option<Instant>) {
    while !parar.load(Ordering::Relaxed) && hasta.is_none_or(|h| Instant::now() < h) {
        for e in v.vuelta(VUELTA) {
            if tx.send(e).is_err() {
                return;
            }
        }
    }
}

// --- ev tail ---

pub const USO: &str = "uso: ev tail <ruta>... [--app SLUG] [--desde-inicio] [--por-segundo N] [--segundos N]";

pub struct Opciones {
    pub ficheros: Vec<Fichero>,
    /// Terminar tras estos segundos; sin él, hasta Ctrl+C.
    pub segundos: Option<u64>,
}

/// Lee los argumentos de `ev tail`. Las rutas relativas se toman desde la carpeta actual.
pub fn opciones(args: &[String]) -> Resultado<Opciones> {
    let (mut rutas, mut app, mut desde_inicio, mut por_segundo, mut segundos) = (Vec::new(), None, false, POR_SEGUNDO_FICHERO, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut valor = |nombre: &str| it.next().cloned().ok_or_else(|| format!("falta el valor de {nombre}\n{USO}"));
        match a.as_str() {
            "--app" => app = Some(valor("--app")?),
            "--desde-inicio" => desde_inicio = true,
            "--por-segundo" => por_segundo = valor("--por-segundo")?.parse().map_err(|_| format!("--por-segundo es un número\n{USO}"))?,
            "--segundos" => segundos = Some(valor("--segundos")?.parse().map_err(|_| format!("--segundos es un número\n{USO}"))?),
            "--" => rutas.extend(it.by_ref().cloned()),
            otro if otro.starts_with("--") => return Err(format!("opción desconocida «{otro}»\n{USO}").into()),
            ruta => rutas.push(ruta.to_string()),
        }
    }
    if rutas.is_empty() {
        return Err(USO.into());
    }
    let ficheros = rutas
        .iter()
        .map(|r| Fichero {
            ruta: if r.trim().is_empty() { String::new() } else { absoluta(PathBuf::from(r.trim())).display().to_string() },
            app: app.clone(),
            desde_inicio,
            por_segundo,
        })
        .collect();
    Ok(Opciones { ficheros, segundos })
}

/// `ev tail`: sigue los ficheros y publica cada línea nueva en el bus hasta Ctrl+C (o `--segundos`). Si el bus no
/// responde, los eventos esperan en la cola en disco, como los de `ev enganchar`.
pub fn ejecutar(o: &Opciones) -> Resultado<()> {
    validar_ficheros(&o.ficheros)?;
    let (tx, rx) = mpsc::channel();
    let enviador = std::thread::spawn(move || enviar_en_lotes(rx, "ev tail"));
    let mut v = Vigilante::new(&o.ficheros);
    for e in v.inicios() {
        eprintln!("ev tail: {}", e.cuerpo_texto());
        let _ = tx.send(e);
    }
    eprintln!("ev tail: publico en el bus del puerto {}; Ctrl+C para terminar", puerto());
    let hasta = o.segundos.map(|s| Instant::now() + Duration::from_secs(s));
    bucle(&mut v, &tx, &AtomicBool::new(false), hasta);
    drop(tx);
    let _ = enviador.join();
    Ok(())
}
