//! Pulso, el bus de eventos universal (v0). Diseño en `docs/2026-10-29-investigacion.md`, apartados 7 a 9.
//!
//! Un solo servicio (`ev servir`) es dueño del almacén SQLite; todo lo demás (`ev run`, `ev hook`, `ev mcp`, la CLI)
//! le habla por HTTP en 127.0.0.1. Las reglas despiertan agentes y lo que hacen vuelve al bus como eventos.

pub mod almacen;
pub mod arneses;
pub mod cliente;
pub mod enganchar;
pub mod enganches;
pub mod evento;
pub mod hook;
pub mod mcp;
pub mod presencia;
pub mod reglas;
pub mod run;
pub mod servidor;
pub mod suscripciones;
pub mod tail;
pub mod tareas;
pub mod texto;

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Resultado<T> = Result<T, Error>;

/// Puerto por defecto del servicio; `EV_PUERTO` lo cambia.
pub const PUERTO: u16 = 8870;

pub fn puerto() -> u16 {
    std::env::var("EV_PUERTO").ok().and_then(|p| p.parse().ok()).unwrap_or(PUERTO)
}

/// Carpeta de datos (base, cola de pendientes, ficheros temporales); `EV_DIR` la cambia. Siempre absoluta: se pasa a
/// procesos que arrancan en otro directorio (el agente, su `ev mcp`).
pub fn carpeta_datos() -> std::path::PathBuf {
    let carpeta: std::path::PathBuf = match std::env::var_os("EV_DIR") {
        Some(d) => d.into(),
        None if cfg!(windows) => r"D:\dev\eventos\datos".into(),
        None => "datos".into(),
    };
    absoluta(carpeta)
}

/// La ruta como absoluta respecto al directorio actual (sin el prefijo `\\?\` de `canonicalize`).
pub fn absoluta(ruta: std::path::PathBuf) -> std::path::PathBuf {
    std::path::absolute(&ruta).unwrap_or(ruta)
}
