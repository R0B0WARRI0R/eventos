#![windows_subsystem = "windows"]
//! Ventana propia del visor del bus de eventos: WebView2 (el motor web de Windows) con la página que sirve
//! `ev servir`. Si el bus no está en marcha, lo arranca sin consola y lo para al cerrar la ventana; si ya estaba en
//! marcha, lo deja como estaba.

use std::fs::{self, File};
use std::io::Write;
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop};
use tao::window::{Icon, WindowBuilder};
use wry::{WebContext, WebViewBuilder};

fn puerto() -> u16 {
    std::env::var("EV_PUERTO").ok().and_then(|p| p.parse().ok()).unwrap_or(8870)
}

/// La misma carpeta de datos que usa `ev` (EV_DIR o D:\dev\eventos\datos).
fn carpeta_datos() -> PathBuf {
    std::env::var_os("EV_DIR").map(Into::into).unwrap_or_else(|| r"D:\dev\eventos\datos".into())
}

/// Reglas para el bus que arranque el visor: EV_REGLAS, o D:\dev\eventos\reglas.toml si existe.
fn reglas() -> Option<PathBuf> {
    match std::env::var_os("EV_REGLAS") {
        Some(r) => Some(r.into()),
        None => Some(PathBuf::from(r"D:\dev\eventos\reglas.toml")).filter(|r| r.exists()),
    }
}

fn escucha(puerto: u16) -> bool {
    TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], puerto)), Duration::from_millis(300)).is_ok()
}

/// El bus, cuando lo arranca el visor: se para al soltarlo.
struct Bus(Child);

impl Drop for Bus {
    fn drop(&mut self) {
        if let Err(e) = self.0.kill() {
            anotar(&format!("no pude parar el bus (pid {}): {e}", self.0.id()));
        }
        let _ = self.0.wait();
    }
}

/// Registro del visor (no tiene consola): `visor.log` en la carpeta de datos.
fn anotar(texto: &str) {
    let ruta = carpeta_datos().join("visor.log");
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(ruta) {
        let _ = writeln!(f, "{texto}");
    }
}

fn arrancar_bus(puerto: u16) -> Result<Option<Bus>, String> {
    if escucha(puerto) {
        return Ok(None);
    }
    let exe = std::env::current_exe().map_err(|e| format!("no sé dónde estoy: {e}"))?;
    let ev = exe.with_file_name("ev.exe");
    if !ev.exists() {
        return Err(format!("no encuentro {} (tiene que estar junto al visor)", ev.display()));
    }
    let datos = carpeta_datos();
    fs::create_dir_all(&datos).map_err(|e| format!("no puedo crear {}: {e}", datos.display()))?;
    let ruta_log = datos.join("servicio.log");
    let log = File::create(&ruta_log).map_err(|e| format!("no puedo escribir {}: {e}", ruta_log.display()))?;
    let log_err = log.try_clone().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(&ev);
    cmd.args(["servir", "--puerto", &puerto.to_string()]);
    if let Some(r) = reglas() {
        cmd.arg("--reglas").arg(r);
    }
    cmd.stdin(Stdio::null()).stdout(log).stderr(log_err);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut hijo = cmd.spawn().map_err(|e| format!("no pude lanzar {}: {e}", ev.display()))?;
    let fin = Instant::now() + Duration::from_secs(15);
    while Instant::now() < fin {
        if escucha(puerto) {
            return Ok(Some(Bus(hijo)));
        }
        if let Ok(Some(estado)) = hijo.try_wait() {
            return Err(format!("el bus se cerró al arrancar ({estado}); el motivo está en {}", ruta_log.display()));
        }
        thread::sleep(Duration::from_millis(150));
    }
    drop(Bus(hijo));
    Err(format!("el bus no respondió en 15 s; mira {}", ruta_log.display()))
}

/// Icono: un anillo con un punto en el centro, naranja, sobre transparente.
fn icono() -> Option<Icon> {
    const LADO: u32 = 64;
    let c = (LADO as f32 - 1.0) / 2.0;
    let cubre = |d: f32, desde: f32, hasta: f32| ((d - desde + 0.5).clamp(0.0, 1.0)) * ((hasta - d + 0.5).clamp(0.0, 1.0));
    let mut rgba = Vec::with_capacity((LADO * LADO * 4) as usize);
    for y in 0..LADO {
        for x in 0..LADO {
            let d = ((x as f32 - c).powi(2) + (y as f32 - c).powi(2)).sqrt();
            let alfa = cubre(d, 22.0, 29.0).max(cubre(d, -1.0, 11.0));
            rgba.extend_from_slice(&[255, 138, 61, (alfa * 255.0) as u8]);
        }
    }
    Icon::from_rgba(rgba, LADO, LADO).ok()
}

fn escapar(texto: &str) -> String {
    texto.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn pagina_error(motivo: &str) -> String {
    format!(
        "<!doctype html><html lang=\"es\"><meta charset=\"utf-8\"><title>Bus de eventos</title>\
         <body style=\"background:#0d1117;color:#e6edf3;font:16px system-ui;padding:40px;max-width:760px\">\
         <h1 style=\"color:#ff8a3d\">El bus de eventos no está disponible</h1><p>{}</p>\
         <p>Prueba a arrancarlo a mano con <code>ev servir --reglas D:\\dev\\eventos\\reglas.toml</code> y vuelve a abrir el visor.</p>\
         </body></html>",
        escapar(motivo)
    )
}

fn main() {
    let puerto = puerto();
    let arranque = arrancar_bus(puerto);
    if let Err(e) = &arranque {
        anotar(&format!("arranque: {e}"));
    }
    let titulo = match &arranque {
        Ok(Some(_)) => "Bus de eventos (servicio iniciado por el visor; se para al cerrar)",
        Ok(None) => "Bus de eventos",
        Err(_) => "Bus de eventos (sin servicio)",
    };

    let event_loop = EventLoop::new();
    let ventana = match WindowBuilder::new()
        .with_title(titulo)
        .with_inner_size(LogicalSize::new(1440.0, 900.0))
        .with_min_inner_size(LogicalSize::new(900.0, 600.0))
        .with_window_icon(icono())
        .build(&event_loop)
    {
        Ok(v) => v,
        Err(e) => {
            anotar(&format!("no pude crear la ventana: {e}"));
            std::process::exit(1);
        }
    };
    // Los datos de WebView2 van a D:, no junto al ejecutable ni en C:.
    let mut contexto = WebContext::new(Some(carpeta_datos().join("visor-webview2")));
    let constructor = WebViewBuilder::new_with_web_context(&mut contexto);
    let constructor = match &arranque {
        Ok(_) => constructor.with_url(format!("http://127.0.0.1:{puerto}/")),
        Err(e) => constructor.with_html(pagina_error(e)),
    };
    let _vista = match constructor.build(&ventana) {
        Ok(v) => v,
        Err(e) => {
            anotar(&format!("no pude abrir WebView2: {e}"));
            std::process::exit(1);
        }
    };

    let mut bus = arranque.ok().flatten();
    event_loop.run(move |evento, _, control| {
        *control = ControlFlow::Wait;
        if let Event::WindowEvent { event: WindowEvent::CloseRequested, .. } = evento {
            // `run` no vuelve: el bus se para aquí y no al salir de main.
            drop(bus.take());
            *control = ControlFlow::Exit;
        }
    });
}
