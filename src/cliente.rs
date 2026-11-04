use crate::evento::Evento;
use crate::texto::redactar_evento;
use crate::{carpeta_datos, puerto, Resultado};
use serde_json::Value;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

/// Cliente HTTP del servicio. Si el bus no responde, los eventos esperan en una cola en disco que el servicio importa
/// al arrancar: quien produce eventos nunca se bloquea ni los pierde por eso.
pub struct Cliente {
    base: String,
    cola: PathBuf,
    espera: Duration,
}

/// Qué pasó con un envío.
#[derive(Debug)]
pub enum Envio {
    Publicado(Vec<String>),
    /// No llegó al bus (no responde o lo rechazó): está en la cola en disco, que el servicio importa; lo que entonces
    /// siga sin valer queda en `pendientes.rechazados.jsonl`. Nunca se tira.
    EnCola { ruta: PathBuf, motivo: String },
}

impl Default for Cliente {
    fn default() -> Self {
        Self::nuevo()
    }
}

impl Cliente {
    pub fn nuevo() -> Self {
        Cliente {
            base: format!("http://127.0.0.1:{}", puerto()),
            cola: carpeta_datos().join("pendientes.jsonl"),
            espera: Duration::from_secs(5),
        }
    }

    pub fn con_espera(mut self, espera: Duration) -> Self {
        self.espera = espera;
        self
    }

    pub fn publicar(&self, eventos: &[Evento]) -> Resultado<Envio> {
        let cuerpo = serde_json::to_value(eventos)?;
        let motivo = match ureq::post(&format!("{}/eventos", self.base)).timeout(self.espera).send_json(cuerpo) {
            Ok(r) => {
                let v: Value = r.into_json()?;
                let ids = v["ids"].as_array().map(|a| a.iter().filter_map(|i| i.as_str().map(str::to_string)).collect());
                return Ok(Envio::Publicado(ids.unwrap_or_default()));
            }
            Err(ureq::Error::Status(codigo, r)) => {
                format!("el bus rechazó el envío ({codigo}: {})", r.into_string().unwrap_or_default().trim())
            }
            Err(ureq::Error::Transport(t)) => format!("el bus no responde ({t})"),
        };
        self.encolar(eventos)?;
        Ok(Envio::EnCola { ruta: self.cola.clone(), motivo })
    }

    /// Añade los eventos a la cola en disco, ya redactados (la cola también es un fichero que puede leer cualquiera).
    pub fn encolar(&self, eventos: &[Evento]) -> Resultado<()> {
        if let Some(carpeta) = self.cola.parent() {
            std::fs::create_dir_all(carpeta)?;
        }
        let mut lineas = String::new();
        for e in eventos {
            let mut e = e.clone();
            redactar_evento(&mut e);
            lineas += &serde_json::to_string(&e)?;
            lineas.push('\n');
        }
        let mut f = OpenOptions::new().create(true).append(true).open(&self.cola)?;
        f.write_all(lineas.as_bytes())?;
        Ok(())
    }

    pub fn ruta_cola(&self) -> &PathBuf {
        &self.cola
    }

    pub fn consultar(&self, filtro: &[(&str, String)]) -> Resultado<Vec<Evento>> {
        self.get_eventos("/eventos", filtro, self.espera)
    }

    pub fn cadena(&self, id: &str) -> Resultado<Vec<Evento>> {
        self.get_eventos(&format!("/cadena/{id}"), &[], self.espera)
    }

    /// Espera hasta `segundos` a que llegue algún evento que cumpla el filtro (con `despues` como cursor).
    pub fn esperar(&self, filtro: &[(&str, String)], segundos: u64) -> Resultado<Vec<Evento>> {
        let mut f: Vec<(&str, String)> = filtro.to_vec();
        f.push(("segundos", segundos.to_string()));
        self.get_eventos("/esperar", &f, Duration::from_secs(segundos + 10))
    }

    pub fn estado(&self) -> Resultado<Value> {
        Ok(self.peticion(ureq::get(&format!("{}/estado", self.base)).timeout(self.espera))?)
    }

    pub fn fuentes(&self) -> Resultado<Value> {
        self.peticion(ureq::get(&format!("{}/fuentes", self.base)).timeout(self.espera))
    }

    pub fn orden(&self, ruta: &str) -> Resultado<Value> {
        Ok(self.peticion(ureq::post(&format!("{}{ruta}", self.base)).timeout(self.espera))?)
    }

    /// POST con un cuerpo JSON (el latido y la despedida de `ev mcp`).
    pub fn enviar(&self, ruta: &str, cuerpo: &Value) -> Resultado<Value> {
        self.respuesta(ureq::post(&format!("{}{ruta}", self.base)).timeout(self.espera).send_json(cuerpo))
    }

    fn get_eventos(&self, ruta: &str, filtro: &[(&str, String)], espera: Duration) -> Resultado<Vec<Evento>> {
        let mut peticion = ureq::get(&format!("{}{ruta}", self.base)).timeout(espera);
        for (k, v) in filtro {
            peticion = peticion.query(k, v);
        }
        let v = self.peticion(peticion)?;
        Ok(serde_json::from_value(v)?)
    }

    fn peticion(&self, p: ureq::Request) -> Resultado<Value> {
        self.respuesta(p.call())
    }

    fn respuesta(&self, r: Result<ureq::Response, ureq::Error>) -> Resultado<Value> {
        match r {
            Ok(r) => Ok(r.into_json()?),
            Err(ureq::Error::Status(codigo, r)) => {
                Err(format!("el bus respondió {codigo}: {}", r.into_string().unwrap_or_default()).into())
            }
            Err(ureq::Error::Transport(t)) => {
                Err(format!("el bus no responde en {} ({t}); ¿está en marcha `ev servir`?", self.base).into())
            }
        }
    }
}
