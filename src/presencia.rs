//! Quién está conectado al bus (desde 2026-10-31). Cada `ev mcp` late (`POST /latido`) cada pocos segundos y se
//! despide (`POST /adios`) cuando su sesión cierra la entrada; el servicio lo da por ido si deja de latir durante tres
//! periodos, porque cerrar la ventana de Claude Code mata el proceso sin despedida. Los hooks no sirven para contarlas:
//! esa misma ventana cerrada no deja `SessionEnd` (el 2026-10-31 salían 27 sesiones «abiertas» con una sola viva).
//! Vive en memoria: tras reiniciar el servicio, cada uno reaparece en su siguiente latido.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Periodo por defecto entre latidos, en segundos (`EV_LATIDO_SEGUNDOS` lo cambia en el cliente).
pub const PERIODO: u64 = 15;
/// Latidos perdidos seguidos tras los que se da por ido.
pub const PERDIDOS: u32 = 3;
/// Conectados a la vez como mucho: un cliente que inventara ids no puede llenar la memoria del servicio.
pub const MAXIMO: usize = 256;
const LARGO_ID: usize = 64;
const LARGO_TEXTO: usize = 300;

/// Atributo con el que `ev hook` dice qué `claude.exe` lo lanzó (desde 2026-11-01). `/clear` cambia la sesión sin
/// reiniciar `ev mcp`, que se queda con el CLAUDE_CODE_SESSION_ID de su arranque; el proceso no cambia. Claude Code da su
/// pid a los hooks en CLAUDE_PID, pero no a los MCP (comprobado con la 2.1.316): `ev mcp` usa el de su padre.
pub const ATRIBUTO_CLAUDE: &str = "claude_pid";

/// El pid del proceso padre: en una sesión, el `claude.exe` que lanzó este `ev mcp`.
#[cfg(windows)]
pub fn pid_del_padre() -> Option<u64> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };
    let yo = std::process::id();
    // SAFETY: la foto es un handle propio que se cierra al acabar, y la estructura va a cero con su tamaño puesto, que
    // es lo que piden Process32FirstW y Process32NextW.
    unsafe {
        let foto = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if foto == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut proceso: PROCESSENTRY32W = std::mem::zeroed();
        proceso.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut padre = None;
        let mut hay = Process32FirstW(foto, &mut proceso) != 0;
        while hay {
            if proceso.th32ProcessID == yo {
                padre = Some(u64::from(proceso.th32ParentProcessID));
                break;
            }
            hay = Process32NextW(foto, &mut proceso) != 0;
        }
        CloseHandle(foto);
        padre
    }
}

/// El pid del proceso padre: en una sesión, el `claude` que lanzó este `ev mcp`.
#[cfg(unix)]
pub fn pid_del_padre() -> Option<u64> {
    Some(u64::from(std::os::unix::process::parent_id()))
}

/// Lo que dice un `ev mcp` de sí mismo en cada latido.
#[derive(Debug, Clone, PartialEq)]
pub struct Latido {
    pub id: String,
    /// `sesion` (Claude Code interactivo, con canal) o `agente` (una regla del bus, `claude -p`, Agent SDK).
    pub clase: String,
    pub pid: Option<u64>,
    /// La sesión de Claude Code (`CLAUDE_CODE_SESSION_ID`), si la hay: la de su arranque, aunque luego haya un `/clear`.
    pub sesion: Option<String>,
    /// El proceso padre de este `ev mcp`, si se pudo saber (ver `ATRIBUTO_CLAUDE`).
    pub claude_pid: Option<u64>,
    /// La regla que lanzó al agente (`EV_REGLA`), si la hay.
    pub regla: Option<String>,
    /// El cliente MCP que se presentó en `initialize` (p. ej. `claude-code 2.1.316`).
    pub cliente: Option<String>,
    pub directorio: Option<String>,
    pub periodo: u64,
}

fn texto(v: &Value, clave: &str) -> Option<String> {
    v[clave].as_str().map(str::trim).filter(|s| !s.is_empty()).map(|s| s.chars().take(LARGO_TEXTO).collect())
}

impl Latido {
    pub fn desde_json(v: &Value) -> Result<Latido, String> {
        let id = v["id"].as_str().map(str::trim).unwrap_or("");
        if id.is_empty() || id.len() > LARGO_ID || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Err(format!("«id» tiene que ser de 1 a {LARGO_ID} letras, cifras, - o _"));
        }
        let clase = v["clase"].as_str().unwrap_or("");
        if !matches!(clase, "sesion" | "agente") {
            return Err(format!("«clase» tiene que ser sesion o agente, no «{clase}»"));
        }
        let periodo = match &v["periodo"] {
            Value::Null => PERIODO,
            p => p.as_u64().filter(|p| (1..=300).contains(p)).ok_or("«periodo» tiene que ser de 1 a 300 segundos")?,
        };
        Ok(Latido {
            id: id.into(),
            clase: clase.into(),
            pid: v["pid"].as_u64(),
            sesion: texto(v, "sesion"),
            claude_pid: v["claude_pid"].as_u64(),
            regla: texto(v, "regla"),
            cliente: texto(v, "cliente"),
            directorio: texto(v, "directorio"),
            periodo,
        })
    }
}

struct Entrada {
    latido: Latido,
    /// Primer latido, en UTC.
    desde: String,
    visto: Instant,
}

impl Entrada {
    fn caducada(&self, ahora: Instant) -> bool {
        ahora.saturating_duration_since(self.visto) > Duration::from_secs(self.latido.periodo * PERDIDOS as u64)
    }
}

#[derive(Default)]
pub struct Registro {
    vivos: HashMap<String, Entrada>,
    /// La última sesión que el hook de cada `claude.exe` ha dicho que lleva.
    sesiones: HashMap<u64, String>,
}

impl Registro {
    /// Anota la sesión que lleva un `claude.exe`, según su hook. Si no cabe otro, se olvidan todos: cada uno vuelve con
    /// su siguiente hook, y mientras tanto `lista` da la sesión del arranque de su `ev mcp`.
    pub fn sesion_actual(&mut self, claude_pid: u64, sesion: String) {
        if self.sesiones.len() >= MAXIMO && !self.sesiones.contains_key(&claude_pid) {
            self.sesiones.clear();
        }
        self.sesiones.insert(claude_pid, sesion);
    }

    /// Anota un latido; `desde` es la hora UTC de ahora, que solo se guarda en el primero.
    pub fn latir(&mut self, l: Latido, desde: String, ahora: Instant) -> Result<(), String> {
        self.purgar(ahora);
        if let Some(e) = self.vivos.get_mut(&l.id) {
            e.latido = l;
            e.visto = ahora;
            return Ok(());
        }
        if self.vivos.len() >= MAXIMO {
            return Err(format!("ya hay {MAXIMO} conectados; no caben más"));
        }
        self.vivos.insert(l.id.clone(), Entrada { latido: l, desde, visto: ahora });
        Ok(())
    }

    /// Quita a quien se despide; dice si estaba.
    pub fn adios(&mut self, id: &str) -> bool {
        self.vivos.remove(id).is_some()
    }

    fn purgar(&mut self, ahora: Instant) {
        self.vivos.retain(|_, e| !e.caducada(ahora));
    }

    /// Los conectados, del más antiguo al más reciente.
    pub fn lista(&mut self, ahora: Instant) -> Vec<Value> {
        self.purgar(ahora);
        let mut entradas: Vec<&Entrada> = self.vivos.values().collect();
        entradas.sort_by(|a, b| a.desde.cmp(&b.desde).then_with(|| a.latido.id.cmp(&b.latido.id)));
        entradas
            .into_iter()
            .map(|e| {
                let l = &e.latido;
                let sesion = l.claude_pid.and_then(|p| self.sesiones.get(&p)).or(l.sesion.as_ref());
                json!({
                    "id": l.id, "clase": l.clase, "pid": l.pid, "sesion": sesion, "claude_pid": l.claude_pid, "regla": l.regla,
                    "cliente": l.cliente, "directorio": l.directorio, "desde": e.desde,
                    "ultimo_latido_hace_s": ahora.saturating_duration_since(e.visto).as_secs(), "periodo": l.periodo,
                })
            })
            .collect()
    }

    /// Cuántos hay: en total, sesiones y agentes.
    pub fn cuenta(&mut self, ahora: Instant) -> Value {
        self.purgar(ahora);
        let sesiones = self.vivos.values().filter(|e| e.latido.clase == "sesion").count();
        json!({ "total": self.vivos.len(), "sesiones": sesiones, "agentes": self.vivos.len() - sesiones })
    }
}
