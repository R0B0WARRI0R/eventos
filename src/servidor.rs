use crate::almacen::{Almacen, Filtro};
use crate::arneses::{self, Bus};
use crate::evento::{ahora, Evento};
use crate::reglas::{self, Fichero, Motor, Regla, Salida, Veredicto};
use crate::tareas;
use crate::texto::redactar_evento;
use crate::{Error, Resultado};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};
use tiny_http::{Header, Method, Request, Response, Server};
use ulid::Ulid;

pub struct Config {
    pub puerto: u16,
    pub carpeta: PathBuf,
    pub reglas: Option<PathBuf>,
}

/// Estado compartido del servicio: el almacén (un solo escritor), el aviso de eventos nuevos y el motor de reglas.
pub struct Nucleo {
    almacen: Mutex<Almacen>,
    ultimo: Mutex<i64>,
    aviso: Condvar,
    motor: Motor,
    pausado: AtomicBool,
    puerto: u16,
    carpeta: PathBuf,
    importando: Mutex<()>,
}

const LIMITE_CUERPO: u64 = 8 << 20;

impl Nucleo {
    pub fn abrir(cfg: &Config, fichero: Fichero) -> Resultado<Arc<Self>> {
        std::fs::create_dir_all(&cfg.carpeta)?;
        let almacen = Almacen::abrir(&cfg.carpeta.join("eventos.db"))?;
        let ultimo = almacen.ultimo_seq()?;
        Ok(Arc::new(Nucleo {
            almacen: Mutex::new(almacen),
            ultimo: Mutex::new(ultimo),
            aviso: Condvar::new(),
            motor: Motor::new(fichero),
            pausado: AtomicBool::new(false),
            puerto: cfg.puerto,
            carpeta: cfg.carpeta.clone(),
            importando: Mutex::new(()),
        }))
    }

    /// Valida, redacta, guarda, avisa y evalúa las reglas. Devuelve el id.
    pub fn ingerir(self: &Arc<Self>, mut e: Evento) -> Resultado<String> {
        e.fuente = e.fuente.trim().to_string();
        e.tipo = e.tipo.trim().to_string();
        if e.fuente.is_empty() || e.tipo.is_empty() {
            return Err("un evento necesita «fuente» y «tipo»".into());
        }
        e.id = Ulid::new().to_string();
        e.t_visto = ahora();
        e.seq = 0;
        redactar_evento(&mut e);
        self.almacen.lock().expect("almacén").insertar(&mut e)?;
        {
            // Dos inserciones simultáneas pueden avisar en otro orden: el último nunca retrocede.
            let mut ultimo = self.ultimo.lock().expect("último");
            *ultimo = (*ultimo).max(e.seq);
            self.aviso.notify_all();
        }
        self.disparar(&e);
        Ok(e.id)
    }

    fn disparar(self: &Arc<Self>, e: &Evento) {
        for (regla, veredicto) in self.motor.evaluar(e, self.pausado.load(Ordering::SeqCst)) {
            match veredicto {
                Veredicto::Omitir(motivo) => self.anotar(sobre(&regla, e, "regla.omitida", json!(motivo))),
                Veredicto::EnEspera => self.anotar(sobre(&regla, e, "regla.en_espera", json!("sin hueco libre: se lanzará cuando termine otro agente"))),
                Veredicto::Disparar => self.lanzar(regla, e.clone()),
            }
        }
    }

    /// Lanza el arnés en su hilo; el hueco ya está ocupado y se libera al terminar.
    fn lanzar(self: &Arc<Self>, regla: Regla, evento: Evento) {
        self.anotar(sobre(&regla, &evento, "regla.disparo", Value::Null));
        let nucleo = Arc::clone(self);
        thread::spawn(move || {
            let _hueco = Hueco { nucleo: Arc::clone(&nucleo), regla: regla.nombre.clone() };
            arneses::lanzar(&nucleo, &regla, &evento);
        });
    }

    /// Lanza lo que la cola deja salir y anota lo que descarta.
    fn atender_cola(self: &Arc<Self>, salida: Salida) {
        for (regla, evento, motivo) in salida.descartar {
            self.anotar(sobre(&regla, &evento, "regla.omitida", json!(motivo)));
        }
        for (regla, evento) in salida.lanzar {
            self.lanzar(regla, evento);
        }
    }

    /// Para los eventos que publica el propio bus: si no se pueden guardar, queda en su registro de errores.
    fn anotar(self: &Arc<Self>, e: Evento) {
        if let Err(err) = self.ingerir(e) {
            eprintln!("ev: no pude guardar un evento del bus: {err}");
        }
    }

    fn consultar(&self, f: &Filtro) -> Resultado<Vec<Evento>> {
        Ok(self.almacen.lock().expect("almacén").consultar(f)?)
    }

    /// Espera eventos nuevos tras `f.despues` (por defecto, el último ahora mismo) hasta `segundos`.
    fn esperar(&self, mut f: Filtro, segundos: u64) -> Resultado<Vec<Evento>> {
        if f.despues.is_none() {
            f.despues = Some(*self.ultimo.lock().expect("último"));
        }
        let fin = Instant::now() + Duration::from_secs(segundos.min(120));
        loop {
            let visto = *self.ultimo.lock().expect("último");
            let encontrados = self.consultar(&f)?;
            let ahora = Instant::now();
            if !encontrados.is_empty() || ahora >= fin {
                return Ok(encontrados);
            }
            let ultimo = self.ultimo.lock().expect("último");
            if *ultimo == visto {
                drop(self.aviso.wait_timeout(ultimo, fin - ahora).expect("aviso"));
            }
        }
    }

    /// Importa la cola de eventos que no llegaron al bus: la actual y cualquier importación a medias que quedara de
    /// antes. Las líneas que no valen no se tiran: van a `pendientes.rechazados.jsonl`.
    fn importar_cola(self: &Arc<Self>) -> Resultado<usize> {
        let _uno_a_la_vez = self.importando.lock().expect("importación");
        let cola = self.carpeta.join("pendientes.jsonl");
        if cola.exists() {
            let apartada = self.carpeta.join(format!("pendientes.importando-{}.jsonl", Ulid::new()));
            // Si un cliente la tiene abierta, Windows no deja moverla: se reintenta en la siguiente vuelta.
            std::fs::rename(&cola, &apartada).map_err(|e| format!("no pude apartar la cola para importarla (se reintentará): {e}"))?;
        }
        let mut ficheros: Vec<PathBuf> = std::fs::read_dir(&self.carpeta)?
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name().and_then(|f| f.to_str()).is_some_and(|f| f.starts_with("pendientes.importando-") && f.ends_with(".jsonl"))
            })
            .collect();
        ficheros.sort(); // el ULID del nombre los ordena por fecha
        let mut n = 0;
        for fichero in ficheros {
            // Por bytes y línea a línea: una línea rota (un byte que no es UTF-8, una escritura a medias) va a
            // rechazados y no detiene ni el fichero ni la cola.
            let bytes = match std::fs::read(&fichero) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("ev: no pude leer {} (se reintentará): {e}", fichero.display());
                    continue;
                }
            };
            let mut rechazadas: Vec<u8> = Vec::new();
            for linea in bytes.split(|b| *b == b'\n') {
                let linea = linea.strip_suffix(b"\r").unwrap_or(linea);
                if linea.iter().all(u8::is_ascii_whitespace) {
                    continue;
                }
                let resultado = std::str::from_utf8(linea)
                    .map_err(|e| -> Error { format!("no es UTF-8: {e}").into() })
                    .and_then(|s| serde_json::from_str::<Evento>(s).map_err(Into::into))
                    .and_then(|e| self.ingerir(e));
                match resultado {
                    Ok(_) => n += 1,
                    Err(err) => {
                        eprintln!("ev: una línea de la cola no vale ({err}); queda en pendientes.rechazados.jsonl");
                        rechazadas.extend_from_slice(linea);
                        rechazadas.push(b'\n');
                    }
                }
            }
            if !rechazadas.is_empty() {
                let guardadas = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(self.carpeta.join("pendientes.rechazados.jsonl"))
                    .and_then(|mut f| f.write_all(&rechazadas));
                if let Err(e) = guardadas {
                    // Sin poder apartar lo malo, el fichero se queda: mejor reimportar lo bueno que perder lo malo.
                    eprintln!("ev: no pude guardar las líneas rechazadas de {}: {e}", fichero.display());
                    continue;
                }
            }
            if let Err(e) = std::fs::remove_file(&fichero) {
                eprintln!("ev: no pude borrar {} tras importarlo: {e}", fichero.display());
            }
        }
        Ok(n)
    }
}

/// Solo este equipo, y nunca desde otra web. Host tiene que ser 127.0.0.1 o localhost con el puerto del bus (contra
/// el DNS rebinding, que dejaría a una web leer el bus) y, si el navegador manda Origin, tiene que ser el propio bus
/// (contra una web que haga POST a 127.0.0.1: inyectar eventos o quitar la pausa). `ev` y curl no mandan Origin.
fn permitido(req: &Request, puerto: u16) -> Result<(), String> {
    let cabecera = |nombre: &'static str| req.headers().iter().find(|h| h.field.equiv(nombre)).map(|h| h.value.as_str().to_string());
    let validos = [format!("127.0.0.1:{puerto}"), format!("localhost:{puerto}")];
    let host = cabecera("Host").unwrap_or_default();
    if !validos.iter().any(|v| v.eq_ignore_ascii_case(&host)) {
        return Err(format!("Host no permitido: «{host}»"));
    }
    if let Some(origen) = cabecera("Origin") {
        if !validos.iter().any(|v| origen.eq_ignore_ascii_case(&format!("http://{v}"))) {
            return Err(format!("Origin no permitido: «{origen}»"));
        }
    }
    Ok(())
}

fn responder_json(req: Request, codigo: u16, cuerpo: &Value, que: &str) {
    let respuesta = Response::from_string(cuerpo.to_string())
        .with_status_code(codigo)
        .with_header(Header::from_bytes(&b"Content-Type"[..], &b"application/json; charset=utf-8"[..]).expect("cabecera fija"));
    if let Err(e) = req.respond(respuesta) {
        eprintln!("ev: no pude responder a {que}: {e}");
    }
}

/// Un evento del propio bus sobre una regla, hijo del evento que la disparó (misma profundidad).
fn sobre(regla: &Regla, e: &Evento, tipo: &str, cuerpo: Value) -> Evento {
    Evento {
        fuente: "bus".into(),
        tipo: tipo.into(),
        cuerpo,
        atributos: json!({ "regla": regla.nombre, "arnes": regla.arnes }),
        causa: Some(e.id.clone()),
        profundidad: e.profundidad,
        confianza: Some("local".into()),
        ..Default::default()
    }
}

/// Libera el hueco del disparo en el motor al terminar el agente, también si su hilo cae, y deja salir la cola.
struct Hueco {
    nucleo: Arc<Nucleo>,
    regla: String,
}

impl Drop for Hueco {
    fn drop(&mut self) {
        let salida = self.nucleo.motor.terminar(&self.regla, self.nucleo.pausado.load(Ordering::SeqCst));
        self.nucleo.atender_cola(salida);
    }
}

impl Bus for Arc<Nucleo> {
    fn publicar(&self, e: Evento) -> String {
        match self.ingerir(e) {
            Ok(id) => id,
            Err(err) => {
                eprintln!("ev: no pude guardar un evento de un agente: {err}");
                String::new()
            }
        }
    }

    fn puerto(&self) -> u16 {
        self.puerto
    }

    fn carpeta(&self) -> PathBuf {
        self.carpeta.clone()
    }
}

pub fn servir(mut cfg: Config) -> Resultado<()> {
    // Absoluta: se pasa al agente y a su `ev mcp`, que arrancan en otro directorio.
    cfg.carpeta = crate::absoluta(cfg.carpeta);
    let fichero = match &cfg.reglas {
        Some(r) => reglas::cargar(r)?,
        None => Fichero::default(),
    };
    let n_reglas = fichero.reglas.len();
    let nucleo = Nucleo::abrir(&cfg, fichero)?;
    let servidor = Server::http(("127.0.0.1", cfg.puerto))
        .map_err(|e| format!("no puedo escuchar en 127.0.0.1:{}: {e}", cfg.puerto))?;
    // Un problema con la cola no impide arrancar: se avisa y se reintenta en la siguiente vuelta.
    let importados = nucleo.importar_cola().unwrap_or_else(|e| {
        eprintln!("ev: {e}");
        0
    });
    // Lo que se encole con el servicio ya en marcha (un cliente con prisa, un envío rechazado) se recoge sin reiniciar.
    let periodo = std::env::var("EV_IMPORTAR_SEGUNDOS").ok().and_then(|s| s.parse().ok()).unwrap_or(30u64).max(1);
    let importador = Arc::clone(&nucleo);
    thread::spawn(move || loop {
        thread::sleep(Duration::from_secs(periodo));
        match importador.importar_cola() {
            Ok(0) => {}
            Ok(n) => eprintln!("ev: {n} evento(s) importados de la cola"),
            Err(e) => eprintln!("ev: {e}"),
        }
    });
    eprintln!(
        "ev: escuchando en http://127.0.0.1:{} · datos en {} · {n_reglas} regla(s) · {importados} evento(s) importados de la cola",
        cfg.puerto,
        cfg.carpeta.display()
    );
    for peticion in servidor.incoming_requests() {
        let n = Arc::clone(&nucleo);
        thread::spawn(move || atender(&n, peticion));
    }
    Ok(())
}

/// El visor (topología, sobres que viajan, lista y detalle por capas), servido desde el propio bus.
const VISOR: &str = include_str!("visor.html");

fn atender(n: &Arc<Nucleo>, mut req: Request) {
    let url = req.url().to_string();
    let (ruta, consulta) = url.split_once('?').unwrap_or((url.as_str(), ""));
    let q = parametros(consulta);
    let metodo = req.method().clone();
    if let Err(motivo) = permitido(&req, n.puerto) {
        responder_json(req, 403, &json!({ "error": motivo }), &format!("{metodo} {ruta}"));
        return;
    }
    if metodo == Method::Get && matches!(ruta, "/" | "/visor") {
        let pagina = Response::from_string(VISOR)
            .with_header(Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).expect("cabecera fija"));
        if let Err(e) = req.respond(pagina) {
            eprintln!("ev: no pude servir el visor: {e}");
        }
        return;
    }
    let resultado: Resultado<Value> = match (&metodo, ruta) {
        (Method::Post, "/eventos") => recibir(n, &mut req),
        (Method::Get, "/eventos") => filtro(&q).and_then(|f| n.consultar(&f)).map(|v| json!(v)),
        (Method::Get, r) if r.starts_with("/cadena/") => {
            let id = &r["/cadena/".len()..];
            n.almacen.lock().expect("almacén").cadena(id).map(|v| json!(v)).map_err(Into::into)
        }
        (Method::Get, "/esperar") => {
            let segundos = q.get("segundos").and_then(|s| s.parse().ok()).unwrap_or(30);
            filtro(&q).and_then(|f| n.esperar(f, segundos)).map(|v| json!(v))
        }
        (Method::Post, "/pausa") => {
            n.pausado.store(true, Ordering::SeqCst);
            Ok(json!({ "pausado": true }))
        }
        (Method::Post, "/reanudar") => {
            n.pausado.store(false, Ordering::SeqCst);
            n.atender_cola(n.motor.pendientes_listos(false));
            Ok(json!({ "pausado": false }))
        }
        (Method::Get, "/estado") => estado(n),
        (Method::Get, "/tareas") => {
            // Una línea por tarea con lo llegado en las últimas `horas` (24 por defecto, una semana como mucho).
            let horas = tareas::horas(q.get("horas").map(String::as_str));
            let desde = tareas::desde(horas, SystemTime::now());
            let eventos = n.almacen.lock().expect("almacén").de_tipos(tareas::TIPOS, &desde, 5000);
            eventos.map(|v| json!(tareas::derivar(&v))).map_err(Into::into)
        }
        _ => Err(format!("ruta desconocida: {metodo} {ruta}").into()),
    };
    let (codigo, cuerpo) = match resultado {
        Ok(v) => (200, v),
        Err(e) if e.to_string().starts_with("ruta desconocida") => (404, json!({ "error": e.to_string() })),
        Err(e) => (400, json!({ "error": e.to_string() })),
    };
    responder_json(req, codigo, &cuerpo, &format!("{metodo} {ruta}"));
}

fn recibir(n: &Arc<Nucleo>, req: &mut Request) -> Resultado<Value> {
    let mut texto = String::new();
    Read::take(req.as_reader(), LIMITE_CUERPO).read_to_string(&mut texto)?;
    let valor: Value = serde_json::from_str(&texto).map_err(|e| format!("JSON inválido: {e}"))?;
    let lista = match valor {
        Value::Array(a) => a,
        otro => vec![otro],
    };
    let mut ids = Vec::with_capacity(lista.len());
    for (i, v) in lista.into_iter().enumerate() {
        let e: Evento = serde_json::from_value(v).map_err(|err| format!("evento {i}: {err}"))?;
        ids.push(n.ingerir(e).map_err(|err| format!("evento {i}: {err}"))?);
    }
    Ok(json!({ "ids": ids }))
}

fn estado(n: &Arc<Nucleo>) -> Resultado<Value> {
    let eventos = n.almacen.lock().expect("almacén").contar()?;
    let reglas: Vec<&str> = n.motor.reglas().iter().map(|r| r.nombre.as_str()).collect();
    Ok(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "eventos": eventos,
        "ultimo": *n.ultimo.lock().expect("último"),
        "pausado": n.pausado.load(Ordering::SeqCst),
        "agentes_activos": n.motor.activos(),
        "pendientes": n.motor.pendientes(),
        "reglas": reglas,
        "reglas_detalle": n.motor.reglas(),
        "carpeta": n.carpeta.display().to_string(),
    }))
}

fn filtro(q: &HashMap<String, String>) -> Resultado<Filtro> {
    let numero = |clave: &str| -> Resultado<Option<i64>> {
        q.get(clave).map(|v| v.parse::<i64>().map_err(|_| format!("«{clave}» no es un número: {v}").into())).transpose()
    };
    let texto = |clave: &str| q.get(clave).filter(|v| !v.is_empty()).cloned();
    Ok(Filtro {
        texto: texto("texto"),
        tipo: texto("tipo"),
        fuente: texto("fuente"),
        correlacion: texto("correlacion"),
        causa: texto("causa"),
        desde: texto("desde"),
        hasta: texto("hasta"),
        despues: numero("despues")?,
        limite: numero("limite")?.map(|l| l.max(1) as usize),
    })
}

fn parametros(consulta: &str) -> HashMap<String, String> {
    consulta
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (decodificar_url(k), decodificar_url(v))
        })
        .collect()
}

fn decodificar_url(s: &str) -> String {
    let bytes = s.as_bytes();
    let hex = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
    let mut salida = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => salida.push(b' '),
            b'%' if i + 2 < bytes.len() => match (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                (Some(alto), Some(bajo)) => {
                    salida.push(alto * 16 + bajo);
                    i += 2;
                }
                _ => salida.push(b'%'),
            },
            b => salida.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&salida).into_owned()
}
