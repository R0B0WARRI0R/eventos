use crate::evento::Evento;
use rusqlite::{params, params_from_iter, Connection, ToSql};
use serde_json::Value;
use std::path::Path;

/// El almacén del bus: SQLite en modo WAL con búsqueda de texto completo. Solo lo abre el servicio.
pub struct Almacen {
    con: Connection,
}

/// Filtro de consulta. Sin `despues`, devuelve los últimos `limite` eventos; con él, los primeros tras ese `seq`.
#[derive(Debug, Default, Clone)]
pub struct Filtro {
    pub texto: Option<String>,
    pub tipo: Option<String>,
    /// Prefijo de la fuente.
    pub fuente: Option<String>,
    pub correlacion: Option<String>,
    pub causa: Option<String>,
    pub desde: Option<String>,
    pub hasta: Option<String>,
    pub despues: Option<i64>,
    pub limite: Option<usize>,
}

const ESQUEMA: &str = "
CREATE TABLE IF NOT EXISTS eventos (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE,
    fuente TEXT NOT NULL,
    tipo TEXT NOT NULL,
    t_evento TEXT,
    t_visto TEXT NOT NULL,
    gravedad TEXT,
    cuerpo TEXT,
    cuerpo_json INTEGER NOT NULL DEFAULT 0,
    atributos TEXT,
    correlacion TEXT,
    causa TEXT,
    profundidad INTEGER NOT NULL DEFAULT 0,
    confianza TEXT
);
CREATE INDEX IF NOT EXISTS eventos_tipo ON eventos(tipo);
CREATE INDEX IF NOT EXISTS eventos_t_visto ON eventos(t_visto);
CREATE INDEX IF NOT EXISTS eventos_correlacion ON eventos(correlacion);
CREATE INDEX IF NOT EXISTS eventos_causa ON eventos(causa);
CREATE VIRTUAL TABLE IF NOT EXISTS eventos_fts USING fts5(tipo, fuente, cuerpo);
";

const COLUMNAS: &str =
    "seq, id, fuente, tipo, t_evento, t_visto, gravedad, cuerpo, cuerpo_json, atributos, correlacion, causa, profundidad, confianza";

impl Almacen {
    pub fn abrir(ruta: &Path) -> rusqlite::Result<Self> {
        let con = Connection::open(ruta)?;
        con.pragma_update(None, "journal_mode", "WAL")?;
        con.pragma_update(None, "synchronous", "NORMAL")?;
        con.execute_batch(ESQUEMA)?;
        Ok(Almacen { con })
    }

    /// Guarda el evento (ya con id y t_visto) y le pone su `seq`.
    pub fn insertar(&self, e: &mut Evento) -> rusqlite::Result<()> {
        let (cuerpo, es_json) = match &e.cuerpo {
            Value::Null => (None, 0),
            Value::String(s) => (Some(s.clone()), 0),
            otro => (Some(otro.to_string()), 1),
        };
        let atributos = (!e.atributos.is_null()).then(|| e.atributos.to_string());
        let tx = self.con.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO eventos (id, fuente, tipo, t_evento, t_visto, gravedad, cuerpo, cuerpo_json, atributos, \
             correlacion, causa, profundidad, confianza) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![e.id, e.fuente, e.tipo, e.t_evento, e.t_visto, e.gravedad, cuerpo, es_json, atributos,
                e.correlacion, e.causa, e.profundidad, e.confianza],
        )?;
        let seq = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO eventos_fts (rowid, tipo, fuente, cuerpo) VALUES (?1, ?2, ?3, ?4)",
            params![seq, e.tipo, e.fuente, cuerpo.unwrap_or_default()],
        )?;
        tx.commit()?;
        e.seq = seq;
        Ok(())
    }

    pub fn consultar(&self, f: &Filtro) -> rusqlite::Result<Vec<Evento>> {
        let mut condiciones = String::new();
        let mut args: Vec<Box<dyn ToSql>> = Vec::new();
        if let Some(t) = f.texto.as_deref().filter(|t| !t.trim().is_empty()) {
            condiciones += " AND seq IN (SELECT rowid FROM eventos_fts WHERE eventos_fts MATCH ?)";
            args.push(Box::new(consulta_fts(t)));
        }
        for (columna, valor) in [("tipo", &f.tipo), ("correlacion", &f.correlacion), ("causa", &f.causa)] {
            if let Some(v) = valor {
                condiciones += &format!(" AND {columna} = ?");
                args.push(Box::new(v.clone()));
            }
        }
        if let Some(p) = &f.fuente {
            condiciones += " AND substr(fuente, 1, length(?)) = ?";
            args.push(Box::new(p.clone()));
            args.push(Box::new(p.clone()));
        }
        if let Some(d) = &f.desde {
            condiciones += " AND t_visto >= ?";
            args.push(Box::new(d.clone()));
        }
        if let Some(h) = &f.hasta {
            condiciones += " AND t_visto <= ?";
            args.push(Box::new(h.clone()));
        }
        if let Some(s) = f.despues {
            condiciones += " AND seq > ?";
            args.push(Box::new(s));
        }
        let limite = f.limite.unwrap_or(100).clamp(1, 1000) as i64;
        args.push(Box::new(limite));
        let sql = if f.despues.is_some() {
            format!("SELECT {COLUMNAS} FROM eventos WHERE 1=1{condiciones} ORDER BY seq ASC LIMIT ?")
        } else {
            format!("SELECT * FROM (SELECT {COLUMNAS} FROM eventos WHERE 1=1{condiciones} ORDER BY seq DESC LIMIT ?) ORDER BY seq ASC")
        };
        let mut stmt = self.con.prepare(&sql)?;
        let filas = stmt.query_map(params_from_iter(args.iter()), fila_a_evento)?;
        filas.collect()
    }

    /// Los últimos `limite` eventos de estos tipos llegados desde `desde`, en orden de llegada. Para el panel de
    /// tareas: así no arrastra las líneas de salida de los programas, que pueden ser miles.
    pub fn de_tipos(&self, tipos: &[&str], desde: &str, limite: usize) -> rusqlite::Result<Vec<Evento>> {
        let marcas = vec!["?"; tipos.len()].join(", ");
        let sql = format!(
            "SELECT * FROM (SELECT {COLUMNAS} FROM eventos WHERE t_visto >= ? AND tipo IN ({marcas}) \
             ORDER BY seq DESC LIMIT ?) ORDER BY seq ASC"
        );
        let mut args: Vec<Box<dyn ToSql>> = vec![Box::new(desde.to_string())];
        args.extend(tipos.iter().map(|t| Box::new(t.to_string()) as Box<dyn ToSql>));
        args.push(Box::new(limite as i64));
        let mut stmt = self.con.prepare(&sql)?;
        let filas = stmt.query_map(params_from_iter(args.iter()), fila_a_evento)?;
        filas.collect()
    }

    /// El árbol causal entero del evento: desde su antepasado más lejano que siga en el bus, todos sus descendientes.
    pub fn cadena(&self, id: &str) -> rusqlite::Result<Vec<Evento>> {
        let sql = format!(
            "WITH RECURSIVE
               arriba(id, causa) AS (
                 SELECT id, causa FROM eventos WHERE id = ?1
                 UNION SELECT e.id, e.causa FROM eventos e JOIN arriba a ON e.id = a.causa),
               raiz(id) AS (SELECT id FROM arriba WHERE causa IS NULL OR causa NOT IN (SELECT id FROM eventos)),
               abajo(id) AS (
                 SELECT id FROM raiz
                 UNION SELECT e.id FROM eventos e JOIN abajo b ON e.causa = b.id)
             SELECT {COLUMNAS} FROM eventos WHERE id IN (SELECT id FROM abajo) ORDER BY seq LIMIT 2000"
        );
        let mut stmt = self.con.prepare(&sql)?;
        let filas = stmt.query_map(params![id], fila_a_evento)?;
        filas.collect()
    }

    pub fn contar(&self) -> rusqlite::Result<i64> {
        self.con.query_row("SELECT count(*) FROM eventos", [], |r| r.get(0))
    }

    pub fn ultimo_seq(&self) -> rusqlite::Result<i64> {
        self.con.query_row("SELECT coalesce(max(seq), 0) FROM eventos", [], |r| r.get(0))
    }
}

/// Cada palabra como frase literal de FTS5: así los guiones, puntos o comillas del texto no son sintaxis.
fn consulta_fts(texto: &str) -> String {
    texto.split_whitespace().map(|p| format!("\"{}\"", p.replace('"', "\"\""))).collect::<Vec<_>>().join(" ")
}

fn fila_a_evento(r: &rusqlite::Row) -> rusqlite::Result<Evento> {
    let cuerpo: Option<String> = r.get(7)?;
    let es_json: i64 = r.get(8)?;
    let cuerpo = match cuerpo {
        None => Value::Null,
        Some(c) if es_json == 1 => match serde_json::from_str(&c) {
            Ok(v) => v,
            Err(_) => Value::String(c),
        },
        Some(c) => Value::String(c),
    };
    let atributos: Option<String> = r.get(9)?;
    let profundidad: i64 = r.get(12)?;
    Ok(Evento {
        seq: r.get(0)?,
        id: r.get(1)?,
        fuente: r.get(2)?,
        tipo: r.get(3)?,
        t_evento: r.get(4)?,
        t_visto: r.get(5)?,
        gravedad: r.get(6)?,
        cuerpo,
        atributos: atributos.and_then(|a| serde_json::from_str(&a).ok()).unwrap_or(Value::Null),
        correlacion: r.get(10)?,
        causa: r.get(11)?,
        profundidad: u32::try_from(profundidad).unwrap_or(0),
        confianza: r.get(13)?,
    })
}
