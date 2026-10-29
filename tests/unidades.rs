//! Piezas sin servicio: decodificación, redacción, comodines, validación de reglas y traducción de stream-json.

use eventos::arneses::traducir;
use eventos::reglas::{cargar, coincide, comodin, Regla};
use eventos::evento::Evento;
use eventos::texto::{decodificar, decodificar_con, redactar};
use serde_json::json;

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
