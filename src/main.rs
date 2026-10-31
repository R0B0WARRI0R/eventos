use eventos::cliente::{Cliente, Envio};
use eventos::evento::Evento;
use eventos::servidor::{self, Config};
use eventos::texto::recortar;
use eventos::{carpeta_datos, enganchar, hook, mcp, puerto, run, tail, Resultado};
use serde_json::Value;

const AYUDA: &str = "ev — Pulso, el bus de eventos universal (v0)

  ev servir [--puerto N] [--reglas F] [--carpeta D]   arranca el servicio (127.0.0.1:8870 por defecto)
  ev run -- <orden> [args...]       lanza la orden y guarda cada línea, el inicio y el final como eventos
  ev hook                           adaptador para los hooks de Claude Code (JSON por la entrada estándar)
  ev mcp                            servidor MCP por stdio: publicar, buscar, esperar
  ev enganchar [--config F]         vigila desde fuera las apps de enganches.toml (abrir, cerrar, cuelgues, caídas)
                                    y sigue sus ficheros de registro ([[fichero]])
  ev tail <ruta>... [--app SLUG] [--desde-inicio] [--por-segundo N]
                                    sigue ficheros de registro y publica cada línea nueva (registro.linea) hasta
                                    Ctrl+C; empieza al final, espera al fichero si aún no existe y lo relee si se rota
  ev publicar <tipo> [texto...]     publica un evento a mano
  ev buscar <palabras...> [--limite N] [--despues SEQ]
  ev ver [--tipo T] [--fuente F] [--correlacion C] [--causa I] [--desde T] [--hasta T] [--limite N] [--despues SEQ]
                                    sin --despues, los últimos N (100; como mucho 1000); con él, los N primeros
                                    tras ese seq. Si llega al tope, avisa por stderr del seq con el que seguir
  ev cadena <id>                    el árbol causal de un evento
  ev seguir [--tipo T]              muestra los eventos según llegan (Ctrl+C para salir)
  ev estado | ev pausa | ev reanudar

Entorno: EV_PUERTO (8870), EV_DIR (carpeta de datos), EV_CLAUDE (ruta de claude),
EV_HOOK_IGNORAR (más carpetas, separadas por ;, cuyas sesiones no publica ev hook; - quita la de por defecto).";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let codigo = match ejecutar(&args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("ev: {e}");
            1
        }
    };
    std::process::exit(codigo);
}

fn ejecutar(args: &[String]) -> Resultado<i32> {
    let resto = args.get(1..).unwrap_or(&[]);
    match args.first().map(String::as_str) {
        Some("servir") => {
            let cfg = Config {
                puerto: bandera(resto, "--puerto").map(|p| p.parse()).transpose()?.unwrap_or_else(puerto),
                carpeta: bandera(resto, "--carpeta").map(Into::into).unwrap_or_else(carpeta_datos),
                reglas: bandera(resto, "--reglas").map(Into::into),
            };
            servidor::servir(cfg)?;
            Ok(0)
        }
        Some("run") => {
            let orden = match resto.first().map(String::as_str) {
                Some("--") => &resto[1..],
                _ => resto,
            };
            Ok(run::ejecutar(orden))
        }
        Some("hook") => Ok(hook::recibir()),
        Some("enganchar") => {
            let o = enganchar::Opciones {
                config: bandera(resto, "--config").map(Into::into).unwrap_or_else(|| r"D:\dev\eventos\enganches.toml".into()),
                segundos: bandera(resto, "--segundos").map(|s| s.parse()).transpose()?,
            };
            enganchar::ejecutar(&o)?;
            Ok(0)
        }
        Some("tail") => {
            tail::ejecutar(&tail::opciones(resto)?)?;
            Ok(0)
        }
        Some("mcp") => Ok(mcp::servir()),
        Some("publicar") => {
            let tipo = resto.first().ok_or("uso: ev publicar <tipo> [texto...]")?;
            let texto = resto[1..].join(" ");
            let cuerpo = if texto.is_empty() { Value::Null } else { Value::String(texto) };
            let e = Evento { confianza: Some("local".into()), ..Evento::nuevo("cli", tipo.clone(), cuerpo) };
            match Cliente::nuevo().publicar(&[e])? {
                Envio::Publicado(ids) => println!("{}", ids.join(" ")),
                Envio::EnCola { ruta, motivo } => eprintln!("ev: {motivo}; el evento espera en {}", ruta.display()),
            }
            Ok(0)
        }
        Some("buscar") => {
            let texto = posicionales(resto).join(" ");
            if texto.is_empty() {
                return Err("uso: ev buscar <palabras...> [--limite N]".into());
            }
            let mut f = vec![("texto", texto)];
            f.extend(bandera(resto, "--limite").map(|l| ("limite", l)));
            f.extend(bandera(resto, "--despues").map(|s| ("despues", s)));
            consultar_e_imprimir(&f)
        }
        Some("ver") => {
            let claves = ["tipo", "fuente", "correlacion", "causa", "desde", "hasta", "limite", "despues"];
            let f: Vec<(&str, String)> =
                claves.iter().filter_map(|&k| bandera(resto, &format!("--{k}")).map(|v| (k, v))).collect();
            consultar_e_imprimir(&f)
        }
        Some("cadena") => {
            let id = resto.first().ok_or("uso: ev cadena <id>")?;
            imprimir(&Cliente::nuevo().cadena(id)?, true);
            Ok(0)
        }
        Some("seguir") => {
            let cliente = Cliente::nuevo();
            let mut cursor = cliente.estado()?["ultimo"].as_i64().unwrap_or(0);
            let tipo = bandera(resto, "--tipo");
            loop {
                let mut f = vec![("despues", cursor.to_string())];
                f.extend(tipo.clone().map(|t| ("tipo", t)));
                let eventos = cliente.esperar(&f, 60)?;
                if let Some(ultimo) = eventos.last() {
                    cursor = ultimo.seq;
                }
                imprimir(&eventos, false);
            }
        }
        Some("estado") => {
            println!("{}", serde_json::to_string_pretty(&Cliente::nuevo().estado()?)?);
            Ok(0)
        }
        Some("pausa") => {
            Cliente::nuevo().orden("/pausa")?;
            println!("reglas en pausa: no se lanzará ningún agente hasta `ev reanudar`");
            Ok(0)
        }
        Some("reanudar") => {
            Cliente::nuevo().orden("/reanudar")?;
            println!("reglas activas");
            Ok(0)
        }
        Some("-h" | "--help" | "ayuda") | None => {
            println!("{AYUDA}");
            Ok(0)
        }
        Some(otro) => {
            eprintln!("orden desconocida: {otro}\n\n{AYUDA}");
            Ok(2)
        }
    }
}

fn bandera(args: &[String], nombre: &str) -> Option<String> {
    args.iter().position(|a| a == nombre).and_then(|i| args.get(i + 1)).cloned()
}

fn posicionales(args: &[String]) -> Vec<String> {
    let mut salida = Vec::new();
    let mut saltar = false;
    for a in args {
        if saltar {
            saltar = false;
        } else if a.starts_with("--") {
            saltar = true;
        } else {
            salida.push(a.clone());
        }
    }
    salida
}

/// Tope de eventos de una consulta, como lo aplica el almacén: `limite` (100 si no se da) entre 1 y 1000.
const TOPE_POR_DEFECTO: usize = 100;
const TOPE_MAXIMO: usize = 1000;

/// `ev ver` y `ev buscar`: imprime lo que devuelve el bus y, si llega al tope, avisa por stderr de que puede haber
/// más y del `--despues` con el que seguir. La salida estándar es la de siempre.
fn consultar_e_imprimir(f: &[(&str, String)]) -> Resultado<i32> {
    let eventos = Cliente::nuevo().consultar(f)?;
    imprimir(&eventos, false);
    let valor = |clave: &str| f.iter().find(|(k, _)| *k == clave).map(|(_, v)| v.as_str());
    if let Some(aviso) = aviso_de_tope(&eventos, valor("limite"), valor("despues").is_some()) {
        eprintln!("{aviso}");
    }
    Ok(0)
}

/// El aviso de una línea cuando la consulta devuelve tantos eventos como su tope efectivo; `None` si devuelve menos.
fn aviso_de_tope(eventos: &[Evento], limite: Option<&str>, con_despues: bool) -> Option<String> {
    let tope = limite
        .and_then(|l| l.parse::<i64>().ok())
        .map_or(TOPE_POR_DEFECTO, |l| usize::try_from(l.max(1)).unwrap_or(TOPE_MAXIMO))
        .min(TOPE_MAXIMO);
    if eventos.len() < tope {
        return None;
    }
    let (primero, ultimo) = (eventos.first()?.seq, eventos.last()?.seq);
    Some(if con_despues {
        format!(
            "ev: {} eventos, el tope de la consulta (seq {primero}-{ultimo}); puede haber más: sigue con --despues {ultimo}",
            eventos.len()
        )
    } else {
        format!(
            "ev: {} eventos, el tope de la consulta: son los últimos (seq {primero}-{ultimo}) y puede haber más anteriores; \
             para recorrerlos todos, empieza con --despues 0 y sigue con el seq que diga este aviso",
            eventos.len()
        )
    })
}

/// Una línea por evento: hora, tipo, fuente, profundidad y cuerpo recortado. En las cadenas, sangrado por profundidad.
fn imprimir(eventos: &[Evento], sangrar: bool) {
    for e in eventos {
        let hora = e.t_visto.get(11..23).unwrap_or(&e.t_visto);
        let sangria = if sangrar { "  ".repeat(e.profundidad as usize) } else { String::new() };
        let cuerpo = recortar(&e.cuerpo_texto().replace(['\n', '\r'], " "), 140);
        let id = &e.id[e.id.len().saturating_sub(6)..];
        println!("{hora} {sangria}{:<20} {} [p{} ·{id}] {cuerpo}", e.tipo, e.fuente, e.profundidad);
    }
}
