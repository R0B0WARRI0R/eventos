use crate::cliente::{Cliente, Envio};
use crate::evento::{ahora, Evento};
use crate::texto::{decodificar, recortar};
use serde_json::{json, Value};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use ulid::Ulid;

/// Emite los eventos de una ejecución con su fuente y su correlación.
#[derive(Clone)]
struct Emisor {
    fuente: String,
    correlacion: String,
    tx: Sender<Evento>,
}

impl Emisor {
    fn emitir(&self, tipo: &str, cuerpo: Value, atributos: Value, gravedad: Option<&str>) {
        let e = Evento {
            fuente: self.fuente.clone(),
            tipo: tipo.into(),
            cuerpo,
            atributos,
            gravedad: gravedad.map(str::to_string),
            correlacion: Some(self.correlacion.clone()),
            t_evento: Some(ahora()),
            confianza: Some("local".into()),
            ..Default::default()
        };
        // Si el hilo enviador ya terminó, el evento no tiene adónde ir; la ejecución sigue igual.
        let _ = self.tx.send(e);
    }
}

/// `ev run -- <orden>`: lanza la orden, la deja ver en la consola como siempre y guarda cada línea como evento,
/// más el inicio y el final con su código. Devuelve el código de salida de la orden.
pub fn ejecutar(orden: &[String]) -> i32 {
    let Some(programa) = orden.first() else {
        eprintln!("uso: ev run -- <orden> [argumentos...]");
        return 2;
    };
    let (tx, rx) = mpsc::channel();
    let enviador = thread::spawn(move || enviar_en_lotes(rx));
    let emisor = Emisor {
        fuente: format!("proc:{}", recortar(&orden.join(" "), 200)),
        correlacion: Ulid::new().to_string(),
        tx,
    };

    let inicio = Instant::now();
    let mut cmd = Command::new(programa);
    // Python acumula la salida si no escribe en una consola (medido: todas las líneas al final); así llega a su tiempo.
    cmd.args(&orden[1..]).env("PYTHONUNBUFFERED", "1").stdin(Stdio::inherit()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut hijo = match cmd.spawn() {
        Ok(h) => h,
        Err(e) => {
            eprintln!("ev: no se pudo lanzar «{programa}»: {e}");
            emisor.emitir("proceso.error", json!(e.to_string()), json!({ "orden": orden }), Some("error"));
            drop(emisor);
            let _ = enviador.join();
            return 127;
        }
    };
    let directorio = std::env::current_dir().map(|d| d.display().to_string()).unwrap_or_default();
    emisor.emitir("proceso.inicio", Value::Null, json!({ "orden": orden, "directorio": directorio, "pid": hijo.id() }), None);

    let lectores: Vec<JoinHandle<()>> = [
        hijo.stdout.take().map(|s| leer(s, emisor.clone(), "linea.stdout", false)),
        hijo.stderr.take().map(|s| leer(s, emisor.clone(), "linea.stderr", true)),
    ]
    .into_iter()
    .flatten()
    .collect();
    let estado = hijo.wait();
    for l in lectores {
        let _ = l.join();
    }
    let codigo = match estado {
        Ok(s) => s.code().unwrap_or(-1),
        Err(e) => {
            eprintln!("ev: no pude esperar a «{programa}»: {e}");
            -1
        }
    };
    let duracion = inicio.elapsed().as_millis() as u64;
    emisor.emitir("proceso.fin", Value::Null, json!({ "codigo": codigo, "duracion_ms": duracion }),
        (codigo != 0).then_some("error"));
    drop(emisor);
    let _ = enviador.join();
    codigo
}

fn leer(fuente: impl Read + Send + 'static, emisor: Emisor, tipo: &'static str, es_error: bool) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut lector = BufReader::new(fuente);
        let mut linea = Vec::new();
        loop {
            linea.clear();
            match lector.read_until(b'\n', &mut linea) {
                Ok(0) => break,
                Ok(_) => {}
                Err(e) => {
                    emisor.emitir("proceso.aviso", json!(format!("lectura de {tipo}: {e}")), Value::Null, Some("aviso"));
                    break;
                }
            }
            // Eco a la consola tal cual llega. Si la consola ya no está (tubería cerrada), no hay a quién avisar.
            if es_error {
                let _ = io::stderr().lock().write_all(&linea);
            } else {
                let mut out = io::stdout().lock();
                let _ = out.write_all(&linea);
                let _ = out.flush();
            }
            let mut fin = linea.len();
            while fin > 0 && matches!(linea[fin - 1], b'\n' | b'\r') {
                fin -= 1;
            }
            // Una línea gigante (un volcado de 9 MB) se guarda recortada: entera no cabría en un envío.
            let mut trozo = &linea[..fin.min(MAX_LINEA_BYTES)];
            if fin > MAX_LINEA_BYTES {
                // Sin partir un carácter UTF-8: si no, la línea entera se tomaría por cp850.
                if let Err(e) = std::str::from_utf8(trozo) {
                    if e.error_len().is_none() {
                        trozo = &trozo[..e.valid_up_to()];
                    }
                }
            }
            let texto = decodificar(trozo);
            let texto = if fin > MAX_LINEA_BYTES { format!("{texto}… [recortado: {fin} bytes en total]") } else { texto };
            emisor.emitir(tipo, json!(texto), Value::Null, None);
        }
    })
}

/// Bytes de una línea que se guardan; el resto se cuenta pero no se guarda.
const MAX_LINEA_BYTES: usize = 64 * 1024;
/// Tope de un envío (el servicio acepta 8 MB por petición).
const MAX_LOTE_BYTES: usize = 2 << 20;

/// Cómo va el envío de una ejecución. En cuanto el bus falla una vez, todo lo demás va directo a la cola: así, un bus
/// colgado no hace esperar 2 s por lote a quien espera a `ev run`.
struct Envios {
    cliente: Cliente,
    caido: bool,
    avisado: bool,
}

/// Agrupa los eventos (cada 250 ms, cada 200 o cada 2 MB) para no hacer una petición por línea.
fn enviar_en_lotes(rx: mpsc::Receiver<Evento>) {
    let mut envios = Envios { cliente: Cliente::nuevo().con_espera(Duration::from_secs(2)), caido: false, avisado: false };
    let mut lote = Vec::new();
    let mut bytes = 0;
    loop {
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(e) => {
                bytes += e.cuerpo.as_str().map_or(256, |s| s.len() + 256);
                lote.push(e);
                if lote.len() >= 200 || bytes >= MAX_LOTE_BYTES {
                    envios.enviar(&mut lote);
                    bytes = 0;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                envios.enviar(&mut lote);
                bytes = 0;
            }
            Err(RecvTimeoutError::Disconnected) => {
                envios.enviar(&mut lote);
                break;
            }
        }
    }
}

impl Envios {
    fn enviar(&mut self, lote: &mut Vec<Evento>) {
        if lote.is_empty() {
            return;
        }
        let resultado = if self.caido {
            self.cliente.encolar(lote).map(|_| Envio::EnCola { ruta: self.cliente.ruta_cola().clone(), motivo: String::new() })
        } else {
            self.cliente.publicar(lote)
        };
        match resultado {
            Ok(Envio::Publicado(_)) => {}
            Ok(Envio::EnCola { ruta, motivo }) => {
                self.caido = true;
                if !self.avisado {
                    eprintln!("ev: {motivo}; los eventos esperan en {}", ruta.display());
                    self.avisado = true;
                }
            }
            Err(e) => eprintln!("ev: {} eventos no se pudieron ni enviar ni encolar: {e}", lote.len()),
        }
        lote.clear();
    }
}
