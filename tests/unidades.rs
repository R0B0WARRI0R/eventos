//! Piezas sin servicio: decodificación, redacción, comodines, validación de reglas y traducción de stream-json.

use eventos::arneses::traducir;
use eventos::reglas::{cargar, coincide, comodin, Regla};
use eventos::evento::Evento;
use eventos::almacen::Almacen;
use eventos::tareas::{derivar, desde, horas, Estado, TIPOS};
use std::time::{Duration, UNIX_EPOCH};
use eventos::texto::{decodificar, decodificar_con, redactar};
use serde_json::{json, Value};

/// Un evento ya guardado (con id, seq y hora de llegada) para las pruebas de tareas.
fn ev(seq: i64, id: &str, tipo: &str, correlacion: &str, cuerpo: Value, atributos: Value) -> Evento {
    Evento {
        id: id.into(),
        seq,
        fuente: "prueba".into(),
        tipo: tipo.into(),
        t_visto: format!("2026-10-29T18:00:{seq:02}.000Z"),
        cuerpo,
        atributos,
        correlacion: (!correlacion.is_empty()).then(|| correlacion.to_string()),
        ..Default::default()
    }
}

#[test]
fn una_tarea_de_la_fabrica_pasa_por_sus_estados_y_recoge_la_nota() {
    let c = "fabrica:p:t1";
    let mut v = vec![ev(1, "a", "fabrica.tarea.inicio", c, json!("Arreglar la suma"), json!({"agente": "codex", "proyecto": "p", "tarea": "t1"}))];
    let t = &derivar(&v)[0];
    assert_eq!((t.estado, t.origen.as_str()), (Estado::Trabajando, "fabrica"));
    assert!(t.titulo.contains("Arreglar la suma"), "{}", t.titulo);

    v.push(ev(2, "b", "fabrica.tarea.fin", c, json!("fallida; puertas: 0 de 1 bien"), json!({"estado": "fallida", "puertas_ok": false})));
    v.push(ev(3, "f", "fabrica.puerta.fallo", c, json!("assertion failed"), json!({"puerta": "pruebas"})));
    // La nota la publica el agente con su propia correlación; se engancha por su causa, el evento del fallo.
    let mut nota = ev(4, "n", "nota.reanudacion", "otra-correlacion", json!("Cambiar a - b por a + b"), Value::Null);
    nota.causa = Some("f".into());
    v.push(nota);
    let tareas = derivar(&v);
    assert_eq!(tareas.len(), 1, "{tareas:?}");
    assert_eq!((tareas[0].estado, tareas[0].evento.as_str()), (Estado::Fallido, "b"));
    assert_eq!(tareas[0].nota.as_deref(), Some("Cambiar a - b por a + b"));
    assert!(tareas[0].titulo.contains("Arreglar la suma"), "el final no borra el título: {}", tareas[0].titulo);
}

#[test]
fn hecha_con_puertas_bien_queda_lista_e_interrumpida_queda_parada() {
    let estado = |atributos: Value| derivar(&[ev(1, "a", "fabrica.tarea.fin", "fabrica:p:t", json!("resumen"), atributos)])[0].estado;
    assert_eq!(estado(json!({"estado": "hecha", "puertas_ok": true})), Estado::ListoParaRevisar);
    assert_eq!(estado(json!({"estado": "hecha", "puertas_ok": null})), Estado::ListoParaRevisar);
    assert_eq!(estado(json!({"estado": "hecha", "puertas_ok": false})), Estado::Fallido);
    // No hecha es fallida aunque las puertas pasen.
    assert_eq!(estado(json!({"estado": "rechazada", "puertas_ok": true})), Estado::Fallido);
    assert_eq!(estado(json!({"estado": "fallo", "puertas_ok": null})), Estado::Fallido);
    assert_eq!(estado(json!({"estado": "fallo", "motivo": "interrumpida: la app se cerró"})), Estado::Parado);
}

#[test]
fn una_tarea_de_quorum_sigue_sus_rondas_y_su_final() {
    let c = "quorum:p:suma-a1b2";
    let mut v = vec![ev(1, "a", "quorum.tarea.inicio", c, json!("Arregla la suma"), json!({"autor": "opus", "proyecto": "p", "tarea": "suma-a1b2"}))];
    let t = &derivar(&v)[0];
    assert_eq!((t.estado, t.origen.as_str()), (Estado::Trabajando, "quorum"));
    assert!(t.titulo.contains("Quórum · p: Arregla la suma"), "{}", t.titulo);
    v.push(ev(2, "b", "quorum.puerta", c, json!("1 prueba falla"), json!({"ronda": 1, "ok": false})));
    assert_eq!(derivar(&v)[0].detalle, "ronda 1: puerta en rojo");
    v.push(ev(3, "c", "quorum.puerta", c, json!("todo bien"), json!({"ronda": 2, "ok": true})));
    assert_eq!(derivar(&v)[0].detalle, "ronda 2: puerta en verde");
    // Sin `ok` no hay prueba de que pasara: cuenta como roja.
    let sin_ok = derivar(&[ev(1, "a", "quorum.puerta", c, json!("?"), json!({"ronda": 1}))]);
    assert_eq!(sin_ok[0].detalle, "ronda 1: puerta en rojo");
    // El panel solo ve los tipos que pide al almacén.
    for tipo in ["quorum.tarea.inicio", "quorum.puerta", "quorum.voto", "quorum.tarea.fin"] {
        assert!(TIPOS.contains(&tipo), "falta {tipo} en TIPOS");
    }
    v.push(ev(4, "d", "quorum.voto", c, json!("RECHAZA: falta la prueba"), json!({"ronda": 2, "revisor": "pruebas", "voto": "RECHAZA"})));
    let t = &derivar(&v)[0];
    assert_eq!((t.estado, t.detalle.as_str()), (Estado::Trabajando, "ronda 2: pruebas RECHAZA"));
    let fin = |estado: &str| {
        let mut v = v.clone();
        v.push(ev(5, "e", "quorum.tarea.fin", c, json!(format!("{estado}: detalle")), json!({"estado": estado, "rondas": 2})));
        let t = derivar(&v).remove(0);
        assert!(t.titulo.contains("Arregla la suma"), "el final no borra el título: {}", t.titulo);
        t.estado
    };
    assert_eq!(fin("integrada"), Estado::ListoParaRevisar);
    assert_eq!(fin("aprobada"), Estado::NecesitaEntrada);
    assert_eq!(fin("rechazada"), Estado::Fallido);
    assert_eq!(fin("error"), Estado::Fallido);
    assert_eq!(fin("sin_cambios"), Estado::Parado);
    // Sin el inicio (fuera de la ventana de horas), el título sale de la correlación.
    let solo_fin = derivar(&[ev(1, "a", "quorum.tarea.fin", c, json!("integrada: abc"), json!({"estado": "integrada"}))]);
    assert_eq!(solo_fin[0].titulo, "Quórum · p:suma-a1b2");
}

#[test]
fn una_sesion_de_claude_code_sigue_sus_hooks() {
    let s = "sesion-1";
    let mut v = vec![
        ev(1, "a", "claude.SessionStart", s, json!({"cwd": "D:\\dev\\eventos", "source": "startup"}), Value::Null),
        ev(2, "b", "claude.UserPromptSubmit", s, json!({"cwd": "D:\\dev\\eventos"}), Value::Null),
    ];
    let t = &derivar(&v)[0];
    assert_eq!((t.estado, t.origen.as_str()), (Estado::Trabajando, "claude"));
    assert!(t.titulo.contains("eventos"), "{}", t.titulo);
    v.push(ev(3, "c", "claude.Notification", s, json!({"message": "Claude needs your permission to use Bash"}), Value::Null));
    let t = &derivar(&v)[0];
    assert_eq!((t.estado, t.detalle.as_str()), (Estado::NecesitaEntrada, "Claude needs your permission to use Bash"));
    v.push(ev(4, "d", "claude.Stop", s, json!({}), Value::Null));
    assert_eq!(derivar(&v)[0].estado, Estado::ListoParaRevisar);
    v.push(ev(5, "e", "claude.SessionEnd", s, json!({"reason": "clear"}), Value::Null));
    assert_eq!(derivar(&v)[0].estado, Estado::Parado);
}

#[test]
fn los_procesos_y_los_agentes_tambien_son_tareas() {
    let v = vec![
        // Como lo publica `ev run`: la orden es la lista de argumentos.
        ev(1, "a", "proceso.inicio", "p1", Value::Null, json!({"orden": ["cargo", "test"]})),
        ev(2, "b", "proceso.fin", "p1", Value::Null, json!({"codigo": 101})),
        ev(3, "c", "proceso.inicio", "p2", Value::Null, json!({"orden": ["cargo", "build"]})),
        ev(4, "d", "proceso.fin", "p2", Value::Null, json!({"codigo": 0})),
        ev(5, "e", "agente.lanzado", "g1", Value::Null, json!({"regla": "diagnosticar"})),
        ev(6, "f", "agente.error", "g1", json!("tiempo agotado"), json!({"regla": "diagnosticar"})),
        ev(7, "g", "agente.lanzado", "g2", Value::Null, json!({"regla": "diagnosticar"})),
        ev(8, "h", "agente.fin", "g2", json!("hecho"), json!({"regla": "diagnosticar", "error": false})),
    ];
    let tareas = derivar(&v);
    let tarea = |k: &str| tareas.iter().find(|t| t.clave == k).unwrap_or_else(|| panic!("falta {k}: {tareas:?}"));
    assert_eq!(tarea("p1").estado, Estado::Fallido);
    assert!(tarea("p1").titulo.contains("cargo test"), "{}", tarea("p1").titulo);
    assert_eq!(tarea("p2").estado, Estado::ListoParaRevisar);
    assert_eq!(tarea("g1").estado, Estado::Fallido);
    assert!(tarea("g1").titulo.contains("diagnosticar"), "{}", tarea("g1").titulo);
    assert_eq!(tarea("g2").estado, Estado::ListoParaRevisar);
}

#[test]
fn cada_tipo_de_evento_da_su_estado() {
    let solo = |tipo: &str, cuerpo: Value, atributos: Value| derivar(&[ev(1, "a", tipo, "k", cuerpo, atributos)]).remove(0);
    assert_eq!(solo("claude.SessionStart", json!({"cwd": "D:\\p"}), Value::Null).estado, Estado::Parado);
    let error = solo("proceso.error", json!("no se encuentra el programa"), json!({"orden": ["inexistente", "--x"]}));
    assert_eq!(error.estado, Estado::Fallido);
    assert!(error.titulo.contains("inexistente --x"), "{}", error.titulo);
    assert_eq!(solo("agente.lanzado", Value::Null, json!({"regla": "r"})).estado, Estado::Trabajando);
    assert_eq!(solo("agente.fin", json!("x"), json!({"regla": "r", "error": true})).estado, Estado::Fallido);
    // Solo `error` verdadero es fallo: sin él o nulo, terminó bien.
    assert_eq!(solo("agente.fin", json!("x"), json!({"regla": "r"})).estado, Estado::ListoParaRevisar);
    assert_eq!(solo("agente.fin", json!("x"), json!({"regla": "r", "error": null})).estado, Estado::ListoParaRevisar);
    let agente = solo("agente.error", json!("no se pudo lanzar"), json!({"regla": "diagnosticar"}));
    assert_eq!(agente.estado, Estado::Fallido);
    assert!(agente.titulo.contains("diagnosticar"), "{}", agente.titulo);
}

#[test]
fn la_nota_va_a_su_tarea_y_se_borra_al_reintentar() {
    let mut v = vec![
        ev(1, "a1", "fabrica.tarea.fin", "fabrica:p:t1", json!("fallida"), json!({"estado": "fallida"})),
        ev(2, "f1", "fabrica.puerta.fallo", "fabrica:p:t1", json!("x"), Value::Null),
        ev(3, "a2", "fabrica.tarea.fin", "fabrica:p:t2", json!("fallida"), json!({"estado": "fallida"})),
        ev(4, "f2", "fabrica.puerta.fallo", "fabrica:p:t2", json!("y"), Value::Null),
    ];
    // Dos notas, cada una con su causa: si una nota se pegara a una tarea cualquiera, las dos acabarían en la misma.
    for (seq, id, texto, causa) in [(5, "n2", "nota de t2", "f2"), (6, "n1", "nota de t1", "f1")] {
        let mut nota = ev(seq, id, "nota.reanudacion", &format!("agente-{id}"), json!(texto), Value::Null);
        nota.causa = Some(causa.into());
        v.push(nota);
    }
    let tareas = derivar(&v);
    let tarea = |k: &str| tareas.iter().find(|t| t.clave == k).expect(k).clone();
    assert_eq!(tarea("fabrica:p:t2").nota.as_deref(), Some("nota de t2"));
    assert_eq!(tarea("fabrica:p:t1").nota.as_deref(), Some("nota de t1"), "cada nota va a la tarea de su causa");
    v.push(ev(7, "r", "fabrica.tarea.inicio", "fabrica:p:t2", json!("Otra vez"), json!({"proyecto": "p"})));
    let t2 = derivar(&v).into_iter().find(|t| t.clave == "fabrica:p:t2").expect("t2");
    assert_eq!((t2.estado, t2.nota), (Estado::Trabajando, None), "al reintentar, la nota del fallo anterior se va");
}

#[test]
fn una_nota_que_llega_despues_del_reintento_no_se_pega() {
    // El agente de diagnóstico puede tardar minutos: si la tarea ya se reintentó, su nota es de un fallo pasado.
    let c = "fabrica:p:t1";
    let mut v = vec![
        ev(1, "i1", "fabrica.tarea.inicio", c, json!("Sumar"), json!({"proyecto": "p"})),
        ev(2, "fin", "fabrica.tarea.fin", c, json!("fallo"), json!({"estado": "fallo", "puertas_ok": false})),
        ev(3, "f", "fabrica.puerta.fallo", c, json!("x"), Value::Null),
        ev(4, "i2", "fabrica.tarea.inicio", c, json!("Sumar"), json!({"proyecto": "p"})),
    ];
    let mut nota = ev(5, "n", "nota.reanudacion", "agente-1", json!("nota del fallo anterior"), Value::Null);
    nota.causa = Some("f".into());
    v.push(nota);
    let t = derivar(&v).remove(0);
    assert_eq!((t.estado, t.nota), (Estado::Trabajando, None));
}

#[test]
fn el_titulo_de_una_orden_no_ensena_secretos() {
    let t = derivar(&[ev(1, "a", "proceso.inicio", "p", Value::Null, json!({"orden": ["cargo", "run", "--password", "hunter2"]}))]).remove(0);
    assert!(!t.titulo.contains("hunter2"), "{}", t.titulo);
    assert!(t.titulo.contains("cargo run"), "{}", t.titulo);
}

#[test]
fn la_ventana_de_tareas_cuenta_horas_y_deja_fuera_lo_viejo() {
    // 2026-10-29T10:00:00Z.
    let ahora = UNIX_EPOCH + Duration::from_secs(1_793_268_000);
    assert_eq!(desde(2, ahora), "2026-10-29T08:00:00.000Z");
    assert_eq!(desde(24, ahora), "2026-10-28T10:00:00.000Z");

    let dir = std::env::temp_dir().join(format!("ev-unidades-ventana-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("carpeta");
    let almacen = Almacen::abrir(&dir.join("eventos.db")).expect("almacén");
    for (id, tipo, t_visto) in [
        ("hace-26-h", "proceso.fin", "2026-10-28T08:00:00.000Z"),
        ("hace-2-h", "proceso.fin", "2026-10-29T08:00:00.000Z"),
        ("linea", "linea.stdout", "2026-10-29T09:00:00.000Z"),
    ] {
        let mut e = ev(0, id, tipo, id, Value::Null, json!({"codigo": 0}));
        e.t_visto = t_visto.into();
        almacen.insertar(&mut e).expect("insertar");
    }
    let ids = |horas: u64| -> Vec<String> {
        almacen.de_tipos(TIPOS, &desde(horas, ahora), 5000).expect("consultar").into_iter().map(|e| e.id).collect()
    };
    assert_eq!(ids(24), ["hace-2-h"], "con 24 horas, lo de hace 26 se queda fuera; las líneas nunca entran");
    assert_eq!(ids(48), ["hace-26-h", "hace-2-h"]);
    drop(almacen);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_igual_hora_manda_el_orden_de_llegada() {
    // Los eventos de un mismo envío comparten milisegundo: el desempate es el orden de llegada, no el azar.
    let mut a = ev(1, "a", "proceso.fin", "primero", Value::Null, json!({"codigo": 0}));
    let mut b = ev(2, "b", "proceso.fin", "segundo", Value::Null, json!({"codigo": 0}));
    a.t_visto = "2026-10-29T18:00:00.000Z".into();
    b.t_visto = a.t_visto.clone();
    for _ in 0..20 {
        let claves: Vec<String> = derivar(&[a.clone(), b.clone()]).into_iter().map(|t| t.clave).collect();
        assert_eq!(claves, ["segundo", "primero"]);
    }
}

#[test]
fn las_horas_de_tareas_tienen_valor_por_defecto_y_tope() {
    assert_eq!(horas(None), 24);
    assert_eq!(horas(Some("2")), 2);
    assert_eq!(horas(Some("1000")), 168);
    assert_eq!(horas(Some("no es un número")), 24);
}

#[test]
fn lo_que_pide_atencion_va_primero_y_lo_que_no_es_tarea_no_sale() {
    let v = vec![
        ev(1, "a", "proceso.fin", "listo", Value::Null, json!({"codigo": 0})),
        ev(2, "b", "claude.Notification", "espera", json!({"message": "te necesita"}), Value::Null),
        ev(3, "c", "proceso.inicio", "trabaja", Value::Null, json!({"orden": "x"})),
        ev(4, "d", "proceso.fin", "falla", Value::Null, json!({"codigo": 1})),
        ev(5, "e", "claude.SessionEnd", "parada", json!({}), Value::Null),
        ev(6, "f", "linea.stdout", "ruido", json!("hola"), Value::Null),
        ev(7, "g", "proceso.fin", "", Value::Null, json!({"codigo": 0})),
        ev(8, "h", "fabrica.puerta.fallo", "solo-fallo", json!("x"), Value::Null),
        ev(9, "i", "proceso.fin", "listo-despues", Value::Null, json!({"codigo": 0})),
    ];
    let claves: Vec<String> = derivar(&v).into_iter().map(|t| t.clave).collect();
    // Necesita entrada, fallido, trabajando, listo (el más reciente antes) y parado; sin las que no tienen estado.
    assert_eq!(claves, ["espera", "falla", "trabaja", "listo-despues", "listo", "parada"]);
}

#[test]
fn utf8_pasa_tal_cual_y_cp850_se_decodifica() {
    assert_eq!(decodificar("Estadísticas".as_bytes()), "Estadísticas");
    // 0xA1 es «í» en cp850 (lo que escribe ping en un Windows en castellano).
    assert_eq!(decodificar_con(b"Estad\xa1sticas", 850), "Estadísticas");
    assert_eq!(decodificar(b""), "");
}

#[test]
fn la_redaccion_tapa_secretos_y_deja_el_resto() {
    assert_eq!(redactar("Authorization: Bearer abcdefghijkl123"), "Authorization: [REDACTADO]");
    assert_eq!(redactar("password=hunter2 y sigo"), "password=[REDACTADO] y sigo");
    assert_eq!(redactar("api_key: \"con espacios dentro\""), "api_key: [REDACTADO]");
    assert_eq!(redactar("curl -H 'x' bearer abcdefghijklmnop"), "curl -H 'x' Bearer [REDACTADO]");
    assert_eq!(redactar("clave sk-abcdefghijklmnopqrstu fin"), "clave [REDACTADO] fin");
    assert_eq!(redactar("la clave del éxito es la constancia"), "la clave del éxito es la constancia");
}

#[test]
fn la_redaccion_cubre_prefijos_json_esquemas_y_tokens_sueltos() {
    assert_eq!(redactar("GITHUB_TOKEN=ghp_abcdefghijklmnopqrstuvwxyz0123"), "GITHUB_TOKEN=[REDACTADO]");
    assert_eq!(redactar("AWS_SECRET_ACCESS_KEY=wJalrXUtnFEMIK7MDENG"), "AWS_SECRET_ACCESS_KEY=[REDACTADO]");
    assert_eq!(redactar("set DB_PASSWORD=supersecreta"), "set DB_PASSWORD=[REDACTADO]");
    assert_eq!(
        redactar(r#"{"password":"hunter2json","api_key": "sk_live_jsonkey"}"#),
        r#"{"password":[REDACTADO],"api_key": [REDACTADO]}"#
    );
    assert_eq!(redactar("Authorization: Basic dXNlcjpwYXNzd29yZA=="), "Authorization: [REDACTADO]");
    assert_eq!(redactar("-H 'basic dXNlcjpwYXNzd29yZA=='"), "-H 'Basic [REDACTADO]'");
    assert_eq!(redactar("un token suelto ghp_abcdefghijklmnopqrstuvwxyz0123 aquí"), "un token suelto [REDACTADO] aquí");
}

#[test]
fn la_redaccion_cubre_json_escapado_opciones_con_espacio_y_urls() {
    // Así llega por un hook: el JSON va dentro de una orden, con las comillas escapadas.
    assert_eq!(
        redactar(r#"curl -d "{\"user\":\"a\",\"password\":\"hunter2\"}" https://api.example/login"#),
        r#"curl -d "{\"user\":\"a\",\"password\":[REDACTADO]}" https://api.example/login"#
    );
    assert_eq!(redactar(r#"{"password":"ab\"cdSECRETO"}"#), r#"{"password":[REDACTADO]}"#);
    // Con espacios y comas dentro del valor escapado, el corte genérico dejaría la mitad en claro.
    assert_eq!(redactar(r#"{\"password\":\"mi clave, muy larga\"}"#), r#"{\"password\":[REDACTADO]}"#);
    assert_eq!(redactar("mysql --password hunter2 --verbose"), "mysql --password [REDACTADO] --verbose");
    assert_eq!(redactar("cli --token --verbose"), "cli --token --verbose", "no se come la opción siguiente");
    assert_eq!(redactar("postgres://ana:hunter2@db.local:5432/x"), "postgres://ana:[REDACTADO]@db.local:5432/x");
    assert_eq!(redactar("visita https://ejemplo.com:8080/ruta"), "visita https://ejemplo.com:8080/ruta");
}

#[test]
fn comodines() {
    assert!(comodin("proc:cargo test*", "proc:cargo test --lib"));
    assert!(comodin("proc:cargo test*", "proc:cargo test"));
    assert!(!comodin("proc:cargo test*", "proc:cargo build"));
    assert!(comodin("*", ""));
    assert!(comodin("a*b*c", "axxbyyc"));
    assert!(!comodin("a*b", "a"));
    assert!(!comodin("abc", "abcd"));
}

fn regla(texto: &str) -> Regla {
    let f: eventos::reglas::Fichero = toml::from_str(texto).expect("TOML de prueba");
    f.reglas.into_iter().next().expect("una regla")
}

#[test]
fn una_regla_casa_por_tipo_fuente_e_atributos() {
    let r = regla(
        r#"[[regla]]
nombre = "rotas"
tipo = "proceso.fin"
fuente = "proc:cargo test*"
arnes = "eco"
publica = "x"
[regla.atributos_distinto]
codigo = 0"#,
    );
    let mut e = Evento::nuevo("proc:cargo test -q", "proceso.fin", serde_json::Value::Null);
    e.atributos = json!({ "codigo": 101 });
    assert!(coincide(&r, &e));
    e.atributos = json!({ "codigo": 0 });
    assert!(!coincide(&r, &e), "con código 0 no casa");
    e.atributos = json!({});
    assert!(!coincide(&r, &e), "sin el atributo no casa");
    e.atributos = json!({ "codigo": 101 });
    e.fuente = "proc:cargo build".into();
    assert!(!coincide(&r, &e), "otra fuente no casa");
}

fn fichero_temporal(nombre: &str, texto: &str) -> std::path::PathBuf {
    let ruta = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(nombre);
    std::fs::write(&ruta, texto).expect("escribir fichero de prueba");
    ruta
}

#[test]
fn la_v0_rechaza_agentes_que_no_sean_de_lectura_y_campos_desconocidos() {
    let escritura = fichero_temporal(
        "reglas-escritura.toml",
        "[[regla]]\nnombre = \"a\"\ntipo = \"t\"\narnes = \"claude\"\nencargo = \"e\"\npermisos = \"escritura\"\n",
    );
    let error = cargar(&escritura).expect_err("debería rechazarse").to_string();
    assert!(error.contains("solo lanza agentes de lectura"), "{error}");

    let errata = fichero_temporal("reglas-errata.toml", "[[regla]]\nnombre = \"a\"\ntipo = \"t\"\narnes = \"eco\"\npublica = \"p\"\nprofundiad = 3\n");
    let error = cargar(&errata).expect_err("una errata no debe pasar en silencio").to_string();
    assert!(error.contains("profundiad"), "{error}");

    let repetida = fichero_temporal(
        "reglas-repetida.toml",
        "[[regla]]\nnombre = \"a\"\ntipo = \"t\"\narnes = \"eco\"\npublica = \"p\"\n[[regla]]\nnombre = \"a\"\ntipo = \"u\"\narnes = \"eco\"\npublica = \"p\"\n",
    );
    assert!(cargar(&repetida).is_err());
}

#[test]
fn traduce_la_salida_stream_json_de_claude() {
    let tipos = |linea: &str| traducir(linea).into_iter().map(|(t, _, _)| t).collect::<Vec<_>>();
    assert_eq!(tipos(r#"{"type":"system","subtype":"init","session_id":"s","model":"m"}"#), ["agente.inicio"]);
    let asistente = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hola"},{"type":"tool_use","name":"mcp__eventos__buscar","input":{"tipo":"x"}}]}}"#;
    let traducidos = traducir(asistente);
    assert_eq!(traducidos.len(), 2);
    assert_eq!((traducidos[0].0.as_str(), &traducidos[0].1), ("agente.mensaje", &json!("hola")));
    assert_eq!(traducidos[1].0, "agente.herramienta");
    assert_eq!(traducidos[1].2["herramienta"], "mcp__eventos__buscar");
    let fin = traducir(r#"{"type":"result","result":"listo","is_error":false,"num_turns":2,"duration_ms":10,"total_cost_usd":0.01,"session_id":"s"}"#);
    assert_eq!(fin[0].0, "agente.fin");
    assert_eq!(fin[0].1, json!("listo"));
    assert_eq!(fin[0].2["turnos"], 2);
    assert_eq!(tipos("esto no es json"), ["agente.salida"]);
    assert!(tipos(r#"{"type":"user","message":{}}"#).is_empty());
    assert!(tipos("   ").is_empty());
}
