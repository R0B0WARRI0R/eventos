//! Seguir ficheros de registro (enganches, grupo 2a). Primero las piezas sueltas, con ficheros temporales y sin bus:
//! el seguidor de un fichero, el tope, la configuración y los dos caminos del vigilante (avisos de `notify` y sondeo)
//! por separado. Después, `ev tail` y `ev enganchar` de verdad contra un bus de verdad en un puerto libre.
//!
//! Las esperas tienen plazo: se mira hasta que llega lo que se espera o se acaba el plazo, nunca un sueño fijo.

use eventos::enganches::{validar, Config, Fichero, Limitador, Vigia};
use eventos::evento::Evento;
use eventos::tail::{Descarte, Lectura, Motivo, Seguidor, Suceso, TopeLineas, Vigilante};
use eventos::texto::decodificar;
use serde_json::{json, Value};
use std::fs::{self, OpenOptions};
use std::io::{BufWriter, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const EV: &str = env!("CARGO_BIN_EXE_ev");

fn carpeta_temporal(nombre: &str) -> PathBuf {
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("reloj").as_nanos();
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("ev-{nombre}-{}-{n}", std::process::id()));
    fs::create_dir_all(&dir).expect("carpeta de prueba");
    dir
}

/// Añade bytes al final y cierra, como un programa que abre, escribe y cierra cada vez.
fn anadir(ruta: &Path, bytes: &[u8]) {
    let mut f = OpenOptions::new().create(true).append(true).open(ruta).expect("abrir para añadir");
    f.write_all(bytes).expect("añadir");
}

fn lineas(l: &Lectura) -> Vec<(u64, String)> {
    l.lineas().iter().map(|x| (x.numero, x.texto.clone())).collect()
}

fn par(n: u64, t: &str) -> (u64, String) {
    (n, t.to_string())
}

// --- El seguidor de un fichero ---

#[test]
fn por_defecto_empieza_al_final_y_solo_da_lo_nuevo() {
    let ruta = carpeta_temporal("final").join("app.log");
    fs::write(&ruta, "vieja 1\nvieja 2\n").expect("fichero");
    let mut s = Seguidor::new(&ruta, false);
    assert!(s.existia());
    assert_eq!(s.saltadas(), 2);
    assert!(s.pasada().sucesos.is_empty(), "lo que ya había no sale");
    anadir(&ruta, b"nueva 3\nnueva 4\n");
    assert_eq!(lineas(&s.pasada()), [par(3, "nueva 3"), par(4, "nueva 4")], "los números siguen los del fichero");
    assert!(s.pasada().sucesos.is_empty(), "sin nada nuevo, nada");
}

#[test]
fn desde_inicio_lee_lo_que_ya_habia() {
    let ruta = carpeta_temporal("desde-inicio").join("app.log");
    fs::write(&ruta, "uno\ndos\n").expect("fichero");
    let mut s = Seguidor::new(&ruta, true);
    assert_eq!(s.saltadas(), 0);
    assert_eq!(lineas(&s.pasada()), [par(1, "uno"), par(2, "dos")]);
}

#[test]
fn una_linea_a_medias_espera_a_su_salto() {
    let ruta = carpeta_temporal("medias").join("app.log");
    // Al empezar ya hay una línea a medias: no es vieja del todo; sale entera cuando llega su salto.
    fs::write(&ruta, "vieja\nempezada").expect("fichero");
    let mut s = Seguidor::new(&ruta, false);
    assert_eq!(s.saltadas(), 1);
    assert!(s.pasada().sucesos.is_empty(), "sin salto no sale");
    anadir(&ruta, b" y acabada\r\nsiguiente");
    assert_eq!(lineas(&s.pasada()), [par(2, "empezada y acabada")], "entera y sin el \\r");
    assert!(s.pasada().sucesos.is_empty(), "«siguiente» sigue esperando");
    anadir(&ruta, b"\n");
    assert_eq!(lineas(&s.pasada()), [par(3, "siguiente")]);
}

#[test]
fn al_truncar_o_rotar_vuelve_a_empezar_y_no_pierde_lo_ultimo() {
    let dir = carpeta_temporal("rotar");
    let ruta = dir.join("app.log");
    fs::write(&ruta, "").expect("fichero vacío");
    let mut s = Seguidor::new(&ruta, false);
    anadir(&ruta, b"a1\na2\nmedia");
    assert_eq!(lineas(&s.pasada()), [par(1, "a1"), par(2, "a2")]);

    // Truncado: más corto que lo ya leído. Lo que esperaba su salto era lo último del fichero de antes.
    fs::write(&ruta, "b1\n").expect("truncar");
    let l = s.pasada();
    assert_eq!(l.sucesos.len(), 3, "{:?}", l.sucesos);
    assert!(matches!(&l.sucesos[0], Suceso::Linea(x) if x.numero == 3 && x.texto == "media" && x.incompleta));
    assert_eq!(l.sucesos[1], Suceso::Rotado(Motivo::Encogido));
    assert!(matches!(&l.sucesos[2], Suceso::Linea(x) if x.numero == 1 && x.texto == "b1" && !x.incompleta));

    // Reescrito más largo y con otro principio.
    fs::write(&ruta, "c1 distinto\nc2\n").expect("reescribir");
    let l = s.pasada();
    assert_eq!(l.sucesos.first(), Some(&Suceso::Rotado(Motivo::Cambiado)));
    assert_eq!(lineas(&l), [par(1, "c1 distinto"), par(2, "c2")]);

    // Con la misma cabecera de siempre (muchas apps empiezan su registro igual): la cambia lo de antes de donde íbamos.
    let cabecera = format!("=== Constructo: registro de la sesión {} ===\n", "x".repeat(40));
    fs::write(&ruta, format!("{cabecera}vieja de la sesión 1\n")).expect("sesión 1");
    let l = s.pasada();
    assert_eq!(l.sucesos.first(), Some(&Suceso::Rotado(Motivo::Cambiado)));
    fs::write(&ruta, format!("{cabecera}nueva de la sesión 2, más larga que la otra\n")).expect("sesión 2");
    let l = s.pasada();
    assert_eq!(l.sucesos.first(), Some(&Suceso::Rotado(Motivo::Cambiado)), "{:?}", l.sucesos);
    assert_eq!(lineas(&l).last(), Some(&par(2, "nueva de la sesión 2, más larga que la otra")));

    // Desaparece (la app lo rota con otro nombre) y vuelve: se lee entero desde 0.
    fs::rename(&ruta, dir.join("app.log.1")).expect("rotar con otro nombre");
    assert_eq!(s.pasada().sucesos, [Suceso::Rotado(Motivo::Desaparecido)]);
    assert!(s.pasada().sucesos.is_empty(), "mientras no esté, nada");
    fs::write(&ruta, "d1\n").expect("fichero nuevo");
    assert_eq!(lineas(&s.pasada()), [par(1, "d1")]);
}

#[test]
fn los_bytes_cp850_se_decodifican_como_en_ev_run() {
    let ruta = carpeta_temporal("cp850").join("app.log");
    // Marca de orden de bytes al principio, finales CRLF, líneas vacías (cuentan pero no salen) y una en cp850.
    fs::write(&ruta, b"\xEF\xBB\xBFprimera\r\n\r\n   \nEstad\xa1sticas\r\na\xc3\xb1o en UTF-8\n").expect("fichero");
    let mut s = Seguidor::new(&ruta, true);
    let cp850 = decodificar(b"Estad\xa1sticas");
    assert!(!cp850.contains('\u{FFFD}'), "la página OEM no es UTF-8 con sustituciones: {cp850}");
    assert_eq!(lineas(&s.pasada()), [par(1, "primera"), par(4, &cp850), par(5, "año en UTF-8")]);
}

#[test]
fn una_linea_enorme_se_recorta_sin_perder_la_cuenta() {
    let ruta = carpeta_temporal("enorme").join("app.log");
    fs::write(&ruta, "").expect("fichero");
    let mut s = Seguidor::new(&ruta, false);
    anadir(&ruta, &vec![b'x'; 50_000]);
    assert!(s.pasada().sucesos.is_empty());
    anadir(&ruta, &[vec![b'y'; 50_000], b"\ndespues\n".to_vec()].concat());
    let l = lineas(&s.pasada());
    assert_eq!(l.len(), 2);
    assert!(l[0].1.len() < 70_000 && l[0].1.ends_with("[recortado: 100000 bytes en total]"), "{}", l[0].1.len());
    assert_eq!(l[1], par(2, "despues"));
}

#[test]
fn un_fichero_que_aun_no_existe_se_espera_y_se_lee_entero_al_aparecer() {
    let ruta = carpeta_temporal("aparece").join("aun-no").join("app.log");
    let mut s = Seguidor::new(&ruta, false);
    assert!(!s.existia());
    assert!(s.pasada().sucesos.is_empty(), "ni su carpeta existe: ni error ni nada");
    fs::create_dir_all(ruta.parent().expect("carpeta")).expect("carpeta");
    fs::write(&ruta, "primera\nsegunda\n").expect("fichero");
    assert_eq!(lineas(&s.pasada()), [par(1, "primera"), par(2, "segunda")], "lo que trae al aparecer es nuevo");
}

#[test]
fn un_escritor_que_mantiene_el_fichero_abierto_se_lee_al_vaciar_su_bufer_y_puede_rotarlo() {
    let dir = carpeta_temporal("abierto");
    let ruta = dir.join("app.log");
    let f = OpenOptions::new().create(true).append(true).open(&ruta).expect("abrir");
    let mut s = Seguidor::new(&ruta, false);
    let mut w = BufWriter::new(f);
    w.write_all(b"en su bufer\n").expect("escribir");
    assert!(s.pasada().sucesos.is_empty(), "aún está en el búfer de la app");
    w.flush().expect("vaciar");
    assert_eq!(lineas(&s.pasada()), [par(1, "en su bufer")], "vaciado el búfer, se lee aunque siga abierto");
    w.write_all(b"otra\n").expect("escribir");
    w.flush().expect("vaciar");
    assert_eq!(lineas(&s.pasada()), [par(2, "otra")]);
    drop(w);
    // Entre pasadas no se tiene abierto: la app puede rotarlo sin que se lo impidamos.
    fs::rename(&ruta, dir.join("app.log.1")).expect("la app puede rotar su registro");
    assert_eq!(s.pasada().sucesos, [Suceso::Rotado(Motivo::Desaparecido)]);
}

#[cfg(windows)]
#[test]
fn un_fichero_que_no_se_deja_leer_avisa_una_vez_y_se_lee_cuando_se_deja() {
    use std::os::windows::fs::OpenOptionsExt;
    let ruta = carpeta_temporal("exclusivo").join("app.log");
    fs::write(&ruta, "").expect("fichero");
    let mut s = Seguidor::new(&ruta, false);
    let mut exclusivo = OpenOptions::new().append(true).share_mode(0).open(&ruta).expect("abrir en exclusiva");
    exclusivo.write_all(b"escrita en exclusiva\n").expect("escribir");
    let l = s.pasada();
    assert!(matches!(l.sucesos.as_slice(), [Suceso::Error(_)]), "{:?}", l.sucesos);
    assert!(s.pasada().sucesos.is_empty(), "el mismo error no se repite");
    drop(exclusivo);
    assert_eq!(lineas(&s.pasada()), [par(1, "escrita en exclusiva")]);
}

// --- El tope ---

#[test]
fn el_tope_descarta_lo_que_sobra_y_resume_la_racha_una_vez() {
    let mut t = TopeLineas::new(3);
    let t0 = Instant::now();
    let r: Vec<(bool, Option<Descarte>)> = (1..=10).map(|n| t.admitir(n, t0)).collect();
    assert_eq!(r.iter().filter(|(cabe, _)| *cabe).count(), 3);
    assert!(r.iter().all(|(_, d)| d.is_none()));
    assert_eq!(t.resumen(t0 + Duration::from_millis(500)), None, "la racha aún no ha acabado");
    assert_eq!(t.resumen(t0 + Duration::from_secs(1)), Some(Descarte { descartadas: 7, desde_linea: 4, hasta_linea: 10 }));
    assert_eq!(t.resumen(t0 + Duration::from_secs(2)), None, "una vez");

    // Una racha que acaba porque vuelve a caber una línea: el resumen va antes que ella.
    let t1 = t0 + Duration::from_secs(3);
    for n in 11..=14 {
        t.admitir(n, t1);
    }
    let (cabe, resumen) = t.admitir(15, t1 + Duration::from_secs(1));
    assert!(cabe);
    assert_eq!(resumen, Some(Descarte { descartadas: 1, desde_linea: 14, hasta_linea: 14 }));
}

// --- La configuración ---

fn mala(toml_texto: &str) -> String {
    match toml::from_str::<Config>(toml_texto) {
        Err(_) => String::from("no es TOML válido para la configuración"),
        Ok(c) => validar(&c).expect_err("tenía que rechazarse").to_string(),
    }
}

#[test]
fn los_ficheros_de_la_configuracion_se_validan_con_errores_claros() {
    let c: Config = toml::from_str("[[fichero]]\nruta = 'D:\\x\\app.log'\n").expect("toml");
    validar(&c).expect("válida");
    assert_eq!(c.ficheros, [Fichero { ruta: r"D:\x\app.log".into(), app: None, desde_inicio: false, por_segundo: 20 }]);
    let c: Config =
        toml::from_str("[[fichero]]\nruta = 'D:\\x\\a.log'\napp = \"constructo\"\ndesde_inicio = true\npor_segundo = 5\n").expect("toml");
    validar(&c).expect("válida");
    assert_eq!(c.ficheros[0].app.as_deref(), Some("constructo"));
    assert!(c.ficheros[0].desde_inicio);

    let fichero = |extra: &str| format!("[[fichero]]\nruta = 'D:\\x\\app.log'\n{extra}");
    assert!(mala("[[fichero]]\nruta = ''").contains("falta la ruta"));
    assert!(mala("[[fichero]]\nruta = '   '").contains("falta la ruta"));
    assert!(mala("[[fichero]]\napp = \"a\"").contains("no es TOML válido"), "la ruta es obligatoria");
    assert!(mala("[[fichero]]\nruta = 'app.log'").contains("absoluta"));
    assert!(mala("[[fichero]]\nruta = 'D:\\x\\*.log'").contains("comodines"));
    assert!(mala("[[fichero]]\nruta = 'D:\\x\\App.LOG'\n[[fichero]]\nruta = 'd:/x/app.log'").contains("repetido"));
    assert!(mala(&fichero("por_segundo = 0")).contains("por_segundo"));
    assert!(mala(&fichero("por_segundo = 5000")).contains("por_segundo"));
    assert!(mala(&fichero("app = \"Mi App\"")).contains("slug"));
    assert!(mala(&fichero("campo_inventado = 1")).contains("no es TOML válido"), "deny_unknown_fields");
    let carpeta = carpeta_temporal("es-carpeta");
    assert!(mala(&format!("[[fichero]]\nruta = '{}'", carpeta.display())).contains("carpeta"));
}

#[test]
fn en_enganchar_las_lineas_iguales_no_se_callan_y_van_con_el_arranque_de_su_app() {
    let c: Config = toml::from_str(
        "[[app]]\nslug = \"lector\"\nnombre = \"Lector\"\ncoincide_exe = \"Lector.exe\"\n[[fichero]]\nruta = 'D:\\x\\lector.log'\napp = \"lector\"",
    )
    .expect("toml");
    validar(&c).expect("válida");
    let de = |tipo: &str| Evento {
        gravedad: Some("info".into()),
        ..Evento::nuevo(r"fichero:D:\x\lector.log", tipo, Value::String("igual".into()))
    };
    let mut l = Limitador::new(&c);
    let t0 = Instant::now();
    assert_eq!(l.admitir(de("registro.linea"), t0).len(), 1);
    assert_eq!(l.admitir(de("registro.linea"), t0).len(), 1, "dos líneas iguales son dos cosas que pasaron");
    assert_eq!(l.admitir(de("registro.tope"), t0).len(), 1);
    assert!(l.admitir(de("registro.tope"), t0).is_empty(), "lo demás sí se trata como repetición");

    let mut v = Vigia::new(&c);
    assert_eq!(v.correlacion_de("lector"), None, "cerrada");
    let inicio = v.ventana(10, "Lector.exe", false);
    assert_eq!(v.correlacion_de("lector"), inicio[0].correlacion);
    assert_eq!(v.correlacion_de("otra"), None);
    v.termina(10, 0);
    assert_eq!(v.correlacion_de("lector"), None, "ya cerrada");
}

// --- El vigilante: cada camino por separado ---

fn fichero(ruta: &Path) -> Fichero {
    Fichero { ruta: ruta.display().to_string(), app: None, desde_inicio: false, por_segundo: 100 }
}

/// Da vueltas hasta que salgan `n` líneas o se acabe el plazo.
fn vueltas_hasta(v: &mut Vigilante, n: usize, plazo: Duration) -> Vec<Evento> {
    let fin = Instant::now() + plazo;
    let mut salida = Vec::new();
    while salida.iter().filter(|e: &&Evento| e.tipo == "registro.linea").count() < n && Instant::now() < fin {
        salida.extend(v.vuelta(Duration::from_millis(100)));
    }
    salida
}

fn cuerpos_de(eventos: &[Evento]) -> Vec<String> {
    eventos.iter().filter(|e| e.tipo == "registro.linea").map(Evento::cuerpo_texto).collect()
}

#[test]
fn con_solo_los_avisos_de_notify_se_lee_sin_esperar_al_sondeo() {
    let ruta = carpeta_temporal("solo-notify").join("app.log");
    fs::write(&ruta, "").expect("fichero");
    // Sondeo de una hora: si sale algo, lo ha traído un aviso de notify.
    let mut v = Vigilante::con(&[fichero(&ruta)], Duration::from_secs(3600), true);
    anadir(&ruta, b"por aviso\n");
    let t = Instant::now();
    let eventos = vueltas_hasta(&mut v, 1, Duration::from_secs(10));
    assert_eq!(cuerpos_de(&eventos), ["por aviso"], "notify no avisó en 10 s");
    eprintln!("notify avisó en {:?}", t.elapsed());
}

#[test]
fn sin_notify_el_sondeo_ve_a_un_escritor_que_mantiene_el_fichero_abierto() {
    let ruta = carpeta_temporal("solo-sondeo").join("app.log");
    let f = OpenOptions::new().create(true).append(true).open(&ruta).expect("abrir");
    let mut v = Vigilante::con(&[fichero(&ruta)], Duration::from_millis(300), false);
    let mut w = BufWriter::new(f);
    w.write_all(b"por sondeo\n").expect("escribir");
    w.flush().expect("vaciar");
    let eventos = vueltas_hasta(&mut v, 1, Duration::from_secs(10));
    assert_eq!(cuerpos_de(&eventos), ["por sondeo"]);
    let e = eventos.iter().find(|e| e.tipo == "registro.linea").expect("línea");
    assert_eq!(e.fuente, format!("fichero:{}", ruta.display()));
    assert_eq!(e.confianza.as_deref(), Some("externa"));
}

// --- ev tail y ev enganchar contra un bus de verdad ---

struct Bus {
    hijos: Vec<Child>,
    puerto: u16,
    dir: PathBuf,
}

impl Drop for Bus {
    fn drop(&mut self) {
        // Primero los que publican; el bus, el último.
        for hijo in self.hijos.iter_mut().rev() {
            let _ = hijo.kill();
            let _ = hijo.wait();
        }
        let _ = fs::remove_dir_all(&self.dir);
    }
}

impl Bus {
    fn arrancar(nombre: &str) -> Bus {
        let dir = carpeta_temporal(nombre);
        let puerto = TcpListener::bind("127.0.0.1:0").expect("puerto").local_addr().expect("dirección").port();
        let bus = Command::new(EV)
            .args(["servir", "--puerto", &puerto.to_string()])
            .env("EV_DIR", dir.join("datos"))
            .stdout(Stdio::null())
            .stderr(fs::File::create(dir.join("servir.err")).expect("fichero de stderr"))
            .spawn()
            .expect("ev servir");
        let b = Bus { hijos: vec![bus], puerto, dir };
        let fin = Instant::now() + Duration::from_secs(15);
        while ureq::get(&b.url("/estado")).call().is_err() {
            assert!(Instant::now() < fin, "el bus no arrancó en 15 s\n{}", b.diagnostico());
            thread::sleep(Duration::from_millis(100));
        }
        b
    }

    fn url(&self, ruta: &str) -> String {
        format!("http://127.0.0.1:{}{ruta}", self.puerto)
    }

    /// Una orden de `ev` que publica en este bus. Lo que diga por stderr queda en `<orden>.err`, para el diagnóstico.
    fn lanzar(&mut self, args: &[&str]) {
        let err = fs::File::create(self.dir.join(format!("{}.err", args[0]))).expect("fichero de stderr");
        let hijo = Command::new(EV)
            .args(args)
            .env("EV_PUERTO", self.puerto.to_string())
            .env("EV_DIR", self.dir.join("datos"))
            .stdout(Stdio::null())
            .stderr(err)
            .spawn()
            .expect("lanzar ev");
        self.hijos.push(hijo);
    }

    /// Lo que han dicho por stderr las órdenes lanzadas y cuántos eventos esperan en la cola en disco.
    fn diagnostico(&self) -> String {
        let cola = fs::read_to_string(self.dir.join("datos").join("pendientes.jsonl")).map_or(0, |t| t.lines().count());
        let mut d = format!("eventos en la cola en disco: {cola}");
        for f in fs::read_dir(&self.dir).into_iter().flatten().flatten() {
            if f.path().extension().is_some_and(|x| x == "err") {
                d += &format!("\n{}: {}", f.file_name().to_string_lossy(), fs::read_to_string(f.path()).unwrap_or_default().trim());
            }
        }
        d
    }

    fn ver(&self, filtro: &[(&str, &str)]) -> Vec<Value> {
        let mut p = ureq::get(&self.url("/eventos"));
        for (k, v) in filtro {
            p = p.query(k, v);
        }
        p.call().expect("consultar").into_json().expect("lista JSON")
    }

    fn esperar(&self, filtro: &[(&str, &str)], n: usize, segundos: u64) -> Vec<Value> {
        let fin = Instant::now() + Duration::from_secs(segundos);
        loop {
            let v = self.ver(filtro);
            if v.len() >= n {
                return v;
            }
            assert!(Instant::now() < fin, "no llegaron {n} eventos con {filtro:?} en {segundos} s; hay: {v:?}\n{}", self.diagnostico());
            thread::sleep(Duration::from_millis(100));
        }
    }
}

fn cuerpos(eventos: &[Value]) -> Vec<String> {
    eventos.iter().map(|e| e["cuerpo"].as_str().unwrap_or("").to_string()).collect()
}

fn texto(p: &Path) -> String {
    p.display().to_string()
}

#[test]
fn ev_tail_publica_solo_lo_nuevo_en_lineas_completas_y_espera_al_que_no_existe() {
    let mut b = Bus::arrancar("tail");
    let (ruta, nuevo) = (b.dir.join("app.log"), b.dir.join("nuevo.log"));
    fs::write(&ruta, "vieja 1\nvieja 2\n").expect("fichero");
    b.lanzar(&["tail", &texto(&ruta), &texto(&nuevo), "--app", "prueba"]);
    let fuente = format!("fichero:{}", texto(&ruta));
    let fuente_nuevo = format!("fichero:{}", texto(&nuevo));

    let inicio = b.esperar(&[("tipo", "registro.inicio"), ("fuente", &fuente)], 1, 15);
    assert_eq!(inicio[0]["atributos"]["existe"], true);
    assert_eq!(inicio[0]["atributos"]["saltadas"], 2);
    let espera = b.esperar(&[("tipo", "registro.inicio"), ("fuente", &fuente_nuevo)], 1, 5);
    assert_eq!(espera[0]["atributos"]["existe"], false);

    // Una línea entera y otra a medias en la misma escritura: cuando llega la primera, la segunda ya se ha leído.
    anadir(&ruta, b"nueva 3\npar");
    b.esperar(&[("tipo", "registro.linea")], 1, 10);
    anadir(&ruta, b"cial\r\nEstad\xa1sticas\n");
    let lineas = b.esperar(&[("tipo", "registro.linea"), ("fuente", &fuente)], 3, 10);
    assert_eq!(cuerpos(&lineas), ["nueva 3".to_string(), "parcial".into(), decodificar(b"Estad\xa1sticas")]);
    let e = &lineas[0];
    assert_eq!(e["fuente"], fuente.as_str());
    assert_eq!(e["confianza"], "externa", "una línea de registro puede traer texto de terceros");
    assert_eq!(e["gravedad"], "info");
    assert_eq!(e["atributos"], json!({"linea": 3, "ruta": texto(&ruta), "app": "prueba"}));
    assert_eq!(lineas[1]["atributos"]["linea"], 4);
    assert_eq!(lineas[2]["atributos"]["linea"], 5);

    fs::write(&nuevo, "primera\n").expect("crear el que faltaba");
    let n = b.esperar(&[("tipo", "registro.linea"), ("fuente", &fuente_nuevo)], 1, 10);
    assert_eq!(cuerpos(&n), ["primera"]);
    assert_eq!(n[0]["atributos"]["linea"], 1);
    let todo = serde_json::to_string(&b.ver(&[("limite", "1000")])).expect("JSON");
    assert!(!todo.contains("vieja"), "lo que ya había se ha publicado: {todo}");
}

#[test]
fn ev_tail_desde_inicio_lee_lo_que_ya_habia() {
    let mut b = Bus::arrancar("tail-inicio");
    let ruta = b.dir.join("app.log");
    fs::write(&ruta, "uno\ndos\n").expect("fichero");
    b.lanzar(&["tail", "--desde-inicio", &texto(&ruta)]);
    let lineas = b.esperar(&[("tipo", "registro.linea")], 2, 15);
    assert_eq!(cuerpos(&lineas), ["uno", "dos"]);
    assert!(lineas[0]["atributos"].get("app").is_none(), "sin --app no hay app");
}

#[test]
fn ev_tail_vuelve_a_leer_desde_el_principio_un_fichero_truncado() {
    let mut b = Bus::arrancar("tail-truncado");
    let ruta = b.dir.join("app.log");
    fs::write(&ruta, "").expect("fichero");
    b.lanzar(&["tail", &texto(&ruta)]);
    b.esperar(&[("tipo", "registro.inicio")], 1, 15);
    anadir(&ruta, b"a1\na2\n");
    b.esperar(&[("tipo", "registro.linea")], 2, 10);
    fs::write(&ruta, "b1\n").expect("truncar");
    let lineas = b.esperar(&[("tipo", "registro.linea")], 3, 10);
    assert_eq!(cuerpos(&lineas), ["a1", "a2", "b1"]);
    assert_eq!(lineas[2]["atributos"]["linea"], 1);
    let rotado = b.esperar(&[("tipo", "registro.rotado")], 1, 5);
    assert_eq!(rotado[0]["atributos"]["motivo"], "encogido");
    assert!(rotado[0]["seq"].as_i64() < lineas[2]["seq"].as_i64(), "el aviso va antes que lo nuevo");
}

#[test]
fn ev_tail_resume_en_un_aviso_lo_que_pasa_del_tope() {
    let mut b = Bus::arrancar("tail-tope");
    let ruta = b.dir.join("app.log");
    fs::write(&ruta, "").expect("fichero");
    b.lanzar(&["tail", &texto(&ruta), "--por-segundo", "3"]);
    b.esperar(&[("tipo", "registro.inicio")], 1, 15);
    let diez: String = (1..=10).map(|i| format!("l{i}\n")).collect();
    anadir(&ruta, diez.as_bytes());
    let tope = b.esperar(&[("tipo", "registro.tope")], 1, 10);
    assert_eq!(tope[0]["gravedad"], "aviso");
    assert_eq!(tope[0]["atributos"]["descartadas"], 7);
    assert_eq!(tope[0]["atributos"]["desde_linea"], 4);
    assert_eq!(tope[0]["atributos"]["hasta_linea"], 10);
    assert_eq!(cuerpos(&b.ver(&[("tipo", "registro.linea")])), ["l1", "l2", "l3"]);
}

#[test]
fn ev_tail_ve_a_un_escritor_que_mantiene_el_fichero_abierto() {
    let mut b = Bus::arrancar("tail-abierto");
    let ruta = b.dir.join("app.log");
    let f = OpenOptions::new().create(true).append(true).open(&ruta).expect("abrir");
    b.lanzar(&["tail", &texto(&ruta)]);
    b.esperar(&[("tipo", "registro.inicio")], 1, 15);
    let mut w = BufWriter::new(f);
    for i in 1..=3 {
        writeln!(w, "abierta {i}").expect("escribir");
        w.flush().expect("vaciar el búfer, sin cerrar");
        let t = Instant::now();
        b.esperar(&[("tipo", "registro.linea")], i, 10);
        eprintln!("línea {i} en el bus a los {:?}", t.elapsed());
    }
    assert_eq!(cuerpos(&b.ver(&[("tipo", "registro.linea")])), ["abierta 1", "abierta 2", "abierta 3"]);
    drop(w);
}

#[cfg(windows)]
#[test]
fn ev_enganchar_sigue_los_ficheros_de_su_configuracion() {
    let mut b = Bus::arrancar("enganchar-fichero");
    let ruta = b.dir.join("app.log");
    fs::write(&ruta, "vieja\n").expect("fichero");
    let config = b.dir.join("enganches.toml");
    fs::write(&config, format!("[[fichero]]\nruta = '{}'\napp = \"prueba\"\npor_segundo = 50\n", texto(&ruta))).expect("config");
    b.lanzar(&["enganchar", "--config", &texto(&config), "--segundos", "60"]);
    b.esperar(&[("tipo", "registro.inicio")], 1, 15);
    anadir(&ruta, b"igual\nigual\n");
    let lineas = b.esperar(&[("tipo", "registro.linea")], 2, 10);
    assert_eq!(cuerpos(&lineas), ["igual", "igual"], "en enganchar, dos líneas iguales no son una repetición");
    assert_eq!(lineas[0]["fuente"], format!("fichero:{}", texto(&ruta)));
    assert_eq!(lineas[0]["atributos"]["app"], "prueba");
    assert_eq!(lineas[1]["atributos"]["linea"], 3);
    assert!(lineas[0].get("correlacion").is_none(), "la app no está vigilada ni abierta: sin arranque al que atarla");
}
