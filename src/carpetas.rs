//! Carpetas vigiladas (enganches, Grupo 2c; diseño en `docs/2026-10-30-diseno-enganches.md` §4.3): cuando aparece un
//! fichero nuevo en una de las carpetas `[[carpeta]]` de `enganches.toml`, un `carpeta.fichero`. Nada más se vigila.
//!
//! `notify` dice qué nombres aparecen (creados o renombrados a ese nombre); el tamaño se mira aquí, en cada `recoger`,
//! porque Windows no siempre avisa mientras un programa escribe. Un fichero se publica una sola vez, cuando lleva
//! `antirrebote_ms` sin cambiar de tamaño: el que se escribe en varios pasos da un evento, y la descarga que se crea
//! como `.crdownload` y se renombra a `.pdf`, uno con el nombre final. Nunca se abre el contenido de un fichero: solo
//! se piden sus metadatos.

use crate::enganches::{Carpeta, Config};
use crate::evento::{self, Evento};
use crate::Resultado;
use notify::event::{ModifyKind, RenameMode};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

/// Ficheros ya publicados que se recuerdan por carpeta para no publicarlos otra vez.
const RECORDADOS: usize = 4096;

/// Si el nombre de un fichero encaja con alguno de los patrones (`*` y `?`, sin distinguir mayúsculas).
pub fn ignorado(nombre: &str, patrones: &[String]) -> bool {
    let nombre: Vec<char> = nombre.to_lowercase().chars().collect();
    patrones.iter().any(|p| encaja(&p.to_lowercase().chars().collect::<Vec<_>>(), &nombre))
}

fn encaja(patron: &[char], nombre: &[char]) -> bool {
    let (mut p, mut n) = (0, 0);
    // La última estrella vista y por dónde iba el nombre al verla, para volver a ella si algo no encaja.
    let mut estrella: Option<(usize, usize)> = None;
    while n < nombre.len() {
        if p < patron.len() && (patron[p] == '?' || patron[p] == nombre[n]) {
            p += 1;
            n += 1;
        } else if p < patron.len() && patron[p] == '*' {
            estrella = Some((p, n));
            p += 1;
        } else if let Some((ep, en)) = estrella {
            p = ep + 1;
            n = en + 1;
            estrella = Some((ep, en + 1));
        } else {
            return false;
        }
    }
    patron[p..].iter().all(|&c| c == '*')
}

#[derive(Debug)]
struct Pendiente {
    /// El último tamaño visto; `None` hasta la primera vez que se mira.
    bytes: Option<u64>,
    /// Desde cuándo no cambia.
    desde: Instant,
}

/// Lo que se sabe de una carpeta: los ficheros nuevos que esperan el antirrebote y los ya publicados.
struct Vigilada {
    conf: Carpeta,
    raiz: PathBuf,
    canonica: Option<PathBuf>,
    antirrebote: Duration,
    pendientes: HashMap<PathBuf, Pendiente>,
    publicados: HashSet<PathBuf>,
    orden: VecDeque<PathBuf>,
}

impl Vigilada {
    fn new(conf: Carpeta, raiz: PathBuf) -> Self {
        Vigilada {
            antirrebote: Duration::from_millis(conf.antirrebote_ms),
            conf,
            canonica: std::fs::canonicalize(&raiz).ok().filter(|c| *c != raiz),
            raiz,
            pendientes: HashMap::new(),
            publicados: HashSet::new(),
            orden: VecDeque::new(),
        }
    }

    /// La ruta dentro de la carpeta. `notify` la da colgando de la ruta que se le pasó; por si la diera en su forma
    /// canónica, se prueba también con esa.
    fn relativa<'a>(&self, p: &'a Path) -> Option<&'a Path> {
        p.strip_prefix(&self.raiz).ok().or_else(|| self.canonica.as_ref().and_then(|c| p.strip_prefix(c).ok()))
    }

    /// Un fichero de esta carpeta (o de sus subcarpetas, si es recursiva) cuyo nombre no se ignora.
    fn en_alcance(&self, p: &Path) -> bool {
        let Some(relativa) = self.relativa(p) else { return false };
        let niveles = relativa.components().count();
        if niveles == 0 || (!self.conf.recursiva && niveles > 1) {
            return false;
        }
        let Some(nombre) = relativa.file_name() else { return false };
        !ignorado(&nombre.to_string_lossy(), &self.conf.ignorar)
    }

    /// Un nombre que acaba de aparecer: empieza a esperar su antirrebote.
    fn candidato(&mut self, p: &Path, ahora: Instant) {
        if self.publicados.contains(p) || !self.en_alcance(p) {
            return;
        }
        self.pendientes
            .entry(p.to_path_buf())
            .and_modify(|x| x.desde = ahora)
            .or_insert(Pendiente { bytes: None, desde: ahora });
    }

    /// Un nombre que deja de existir (borrado o renombrado a otro). Si vuelve a aparecer, es nuevo otra vez.
    fn olvidar(&mut self, p: &Path) {
        self.pendientes.remove(p);
        self.publicados.remove(p);
    }

    fn aviso(&mut self, e: &notify::Event, ahora: Instant) {
        match &e.kind {
            EventKind::Create(_) | EventKind::Modify(ModifyKind::Name(RenameMode::To)) => {
                for p in &e.paths {
                    self.candidato(p, ahora);
                }
            }
            EventKind::Modify(ModifyKind::Name(RenameMode::From)) | EventKind::Remove(_) => {
                for p in &e.paths {
                    self.olvidar(p);
                }
            }
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => {
                if let [de, a, ..] = e.paths.as_slice() {
                    self.olvidar(de);
                    self.candidato(a, ahora);
                }
            }
            // Un renombrado sin saber de qué lado: el nombre que existe es el nuevo.
            EventKind::Modify(ModifyKind::Name(_)) => {
                for p in &e.paths {
                    if p.exists() {
                        self.candidato(p, ahora);
                    } else {
                        self.olvidar(p);
                    }
                }
            }
            // Escrituras: solo cuentan para los que ya esperan; un fichero que ya estaba no es nuevo.
            EventKind::Modify(_) => {
                for p in &e.paths {
                    if let Some(x) = self.pendientes.get_mut(p) {
                        x.desde = ahora;
                    }
                }
            }
            _ => {}
        }
    }

    /// Mira el tamaño de los pendientes y publica los que llevan el antirrebote sin cambiar.
    fn revisar(&mut self, ahora: Instant) -> Vec<Evento> {
        let antirrebote = self.antirrebote;
        let mut listos = Vec::new();
        self.pendientes.retain(|p, x| match std::fs::metadata(p) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            // No se deja mirar (lo tiene alguien bloqueado): se cuenta como que sigue cambiando.
            Err(_) => {
                x.desde = ahora;
                true
            }
            Ok(m) if !m.is_file() => false,
            Ok(m) => {
                let bytes = m.len();
                if x.bytes != Some(bytes) {
                    x.bytes = Some(bytes);
                    x.desde = ahora;
                    true
                } else if ahora.duration_since(x.desde) >= antirrebote {
                    listos.push((p.clone(), bytes));
                    false
                } else {
                    true
                }
            }
        });
        listos.sort();
        let mut salida = Vec::with_capacity(listos.len());
        for (p, bytes) in listos {
            salida.push(self.publicable(&p, bytes));
            self.publicados.insert(p.clone());
            self.orden.push_back(p);
            while self.orden.len() > RECORDADOS {
                if let Some(viejo) = self.orden.pop_front() {
                    self.publicados.remove(&viejo);
                }
            }
        }
        salida
    }

    fn publicable(&self, p: &Path, bytes: u64) -> Evento {
        let relativa = self.relativa(p).unwrap_or(p).display().to_string();
        let nombre = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        Evento {
            fuente: format!("carpeta:{}", self.conf.ruta),
            tipo: "carpeta.fichero".into(),
            gravedad: Some("info".into()),
            cuerpo: Value::String(format!("{relativa} ({bytes} bytes)")),
            atributos: json!({"carpeta": self.conf.ruta, "relativa": relativa, "nombre": nombre, "bytes": bytes}),
            t_evento: Some(evento::ahora()),
            confianza: Some("local".into()),
            ..Default::default()
        }
    }
}

/// Las carpetas de la configuración, vigiladas desde que se crea hasta que se suelta.
pub struct Carpetas {
    vigiladas: Vec<Vigilada>,
    rx: Receiver<(usize, notify::Result<notify::Event>)>,
    /// Mientras vivan, `notify` sigue mirando.
    _vigias: Vec<RecommendedWatcher>,
}

impl Carpetas {
    /// Empieza a vigilar las carpetas de la configuración (ya validada). Lo que aparezca desde aquí cuenta.
    pub fn new(c: &Config) -> Resultado<Self> {
        let (tx, rx) = mpsc::channel();
        let mut vigias = Vec::new();
        let mut vigiladas = Vec::new();
        for (i, conf) in c.carpetas.iter().enumerate() {
            let raiz = crate::absoluta(PathBuf::from(&conf.ruta));
            let tx = tx.clone();
            let mut vigia = notify::recommended_watcher(move |r: notify::Result<notify::Event>| {
                let _ = tx.send((i, r));
            })
            .map_err(|e| format!("carpeta «{}»: no puedo vigilarla: {e}", conf.ruta))?;
            let modo = if conf.recursiva { RecursiveMode::Recursive } else { RecursiveMode::NonRecursive };
            vigia.watch(&raiz, modo).map_err(|e| format!("carpeta «{}»: no puedo vigilarla: {e}", conf.ruta))?;
            vigias.push(vigia);
            vigiladas.push(Vigilada::new(conf.clone(), raiz));
        }
        Ok(Carpetas { vigiladas, rx, _vigias: vigias })
    }

    pub fn is_empty(&self) -> bool {
        self.vigiladas.is_empty()
    }

    /// Las rutas vigiladas, como están en la configuración.
    pub fn rutas(&self) -> Vec<&str> {
        self.vigiladas.iter().map(|v| v.conf.ruta.as_str()).collect()
    }

    /// Un `enganche.tanteo` por carpeta, para que conste qué se vigila y cómo.
    pub fn tanteos(&self) -> Vec<Evento> {
        self.vigiladas
            .iter()
            .map(|v| {
                let k = &v.conf;
                let sub = if k.recursiva { " y sus subcarpetas" } else { "" };
                let mut e = Evento::nuevo(
                    format!("enganche:carpeta:{}", k.ruta),
                    "enganche.tanteo",
                    Value::String(format!("Vigilo los ficheros nuevos de {}{sub}", k.ruta)),
                );
                e.gravedad = Some("info".into());
                e.atributos = json!({
                    "carpeta": k.ruta,
                    "recursiva": k.recursiva,
                    "ignorar": k.ignorar,
                    "antirrebote_ms": k.antirrebote_ms,
                });
                e.t_evento = Some(evento::ahora());
                e.confianza = Some("local".into());
                e
            })
            .collect()
    }

    /// Recoge lo que ha visto `notify` y devuelve los ficheros nuevos que ya llevan su antirrebote sin cambiar.
    /// Hay que llamarla a menudo (el antirrebote se mide entre llamadas).
    pub fn recoger(&mut self, ahora: Instant) -> Vec<Evento> {
        while let Ok((i, r)) = self.rx.try_recv() {
            let Some(v) = self.vigiladas.get_mut(i) else { continue };
            match r {
                Ok(e) => v.aviso(&e, ahora),
                Err(e) => eprintln!("ev enganchar: carpeta «{}»: {e}", v.conf.ruta),
            }
        }
        self.vigiladas.iter_mut().flat_map(|v| v.revisar(ahora)).collect()
    }
}
