use regex::Regex;
use serde_json::Value;
use std::sync::OnceLock;

/// Decodifica una línea de un programa: UTF-8 si lo es; si no, la página de códigos OEM del sistema (cp850 en un
/// Windows en castellano), que es la que usan los programas de consola al escribir en una tubería.
pub fn decodificar(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => decodificar_con(bytes, PAGINA_OEM),
    }
}

/// Página «OEM del sistema» para `decodificar_con`.
pub const PAGINA_OEM: u32 = 1;

#[cfg(windows)]
pub fn decodificar_con(bytes: &[u8], pagina: u32) -> String {
    use windows_sys::Win32::Globalization::MultiByteToWideChar;
    if bytes.is_empty() {
        return String::new();
    }
    let largo = i32::try_from(bytes.len()).unwrap_or(i32::MAX);
    // SAFETY: punteros y longitudes salen de slices vivos; la primera llamada solo pide el tamaño.
    let n = unsafe { MultiByteToWideChar(pagina, 0, bytes.as_ptr(), largo, std::ptr::null_mut(), 0) };
    if n <= 0 {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let mut ancho = vec![0u16; n as usize];
    // SAFETY: `ancho` tiene exactamente `n` posiciones, las que pidió la primera llamada.
    let escritos = unsafe { MultiByteToWideChar(pagina, 0, bytes.as_ptr(), largo, ancho.as_mut_ptr(), n) };
    String::from_utf16_lossy(&ancho[..escritos.max(0) as usize])
}

#[cfg(not(windows))]
pub fn decodificar_con(bytes: &[u8], _pagina: u32) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn patrones() -> &'static [(Regex, &'static str)] {
    static P: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    P.get_or_init(|| {
        vec![
            (
                // La clave puede llevar prefijo o sufijo (GITHUB_TOKEN, DB_PASSWORD, AWS_SECRET_ACCESS_KEY) y venir
                // entre comillas como en JSON ("password":"x"). El esquema (Bearer, Basic...) va dentro de lo que se
                // tapa: si no, se taparía la palabra y quedaría la credencial.
                // También con las comillas escapadas (\"password\":\"x\"), que es como llega un JSON dentro de una
                // orden o de un log, y con comillas escapadas dentro del valor.
                Regex::new(
                    r#"(?i)\b([\w-]*(?:authorization|api[_-]?key|access[_-]?token|token|secret|password|passwd|contraseña|clave)[\w-]*)((?:\\?")?\s*[:=]\s*)(?:(?:bearer|basic|token|digest)\s+)?(\\"(?:[^"\\]|\\[^"])*\\"|"(?:[^"\\]|\\.)*"|'[^']*'|[^\s,;}]+)"#,
                )
                .expect("patrón de clave=valor"),
                "${1}${2}[REDACTADO]",
            ),
            (
                // Opciones de línea de órdenes con el valor separado por espacio: --password hunter2.
                Regex::new(r"(?i)(--?[\w-]*(?:password|passwd|token|secret|api[_-]?key)[\w-]*)(\s+)([^\s-]\S*)").expect("patrón de opciones"),
                "${1}${2}[REDACTADO]",
            ),
            (
                // Credenciales dentro de una URL: postgres://usuario:clave@servidor.
                Regex::new(r"(?i)\b([a-z][a-z0-9+.-]*://[^\s:/@]+:)([^\s@/]+)@").expect("patrón de URL"),
                "${1}[REDACTADO]@",
            ),
            (Regex::new(r"(?i)\bbearer\s+[A-Za-z0-9._~+/=-]{8,}").expect("patrón bearer"), "Bearer [REDACTADO]"),
            (Regex::new(r"(?i)\bbasic\s+[A-Za-z0-9+/=]{8,}").expect("patrón basic"), "Basic [REDACTADO]"),
            (
                Regex::new(r"\b(?:sk|sk_live|sk_test|ghp|gho|ghs|ghu|github_pat|xox[abps])[-_][A-Za-z0-9_-]{16,}").expect("patrón de prefijos"),
                "[REDACTADO]",
            ),
        ]
    })
}

/// Quita valores con pinta de secreto (clave=valor, Bearer, sk-...). Un bus que recoge cualquier cosa es también
/// una fuga universal: la redacción se hace al entrar, antes de guardar nada.
pub fn redactar(texto: &str) -> String {
    let mut salida = texto.to_string();
    for (re, por) in patrones() {
        salida = re.replace_all(&salida, *por).into_owned();
    }
    salida
}

/// Redacta un evento entero: fuente, cuerpo y atributos.
pub fn redactar_evento(e: &mut crate::evento::Evento) {
    e.fuente = redactar(&e.fuente);
    match &mut e.cuerpo {
        Value::String(s) => *s = redactar(s),
        otro => redactar_json(otro),
    }
    redactar_json(&mut e.atributos);
}

/// `redactar` sobre todas las cadenas de un JSON, también las claves de objetos con pinta de secreto.
pub fn redactar_json(valor: &mut Value) {
    match valor {
        Value::String(s) => *s = redactar(s),
        Value::Array(a) => a.iter_mut().for_each(redactar_json),
        Value::Object(m) => {
            for (clave, v) in m.iter_mut() {
                let k = clave.to_lowercase();
                let sensible = ["authorization", "api_key", "apikey", "api-key", "token", "secret", "password", "passwd", "clave", "contraseña"]
                    .iter()
                    .any(|s| k.contains(s));
                if sensible && !v.is_null() {
                    *v = Value::String("[REDACTADO]".into());
                } else {
                    redactar_json(v);
                }
            }
        }
        _ => {}
    }
}

/// Corta un texto a `max` caracteres, avisando del corte.
pub fn recortar(texto: &str, max: usize) -> String {
    if texto.chars().count() <= max {
        return texto.to_string();
    }
    let corto: String = texto.chars().take(max).collect();
    format!("{corto}… [recortado]")
}
