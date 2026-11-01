//! Enganches, fase 1. Primero las piezas puras (configuración, caídas del registro, estado de cada app, topes); al
//! final, las de escritorio: `ev enganchar` de verdad contra un bus de verdad y un doble que abre una ventana, se cuelga
//! o se cae. Esas necesitan un escritorio con sesión y van aparte: `cargo test --test enganches -- --ignored`.

use eventos::enganches::{
    caida_desde_xml, cargar, es_excepcion, orden_de_aviso, validar, Config, Limitador, Vigia, CONSULTA_CAIDAS,
};
use eventos::evento::Evento;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

/// El XML de una caída real de este equipo (el Lector, 2026-10-20), tal como lo da `EvtRender`.
const XML_LECTOR: &str = "<Event xmlns='http://schemas.microsoft.com/win/2004/08/events/event'><System><Provider Name='Application Error' Guid='{a0e9b465-b939-57d7-b27d-95d8e925ff57}'/><EventID>1000</EventID><Version>0</Version><Level>2</Level><Task>100</Task><Opcode>0</Opcode><Keywords>0x8000000000000000</Keywords><TimeCreated SystemTime='2026-10-20T20:43:29.4984612Z'/><EventRecordID>78226</EventRecordID><Correlation/><Execution ProcessID='52284' ThreadID='42300'/><Channel>Application</Channel><Computer>Latveria</Computer></System><EventData><Data Name='AppName'>Lector.exe</Data><Data Name='AppVersion'>0.0.0.0</Data><Data Name='AppTimeStamp'>69afa92f</Data><Data Name='ModuleName'>Lector.exe</Data><Data Name='ModuleVersion'>0.0.0.0</Data><Data Name='ModuleTimeStamp'>69afa92f</Data><Data Name='ExceptionCode'>80000003</Data><Data Name='FaultingOffset'>00000000073d0e6c</Data><Data Name='ProcessId'>0xce50</Data><Data Name='ProcessCreationTime'>0x1dd60d39a0ca8e5</Data><Data Name='AppPath'>F:\\La fabrica\\lector pdf epub etc con integración con ia\\dist\\Lector-win32-x64\\Lector.exe</Data><Data Name='ModulePath'>F:\\La fabrica\\Lector.exe</Data><Data Name='IntegratorReportId'>7df2462b-a58c-49ad-84a2-08b16ca6fa80</Data><Data Name='PackageFullName'></Data><Data Name='PackageRelativeAppId'></Data></EventData></Event>";

fn config(toml: &str) -> Config {
    let c: Config = toml::from_str(toml).expect("toml de prueba");
    validar(&c).expect("configuración válida");
    c
}

fn dos_apps() -> Config {
    config(
        r#"
        colgada_segundos = 10
        [[app]]
        slug = "lector"
        nombre = "Lector"
        coincide_exe = "Lector.exe"
        [[app]]
        slug = "notas"
        nombre = "Notas"
        coincide_exe = "notas.exe"
        titulo = true
        por_hora = 3
        "#,
    )
}

fn tipos(eventos: &[Evento]) -> Vec<&str> {
    eventos.iter().map(|e| e.tipo.as_str()).collect()
}

// --- Configuración ---

#[test]
fn el_enganches_toml_del_repo_carga_y_trae_las_tres_apps_elegidas() {
    let c = cargar(std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/enganches.toml"))).expect("enganches.toml");
    let slugs: Vec<&str> = c.apps.iter().map(|a| a.slug.as_str()).collect();
    assert_eq!(slugs, ["constructo", "obsidian", "brave"]);
    assert!(c.apps.iter().all(|a| a.modo == "fuera" && !a.titulo));
    let aviso = c.aviso.expect("aviso en pantalla");
    assert_eq!(aviso.tipos, ["app.sin_respuesta", "app.caida"]);
    assert!(aviso.orden.iter().any(|p| p.ends_with(r"avisos\avisos.py")) && aviso.orden.contains(&"{cuerpo}".to_string()));
}

#[test]
fn el_aviso_solo_sale_con_sus_tipos_y_lleva_el_texto_como_un_argumento() {
    let c = config(
        "[aviso]\ntipos = [\"app.caida\"]\norden = ['aviso.exe', 'nuevo', '{cuerpo}', '--app={app}', '{tipo}']\n[[app]]\nslug = \"lector\"\nnombre = \"Lector\"\ncoincide_exe = \"Lector.exe\"",
    );
    let aviso = c.aviso.clone().expect("aviso");
    let mut v = Vigia::new(&c);
    let inicio = v.ventana(10, "Lector.exe", false);
    assert_eq!(orden_de_aviso(&aviso, &inicio[0]), None, "app.inicio no está en los tipos");
    let mut caida = caida_desde_xml(XML_LECTOR).expect("caída");
    caida.modulo = "x.dll\" & del /q *".into();
    let e = &v.caida(&caida)[0];
    let orden = orden_de_aviso(&aviso, e).expect("app.caida sí");
    assert_eq!(orden.len(), 5, "el texto va entero en un solo argumento");
    assert_eq!(orden[2], "Se ha caído Lector: excepción 0x80000003 en x.dll\" & del /q *");
    assert_eq!(orden[3], "--app=lector");
    assert_eq!(orden[4], "app.caida");

    let mala = |toml: &str| validar(&toml::from_str::<Config>(toml).expect("toml")).expect_err("tenía que rechazarse").to_string();
    assert!(mala("[aviso]\ntipos = [\"app.caida\"]\norden = []").contains("falta la orden"));
    assert!(mala("[aviso]\ntipos = []\norden = ['a.exe']").contains("faltan los tipos"));
    assert!(mala("[aviso]\ntipos = [\"app.cuelgue\"]\norden = ['a.exe']").contains("tipo desconocido"));
}

#[test]
fn la_configuracion_rechaza_lo_que_no_vale() {
    let mala = |toml: &str| {
        let c: Result<Config, _> = toml::from_str(toml);
        match c {
            Err(_) => String::from("no es TOML válido para la configuración"),
            Ok(c) => validar(&c).expect_err("tenía que rechazarse").to_string(),
        }
    };
    let app = |extra: &str| format!("[[app]]\nslug = \"a\"\nnombre = \"A\"\ncoincide_exe = \"a.exe\"\n{extra}");
    assert!(mala(&app("modo = \"upgrade\"")).contains("upgrade aún no existe"));
    assert!(mala(&app("modo = \"dentro\"")).contains("modo desconocido"));
    assert!(mala(&app("por_hora = 0")).contains("por_hora"));
    assert!(mala(&app("campo_inventado = 1")).contains("no es TOML válido"), "deny_unknown_fields");
    assert!(mala(&format!("{}\n{}", app(""), app(""))).contains("repetido"));
    let mismo_exe = "[[app]]\nslug = \"a\"\nnombre = \"A\"\ncoincide_exe = \"A.EXE\"\n[[app]]\nslug = \"b\"\nnombre = \"B\"\ncoincide_exe = \"a.exe\"";
    assert!(mala(mismo_exe).contains("ya usa el ejecutable"));
    let con_ruta = "[[app]]\nslug = \"a\"\nnombre = \"A\"\ncoincide_exe = 'C:\\x\\a.exe'";
    assert!(mala(con_ruta).contains("sin ruta"));
    let slug_raro = "[[app]]\nslug = \"Mi App\"\nnombre = \"A\"\ncoincide_exe = \"a.exe\"";
    assert!(mala(slug_raro).contains("slug"));
    assert!(mala(&format!("colgada_segundos = 0\n{}", app(""))).contains("colgada_segundos"));
}

// --- Caídas del registro ---

#[test]
fn una_caida_real_del_registro_se_lee_entera() {
    let c = caida_desde_xml(XML_LECTOR).expect("es una caída");
    assert_eq!(c.exe, "Lector.exe");
    assert_eq!(c.excepcion, "0x80000003");
    assert_eq!(c.modulo, "Lector.exe");
    assert_eq!(c.pid, Some(0xce50));
    assert_eq!(c.informe, "7df2462b-a58c-49ad-84a2-08b16ca6fa80");
    assert!(c.ruta.ends_with(r"Lector-win32-x64\Lector.exe") && c.ruta.contains("integración"), "{}", c.ruta);
}

#[test]
fn un_id_1000_de_otro_proveedor_no_es_una_caida() {
    // Lo que confundió el recuento del 2026-10-30: el servicio del sandbox de Codex escribe avisos normales con Id 1000.
    let codex = XML_LECTOR.replace("Name='Application Error'", "Name='CodexSandboxService.OpenAI.Codex'");
    assert_eq!(caida_desde_xml(&codex), None);
    assert_eq!(caida_desde_xml(&XML_LECTOR.replace("<EventID>1000<", "<EventID>1001<")), None);
    assert!(CONSULTA_CAIDAS.contains("Provider[@Name='Application Error']") && CONSULTA_CAIDAS.contains("EventID=1000"));
}

#[test]
fn las_entidades_y_las_variantes_del_xml_se_respetan() {
    let xml = XML_LECTOR
        .replace(r"F:\La fabrica\lector pdf epub etc con integración con ia", r"D:\R&amp;D &lt;pruebas&gt; &#x41;&#66;")
        .replace("<EventID>1000</EventID>", "<EventID Qualifiers='0'>1000</EventID>")
        .replace("<Data Name='PackageFullName'></Data>", "<Data Name='PackageFullName'/>")
        .replace("<Data Name='ExceptionCode'>80000003</Data>", "<Data Name='ExceptionCode'>c0000005</Data>")
        .replace("0xce50", "4242");
    let c = caida_desde_xml(&xml).expect("es una caída");
    assert!(c.ruta.starts_with(r"D:\R&D <pruebas> AB\"), "{}", c.ruta);
    assert_eq!(c.excepcion, "0xc0000005");
    assert_eq!(c.pid, Some(4242));
}

// --- Estado de cada app ---

#[test]
fn una_app_con_varios_procesos_es_un_solo_arranque() {
    let mut v = Vigia::new(&dos_apps());
    let inicio = v.ventana(10, r"D:\Programas\LECTOR.EXE", false);
    assert_eq!(tipos(&inicio), ["app.inicio"]);
    let e = &inicio[0];
    assert_eq!(e.fuente, "app:lector");
    assert_eq!(e.cuerpo, json!("Se ha abierto Lector"));
    assert_eq!(e.confianza.as_deref(), Some("local"));
    assert_eq!(e.atributo("app"), Some(&json!("lector")));
    let correlacion = e.correlacion.clone().expect("correlación");
    assert!(correlacion.starts_with("app:lector:"));

    assert!(v.ventana(11, r"C:\otra\lector.exe", false).is_empty(), "segundo proceso, mismo arranque");
    assert!(v.ventana(10, r"D:\Programas\Lector.exe", false).is_empty(), "otra ventana del mismo proceso");
    assert!(v.ventana(99, r"C:\Windows\explorer.exe", false).is_empty(), "no vigilada");
    assert!(!v.vigila(99));
    assert!(v.termina(10, 0).is_empty(), "queda otro proceso");
    let fin = v.termina(11, 0);
    assert_eq!(tipos(&fin), ["app.fin"]);
    assert_eq!(fin[0].correlacion.as_deref(), Some(correlacion.as_str()));
    assert_eq!(fin[0].atributo("cierre"), Some(&json!("normal")));

    let otra = v.ventana(12, "Lector.exe", true);
    assert_eq!(otra[0].cuerpo, json!("Lector ya estaba en marcha"));
    assert_eq!(otra[0].atributo("al_arrancar"), Some(&json!(true)));
    assert_ne!(otra[0].correlacion.as_deref(), Some(correlacion.as_str()), "cada arranque, su correlación");
}

#[test]
fn el_codigo_de_salida_dice_si_fue_una_excepcion() {
    assert!(es_excepcion(0xC000_0005) && es_excepcion(0xC000_0409) && es_excepcion(0x8000_0003));
    assert!(!es_excepcion(0) && !es_excepcion(1) && !es_excepcion(0xFFFF_FFFF), "exit(-1) no es una excepción");
    let mut v = Vigia::new(&dos_apps());
    v.ventana(10, "Lector.exe", false);
    let fin = v.termina(10, 0xC000_0005);
    assert_eq!(fin[0].atributo("cierre"), Some(&json!("excepcion")));
    assert_eq!(fin[0].atributo("codigo_hex"), Some(&json!("0xC0000005")));
    assert_eq!(fin[0].cuerpo, json!("Se ha cerrado Lector con una excepción (0xC0000005)"));
}

#[test]
fn colgada_solo_avisa_pasado_el_umbral_y_una_vez() {
    let mut v = Vigia::new(&dos_apps());
    let t0 = Instant::now();
    v.ventana(10, "Lector.exe", false);
    assert!(v.respuesta(10, true, t0).is_empty());
    assert!(v.respuesta(10, true, t0 + Duration::from_secs(9)).is_empty(), "aún no lleva 10 s");
    assert!(v.respuesta(10, false, t0 + Duration::from_secs(9)).is_empty(), "se recupera antes del umbral: sin aviso");
    assert!(v.respuesta(10, true, t0 + Duration::from_secs(20)).is_empty(), "vuelve a empezar a contar");
    let aviso = v.respuesta(10, true, t0 + Duration::from_secs(30));
    assert_eq!(tipos(&aviso), ["app.sin_respuesta"]);
    assert_eq!(aviso[0].gravedad.as_deref(), Some("aviso"));
    assert_eq!(aviso[0].cuerpo, json!("Lector no responde (lleva al menos 15 s)"), "10 s nuestros + 5 de Windows");
    assert!(v.respuesta(10, true, t0 + Duration::from_secs(40)).is_empty(), "un solo aviso");
    let vuelta = v.respuesta(10, false, t0 + Duration::from_secs(42));
    assert_eq!(tipos(&vuelta), ["app.respuesta"]);
    assert_eq!(vuelta[0].atributo("segundos"), Some(&json!(27)));
    assert_eq!(vuelta[0].correlacion, aviso[0].correlacion);
}

#[test]
fn si_se_cierra_colgada_el_fin_lo_dice() {
    let mut v = Vigia::new(&dos_apps());
    let t0 = Instant::now();
    v.ventana(10, "Lector.exe", false);
    v.respuesta(10, true, t0);
    assert_eq!(tipos(&v.respuesta(10, true, t0 + Duration::from_secs(10))), ["app.sin_respuesta"]);
    let fin = v.termina(10, 1);
    assert_eq!(fin[0].atributo("estaba_colgada"), Some(&json!(true)));
    assert!(v.respuesta(10, false, t0 + Duration::from_secs(11)).is_empty(), "proceso ya olvidado");
}

#[test]
fn los_titulos_esperan_el_respiro_y_no_se_repiten() {
    let mut v = Vigia::new(&dos_apps());
    let t0 = Instant::now();
    v.ventana(10, "Lector.exe", false);
    v.titulo(10, "Libro 1", t0);
    assert!(v.tick(t0 + Duration::from_secs(5)).is_empty(), "Lector no tiene titulo = true");

    v.ventana(20, "notas.exe", false);
    v.titulo(20, "  uno  ", t0);
    v.titulo(20, "dos", t0 + Duration::from_millis(300));
    assert!(v.tick(t0 + Duration::from_millis(600)).is_empty(), "el respiro cuenta desde el último cambio");
    let t = v.tick(t0 + Duration::from_millis(800));
    assert_eq!(tipos(&t), ["app.titulo"]);
    assert_eq!(t[0].cuerpo, json!("dos"));
    assert_eq!(t[0].confianza.as_deref(), Some("externa"), "el título puede venir de una web");
    v.titulo(20, "dos", t0 + Duration::from_secs(2));
    assert!(v.tick(t0 + Duration::from_secs(3)).is_empty(), "mismo título que el último publicado");
    v.titulo(20, "", t0 + Duration::from_secs(3));
    assert!(v.tick(t0 + Duration::from_secs(4)).is_empty(), "título vacío");
}

#[test]
fn una_caida_va_al_arranque_de_su_proceso_aunque_ya_haya_terminado() {
    let mut v = Vigia::new(&dos_apps());
    let mut c = caida_desde_xml(XML_LECTOR).expect("caída");
    c.pid = Some(10);
    let inicio = v.ventana(10, "Lector.exe", false);
    v.termina(10, 0x8000_0003);
    let caida = v.caida(&c);
    assert_eq!(tipos(&caida), ["app.caida"]);
    assert_eq!(caida[0].gravedad.as_deref(), Some("error"));
    assert_eq!(caida[0].correlacion, inicio[0].correlacion);
    assert_eq!(caida[0].cuerpo, json!("Se ha caído Lector: excepción 0x80000003 en Lector.exe"));
    assert_eq!(caida[0].atributo("informe"), Some(&json!("7df2462b-a58c-49ad-84a2-08b16ca6fa80")));

    c.pid = Some(77);
    assert_eq!(v.caida(&c)[0].correlacion, None, "un proceso que nunca vimos: sin arranque al que atarla");
    c.exe = "otra.exe".into();
    assert!(v.caida(&c).is_empty(), "solo las apps vigiladas");
}

// --- Topes ---

fn de(fuente: &str, tipo: &str, cuerpo: &str, gravedad: &str) -> Evento {
    Evento { gravedad: Some(gravedad.into()), ..Evento::nuevo(fuente, tipo, Value::String(cuerpo.into())) }
}

#[test]
fn el_tope_por_hora_avisa_una_vez_y_cuenta_lo_callado() {
    let mut l = Limitador::new(&dos_apps());
    let t0 = Instant::now();
    let mut salen = Vec::new();
    for i in 0..6u64 {
        salen.extend(l.admitir(de("app:notas", "app.titulo", &format!("t{i}"), "info"), t0 + Duration::from_secs(i * 3)));
    }
    assert_eq!(tipos(&salen), ["app.titulo", "app.titulo", "app.titulo", "enganche.tope"]);
    let tope = &salen[3];
    assert_eq!(tope.fuente, "enganche:notas");
    assert_eq!(tope.gravedad.as_deref(), Some("aviso"));
    assert_eq!(tope.atributo("callados"), Some(&json!(1)));
    let caida = l.admitir(de("app:notas", "app.caida", "cae", "error"), t0 + Duration::from_secs(20));
    assert_eq!(tipos(&caida), ["app.caida"], "una caída pasa aunque el tope esté lleno");
    let hora_despues = l.admitir(de("app:notas", "app.titulo", "otra", "info"), t0 + Duration::from_secs(3700));
    assert_eq!(tipos(&hora_despues), ["app.titulo"], "la ventana se desliza");
}

#[test]
fn lo_repetido_seguido_no_sale_dos_veces() {
    let mut l = Limitador::new(&dos_apps());
    let t0 = Instant::now();
    assert_eq!(l.admitir(de("app:lector", "app.inicio", "x", "info"), t0).len(), 1);
    assert!(l.admitir(de("app:lector", "app.inicio", "x", "info"), t0 + Duration::from_secs(1)).is_empty());
    assert_eq!(l.admitir(de("app:lector", "app.fin", "x", "info"), t0 + Duration::from_secs(1)).len(), 1, "otro tipo");
    assert_eq!(l.admitir(de("app:lector", "app.inicio", "y", "info"), t0 + Duration::from_secs(1)).len(), 1, "otro cuerpo");
    assert_eq!(l.admitir(de("app:lector", "app.inicio", "x", "info"), t0 + Duration::from_secs(4)).len(), 1, "pasado el margen");
    assert_eq!(l.admitir(de("app:lector", "app.caida", "c", "error"), t0).len(), 1);
    assert!(l.admitir(de("app:lector", "app.caida", "c", "error"), t0).is_empty(), "las caídas tampoco se repiten");
}

#[test]
fn el_tope_global_por_segundo_frena_una_avalancha() {
    let c = config("por_segundo_global = 2\n[[app]]\nslug = \"a\"\nnombre = \"A\"\ncoincide_exe = \"a.exe\"\npor_hora = 1000");
    let mut l = Limitador::new(&c);
    let t0 = Instant::now();
    let mut salen = Vec::new();
    for i in 0..5 {
        salen.extend(l.admitir(de("app:a", "app.titulo", &format!("t{i}"), "info"), t0));
    }
    assert_eq!(tipos(&salen), ["app.titulo", "app.titulo", "enganche.tope"]);
    assert_eq!(salen[2].fuente, "enganche:global");
    assert_eq!(l.admitir(de("app:a", "app.titulo", "luego", "info"), t0 + Duration::from_secs(1)).len(), 1);
}

// --- Carpetas vigiladas (Grupo 2c) ---

mod carpetas {
    use super::{config, tipos};
    use eventos::carpetas::{ignorado, Carpetas};
    use eventos::enganches::{validar, Config, Limitador, IGNORAR_POR_DEFECTO};
    use eventos::evento::Evento;
    use serde_json::{json, Value};
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    /// Corto para que las pruebas no se eternicen, y holgado frente a las pausas entre pasos de escritura.
    const ANTIRREBOTE_MS: u64 = 700;
    const PLAZO: Duration = Duration::from_secs(20);

    fn carpeta_temporal(nombre: &str) -> PathBuf {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("reloj").as_nanos();
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("ev-carpeta-{nombre}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("carpeta de prueba");
        dir
    }

    fn toml_de(dir: &Path, extra: &str) -> String {
        format!("[[carpeta]]\nruta = '{}'\nantirrebote_ms = {ANTIRREBOTE_MS}\n{extra}\n", dir.display())
    }

    fn vigilar(dir: &Path, extra: &str) -> Carpetas {
        Carpetas::new(&config(&toml_de(dir, extra))).expect("vigilar la carpeta")
    }

    /// Recoge hasta que se cumple la condición o vence el plazo (y entonces falla con lo que llegó).
    fn esperar(c: &mut Carpetas, llegados: &mut Vec<Evento>, hasta: impl Fn(&[Evento]) -> bool) {
        let fin = Instant::now() + PLAZO;
        loop {
            llegados.extend(c.recoger(Instant::now()));
            if hasta(llegados.as_slice()) {
                return;
            }
            assert!(Instant::now() < fin, "no llegó lo esperado en {PLAZO:?}; llegó: {llegados:?}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn nombre(e: &Evento) -> &str {
        e.atributo("nombre").and_then(Value::as_str).unwrap_or_default()
    }

    fn con_nombre(n: &'static str) -> impl Fn(&[Evento]) -> bool {
        move |v: &[Evento]| v.iter().any(|e| nombre(e) == n)
    }

    /// Los nombres publicados, ordenados.
    fn nombres(v: &[Evento]) -> Vec<&str> {
        let mut n: Vec<&str> = v.iter().map(nombre).collect();
        n.sort_unstable();
        n
    }

    /// Un fichero testigo, creado después de lo que se prueba. Cuando llega su evento, lo de antes ya ha tenido su
    /// antirrebote entero: si tuviera que salir algo más, ya habría salido.
    fn testigo(c: &mut Carpetas, dir: &Path, llegados: &mut Vec<Evento>) {
        std::fs::write(dir.join("testigo.txt"), "t").expect("testigo");
        esperar(c, llegados, con_nombre("testigo.txt"));
    }

    #[test]
    fn la_configuracion_de_carpetas_trae_sus_valores_por_defecto() {
        let dir = carpeta_temporal("defecto");
        let c = config(&format!("[[carpeta]]\nruta = '{}'", dir.display()));
        let k = &c.carpetas[0];
        assert!(!k.recursiva);
        assert_eq!(k.ignorar, IGNORAR_POR_DEFECTO);
        assert_eq!(k.ignorar, ["*.tmp", "*.crdownload", "*.part", "~$*"]);
        assert_eq!(k.antirrebote_ms, 2000);
        assert_eq!(k.por_hora, 120);
        assert!(c.apps.is_empty(), "las carpetas no necesitan apps");
    }

    #[test]
    fn validar_rechaza_las_carpetas_que_no_valen_y_dice_cual() {
        let mala = |toml: &str| match toml::from_str::<Config>(toml) {
            Err(_) => String::from("no es TOML válido para la configuración"),
            Ok(c) => validar(&c).expect_err("tenía que rechazarse").to_string(),
        };
        let dir = carpeta_temporal("validar");
        let fichero = dir.join("no-soy-carpeta.txt");
        std::fs::write(&fichero, "x").expect("fichero");
        let falta = dir.join("no-existo");

        assert!(mala("[[carpeta]]\nruta = ''").contains("ruta vacía"));
        assert!(mala("[[carpeta]]\nruta = '   '").contains("ruta vacía"));
        assert!(mala("[[carpeta]]\nrecursiva = true").contains("no es TOML válido"), "la ruta es obligatoria");
        let e = mala(&format!("[[carpeta]]\nruta = '{}'", falta.display()));
        assert!(e.contains("no existe") && e.contains(&falta.display().to_string()), "{e}");
        let e = mala(&format!("[[carpeta]]\nruta = '{}'", fichero.display()));
        assert!(e.contains("no es una carpeta") && e.contains(&fichero.display().to_string()), "{e}");
        let e = mala(&format!("{}{}", toml_de(&dir, ""), toml_de(&dir, "")));
        assert!(e.contains("repetida") && e.contains(&dir.display().to_string()), "{e}");
        let con_barra = format!("{}{}", toml_de(&dir, ""), toml_de(&dir.join(""), ""));
        assert!(mala(&con_barra).contains("repetida"), "la misma carpeta escrita de otra forma");
        assert!(mala(&toml_de(&dir, "por_hora = 0")).contains("por_hora"));
        assert!(mala(&toml_de(&dir, "campo_inventado = 1")).contains("no es TOML válido"), "deny_unknown_fields");
        let otra = carpeta_temporal("validar-otra");
        validar(&toml::from_str(&format!("{}{}", toml_de(&dir, ""), toml_de(&otra, ""))).expect("toml")).expect("dos carpetas distintas valen");
    }

    #[test]
    fn los_patrones_de_ignorar_miran_el_nombre_sin_distinguir_mayusculas() {
        let p: Vec<String> = IGNORAR_POR_DEFECTO.iter().map(|s| s.to_string()).collect();
        for si in ["a.tmp", "INFORME.TMP", "informe.pdf.crdownload", "video.part", "~$carta.docx", ".tmp"] {
            assert!(ignorado(si, &p), "{si} se ignora");
        }
        for no in ["informe.pdf", "a.tmpx", "video.partial", "carta~$.docx", "tmp", "part.txt"] {
            assert!(!ignorado(no, &p), "{no} no se ignora");
        }
        let otros = vec!["foto_??.jpg".to_string(), "*copia*".to_string()];
        assert!(ignorado("foto_12.jpg", &otros) && !ignorado("foto_123.jpg", &otros));
        assert!(ignorado("informe (copia) 2.pdf", &otros));
    }

    #[test]
    fn una_carpeta_tiene_su_tope_por_hora_en_el_limitador() {
        let dir = carpeta_temporal("tope");
        let c = config(&toml_de(&dir, "por_hora = 2"));
        let fuente = format!("carpeta:{}", dir.display());
        let mut l = Limitador::new(&c);
        let t0 = Instant::now();
        let mut salen = Vec::new();
        for i in 0..4u64 {
            let mut e = Evento::nuevo(fuente.clone(), "carpeta.fichero", Value::String(format!("f{i} (1 bytes)")));
            e.gravedad = Some("info".into());
            salen.extend(l.admitir(e, t0 + Duration::from_secs(i)));
        }
        assert_eq!(tipos(&salen), ["carpeta.fichero", "carpeta.fichero", "enganche.tope"]);
        assert_eq!(salen[2].atributo("callados"), Some(&json!(1)));
    }

    #[test]
    fn un_fichero_nuevo_da_un_evento_con_sus_atributos() {
        let dir = carpeta_temporal("nuevo");
        let mut c = vigilar(&dir, "");
        let tanteos = c.tanteos();
        assert_eq!(tipos(&tanteos), ["enganche.tanteo"]);
        assert_eq!(tanteos[0].fuente, format!("enganche:carpeta:{}", dir.display()));

        std::fs::write(dir.join("hola.txt"), "hola mundo").expect("escribir");
        let mut llegados = Vec::new();
        esperar(&mut c, &mut llegados, con_nombre("hola.txt"));
        assert_eq!(llegados.len(), 1, "{llegados:?}");
        let e = &llegados[0];
        let ruta = dir.display().to_string();
        assert_eq!(e.tipo, "carpeta.fichero");
        assert_eq!(e.fuente, format!("carpeta:{ruta}"));
        assert_eq!(e.gravedad.as_deref(), Some("info"));
        assert_eq!(e.cuerpo, json!("hola.txt (10 bytes)"));
        assert_eq!(e.atributos, json!({"carpeta": ruta, "relativa": "hola.txt", "nombre": "hola.txt", "bytes": 10}));
        assert_eq!(e.confianza.as_deref(), Some("externa"), "el nombre lo puede haber elegido una web");
    }

    #[test]
    fn un_fichero_escrito_en_varios_pasos_da_un_solo_evento() {
        let dir = carpeta_temporal("pasos");
        let mut c = vigilar(&dir, "");
        let mut llegados = Vec::new();
        {
            let mut f = std::fs::File::create(dir.join("datos.csv")).expect("crear");
            for paso in 0..4 {
                f.write_all(format!("fila {paso}\n").as_bytes()).expect("escribir");
                f.flush().expect("vaciar");
                // Mientras escribe, se recoge: así se ve que no sale nada a medias.
                let pausa = Instant::now() + Duration::from_millis(150);
                while Instant::now() < pausa {
                    llegados.extend(c.recoger(Instant::now()));
                    std::thread::sleep(Duration::from_millis(30));
                }
            }
        }
        let total = std::fs::metadata(dir.join("datos.csv")).expect("metadatos").len();
        esperar(&mut c, &mut llegados, con_nombre("datos.csv"));
        testigo(&mut c, &dir, &mut llegados);
        assert_eq!(nombres(&llegados), ["datos.csv", "testigo.txt"], "{llegados:?}");
        let e = llegados.iter().find(|e| nombre(e) == "datos.csv").expect("datos.csv");
        assert_eq!(e.atributo("bytes"), Some(&json!(total)), "el tamaño final, no uno a medias");
    }

    #[test]
    fn una_descarga_renombrada_da_un_solo_evento_con_el_nombre_final() {
        let dir = carpeta_temporal("descarga");
        let mut c = vigilar(&dir, "");
        let mut llegados = Vec::new();
        let a_medias = dir.join("informe.pdf.crdownload");
        {
            let mut f = std::fs::File::create(&a_medias).expect("crear");
            for _ in 0..3 {
                f.write_all(&[7u8; 1000]).expect("escribir");
                llegados.extend(c.recoger(Instant::now()));
            }
        }
        std::fs::rename(&a_medias, dir.join("informe.pdf")).expect("renombrar");
        esperar(&mut c, &mut llegados, con_nombre("informe.pdf"));
        testigo(&mut c, &dir, &mut llegados);
        assert_eq!(nombres(&llegados), ["informe.pdf", "testigo.txt"], "{llegados:?}");
        let e = llegados.iter().find(|e| nombre(e) == "informe.pdf").expect("informe.pdf");
        assert_eq!(e.atributo("relativa"), Some(&json!("informe.pdf")));
        assert_eq!(e.atributo("bytes"), Some(&json!(3000)));
    }

    #[test]
    fn un_nombre_ignorado_no_da_ninguno() {
        let dir = carpeta_temporal("ignorado");
        let mut c = vigilar(&dir, "");
        std::fs::write(dir.join("borrador.tmp"), "x").expect("escribir");
        std::fs::write(dir.join("~$carta.docx"), "x").expect("escribir");
        std::fs::write(dir.join("video.part"), "x").expect("escribir");
        let mut llegados = Vec::new();
        testigo(&mut c, &dir, &mut llegados);
        assert_eq!(nombres(&llegados), ["testigo.txt"], "{llegados:?}");

        let dir = carpeta_temporal("ignorado-propio");
        let mut c = vigilar(&dir, "ignorar = [\"*.log\"]");
        std::fs::write(dir.join("traza.log"), "x").expect("escribir");
        std::fs::write(dir.join("sí.tmp"), "x").expect("escribir");
        let mut llegados = Vec::new();
        testigo(&mut c, &dir, &mut llegados);
        assert_eq!(nombres(&llegados), ["sí.tmp", "testigo.txt"], "ignorar sustituye a los de por defecto");
    }

    #[test]
    fn una_subcarpeta_solo_cuenta_con_recursiva() {
        let dir = carpeta_temporal("sin-recursiva");
        let mut c = vigilar(&dir, "");
        std::fs::create_dir(dir.join("sub")).expect("subcarpeta");
        std::fs::write(dir.join("sub").join("dentro.txt"), "abc").expect("escribir");
        let mut llegados = Vec::new();
        testigo(&mut c, &dir, &mut llegados);
        assert_eq!(nombres(&llegados), ["testigo.txt"], "ni la subcarpeta ni lo de dentro: {llegados:?}");

        let dir = carpeta_temporal("recursiva");
        let mut c = vigilar(&dir, "recursiva = true");
        std::fs::create_dir(dir.join("sub")).expect("subcarpeta");
        std::fs::write(dir.join("sub").join("dentro.txt"), "abc").expect("escribir");
        let mut llegados = Vec::new();
        esperar(&mut c, &mut llegados, con_nombre("dentro.txt"));
        testigo(&mut c, &dir, &mut llegados);
        assert_eq!(nombres(&llegados), ["dentro.txt", "testigo.txt"], "la subcarpeta no es un fichero: {llegados:?}");
        let e = llegados.iter().find(|e| nombre(e) == "dentro.txt").expect("dentro.txt");
        let relativa = Path::new("sub").join("dentro.txt").display().to_string();
        assert_eq!(e.atributo("relativa"), Some(&json!(relativa)));
        assert_eq!(e.atributo("bytes"), Some(&json!(3)));
        assert_eq!(e.cuerpo, json!(format!("{relativa} (3 bytes)")));
    }

    /// Una carpeta ya llena, hecha fuera y movida dentro de golpe (como al descomprimir o mover una carpeta):
    /// Windows solo avisa de la carpeta, no de lo que trae.
    fn mover_llena_dentro(dir: &Path, nombre: &str) {
        let fuera = carpeta_temporal(&format!("{nombre}-fuera"));
        std::fs::create_dir_all(fuera.join("fotos").join("viaje")).expect("subcarpetas");
        std::fs::write(fuera.join("fotos").join("una.jpg"), "12345").expect("escribir");
        std::fs::write(fuera.join("fotos").join("viaje").join("dos.jpg"), "1234567").expect("escribir");
        std::fs::rename(fuera.join("fotos"), dir.join("fotos")).expect("mover dentro");
    }

    #[test]
    fn una_subcarpeta_que_llega_llena_publica_lo_que_trae_solo_con_recursiva() {
        let dir = carpeta_temporal("llena");
        let mut c = vigilar(&dir, "recursiva = true");
        mover_llena_dentro(&dir, "llena");
        let mut llegados = Vec::new();
        esperar(&mut c, &mut llegados, |v| con_nombre("una.jpg")(v) && con_nombre("dos.jpg")(v));
        testigo(&mut c, &dir, &mut llegados);
        assert_eq!(nombres(&llegados), ["dos.jpg", "testigo.txt", "una.jpg"], "una vez cada uno: {llegados:?}");
        let e = llegados.iter().find(|e| nombre(e) == "dos.jpg").expect("dos.jpg");
        let relativa = Path::new("fotos").join("viaje").join("dos.jpg").display().to_string();
        assert_eq!(e.atributo("relativa"), Some(&json!(relativa)));
        assert_eq!(e.atributo("bytes"), Some(&json!(7)));

        let dir = carpeta_temporal("llena-sin-recursiva");
        let mut c = vigilar(&dir, "");
        mover_llena_dentro(&dir, "llena-sin-recursiva");
        let mut llegados = Vec::new();
        testigo(&mut c, &dir, &mut llegados);
        assert_eq!(nombres(&llegados), ["testigo.txt"], "sin recursiva, lo de las subcarpetas no cuenta: {llegados:?}");
    }
}

// --- Escritorio: ev enganchar de verdad, contra un bus de verdad y un doble con ventana ---

#[cfg(windows)]
mod escritorio {
    use serde_json::Value;
    use std::net::TcpListener;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::thread;
    use std::time::{Duration, Instant};

    const EV: &str = env!("CARGO_BIN_EXE_ev");

    /// `cargo test` compila los ejemplos junto al binario, en `examples\`.
    fn doble() -> PathBuf {
        let ruta = Path::new(EV).parent().expect("carpeta de ev").join("examples").join("doble_enganche.exe");
        assert!(ruta.exists(), "falta {}; `cargo test` compila los ejemplos", ruta.display());
        ruta
    }

    fn carpeta_temporal(nombre: &str) -> PathBuf {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("reloj").as_nanos();
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("ev-{nombre}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("carpeta de prueba");
        dir
    }

    /// Un bus en un puerto libre, `ev enganchar` vigilando al doble, y ambos cerrados al terminar.
    struct Banco {
        bus: Child,
        enganchar: Option<Child>,
        puerto: u16,
    }

    impl Drop for Banco {
        fn drop(&mut self) {
            for hijo in self.enganchar.iter_mut().chain(std::iter::once(&mut self.bus)) {
                let _ = hijo.kill();
                let _ = hijo.wait();
            }
        }
    }

    impl Banco {
        fn arrancar(nombre: &str) -> Banco {
            let dir = carpeta_temporal(nombre);
            let puerto = TcpListener::bind("127.0.0.1:0").expect("puerto").local_addr().expect("dirección").port();
            let bus = Command::new(EV)
                .args(["servir", "--puerto", &puerto.to_string()])
                .env("EV_DIR", &dir)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("ev servir");
            let config = dir.join("enganches.toml");
            // El aviso en pantalla, de prueba: el propio `ev` publica en este bus lo que saldría en la ventanita.
            let toml = format!(
                "colgada_segundos = 3\n[aviso]\ntipos = [\"app.sin_respuesta\", \"app.caida\"]\norden = ['{EV}', 'publicar', 'aviso.prueba', '{{cuerpo}}']\n[[app]]\nslug = \"doble\"\nnombre = \"Doble\"\ncoincide_exe = \"doble_enganche.exe\"\ntitulo = true\n"
            );
            std::fs::write(&config, toml).expect("enganches.toml de prueba");
            let mut banco = Banco { bus, enganchar: None, puerto };
            let fin = Instant::now() + Duration::from_secs(15);
            while ureq::get(&banco.url("/estado")).call().is_err() {
                assert!(Instant::now() < fin, "el bus no arrancó en 15 s");
                thread::sleep(Duration::from_millis(100));
            }
            let enganchar = Command::new(EV)
                .args(["enganchar", "--config"])
                .arg(&config)
                .args(["--segundos", "120"])
                .env("EV_PUERTO", puerto.to_string())
                .env("EV_DIR", &dir)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("ev enganchar");
            banco.enganchar = Some(enganchar);
            banco.esperar(&[("tipo", "enganche.tanteo")], 1, 15);
            banco
        }

        fn url(&self, ruta: &str) -> String {
            format!("http://127.0.0.1:{}{ruta}", self.puerto)
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
                assert!(Instant::now() < fin, "no llegaron {n} eventos con {filtro:?} en {segundos} s; hay: {v:?}");
                thread::sleep(Duration::from_millis(200));
            }
        }

        /// Con el modo de errores por defecto: `cargo test` pasa a sus hijos uno que apaga el informe de errores de
        /// Windows, y entonces una caída no llega al registro (comprobado el 2026-10-31: lanzado a mano, sí llega).
        fn lanzar_doble(&self, args: &[&str]) -> Child {
            use std::os::windows::process::CommandExt;
            const CREATE_DEFAULT_ERROR_MODE: u32 = 0x0400_0000;
            Command::new(doble())
                .args(args)
                .creation_flags(CREATE_DEFAULT_ERROR_MODE)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("lanzar el doble")
        }
    }

    fn texto<'a>(e: &'a Value, campo: &str) -> &'a str {
        e[campo].as_str().unwrap_or_default()
    }

    #[test]
    #[ignore = "necesita escritorio: cargo test --test enganches -- --ignored"]
    fn abre_cambia_de_titulo_y_cierra_en_un_mismo_arranque() {
        let b = Banco::arrancar("abrir");
        let mut d = b.lanzar_doble(&["abrir"]);
        let pid = d.id();
        assert!(d.wait().expect("doble").success());
        let fin = b.esperar(&[("tipo", "app.fin"), ("fuente", "app:doble")], 1, 15);
        let inicio = b.esperar(&[("tipo", "app.inicio"), ("fuente", "app:doble")], 1, 5);
        let titulos = b.ver(&[("tipo", "app.titulo")]);
        assert_eq!(inicio[0]["atributos"]["pid"], pid);
        assert_eq!(inicio[0]["atributos"]["al_arrancar"], false);
        assert_eq!(texto(&fin[0], "correlacion"), texto(&inicio[0], "correlacion"));
        assert_eq!(fin[0]["atributos"]["cierre"], "normal");
        assert_eq!(fin[0]["atributos"]["codigo"], 0);
        assert!(titulos.iter().any(|t| t["cuerpo"] == "doble 2" && t["correlacion"] == inicio[0]["correlacion"]), "{titulos:?}");
        assert!(b.ver(&[("tipo", "aviso.prueba")]).is_empty(), "abrir y cerrar no sacan aviso en pantalla");
    }

    #[test]
    #[ignore = "necesita escritorio: cargo test --test enganches -- --ignored"]
    fn una_ventana_colgada_avisa_y_avisa_al_volver() {
        let b = Banco::arrancar("colgarse");
        let mut d = b.lanzar_doble(&["colgarse", "14"]);
        let sin = b.esperar(&[("tipo", "app.sin_respuesta")], 1, 30);
        assert_eq!(sin[0]["gravedad"], "aviso");
        let aviso = b.esperar(&[("tipo", "aviso.prueba")], 1, 15);
        assert_eq!(aviso[0]["cuerpo"], sin[0]["cuerpo"], "la ventanita lleva el texto del evento");
        let vuelta = b.esperar(&[("tipo", "app.respuesta")], 1, 30);
        assert_eq!(texto(&vuelta[0], "correlacion"), texto(&sin[0], "correlacion"));
        assert!(d.wait().expect("doble").success());
        let fin = b.esperar(&[("tipo", "app.fin")], 1, 10);
        assert_eq!(fin[0]["atributos"]["estaba_colgada"], false, "ya respondía al cerrarse");
    }

    #[test]
    #[ignore = "necesita escritorio: cargo test --test enganches -- --ignored"]
    fn una_caida_llega_del_registro_de_windows_con_su_arranque() {
        let b = Banco::arrancar("caerse");
        let mut d = b.lanzar_doble(&["caerse"]);
        let codigo = d.wait().expect("doble").code().map(|c| c as u32);
        assert_eq!(codigo, Some(0xC000_0005), "acceso a la dirección 0");
        let caida = b.esperar(&[("tipo", "app.caida")], 1, 60);
        let fin = b.esperar(&[("tipo", "app.fin")], 1, 10);
        assert_eq!(caida[0]["gravedad"], "error");
        assert_eq!(caida[0]["atributos"]["excepcion"], "0xc0000005");
        assert_eq!(caida[0]["atributos"]["exe"], "doble_enganche.exe");
        let aviso = b.esperar(&[("tipo", "aviso.prueba")], 1, 15);
        assert_eq!(aviso[0]["cuerpo"], caida[0]["cuerpo"]);
        assert_eq!(texto(&caida[0], "correlacion"), texto(&fin[0], "correlacion"));
        assert_eq!(fin[0]["atributos"]["cierre"], "excepcion");
    }
}
