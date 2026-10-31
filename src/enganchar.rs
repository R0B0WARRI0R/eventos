//! `ev enganchar`: el proceso de los enganches. Fase 1: mira desde fuera las apps de `enganches.toml` (se abren, se
//! cierran, dejan de responder, se caen, cambian de título) y lo publica en el bus como cualquier fuente. Vive aparte de
//! `ev servir`: lo que se cuelgue aquí no arrastra al bus. Si el bus no responde, los eventos esperan en la cola en disco
//! y se vuelve a probar con espera creciente. Diseño en `docs/2026-10-30-diseno-enganches.md`.
//!
//! No inyecta nada en ninguna app: los avisos de ventanas son `SetWinEventHook` fuera de contexto, las caídas son una
//! suscripción al registro Application (que se lee sin administrador) y los cuelgues, `IsHungAppWindow`.

use crate::cliente::{Cliente, Envio};
use crate::evento::Evento;
use crate::Resultado;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

pub struct Opciones {
    pub config: PathBuf,
    /// Terminar tras estos segundos (para las pruebas); sin él, hasta que lo cierren.
    pub segundos: Option<u64>,
}

#[cfg(windows)]
pub fn ejecutar(o: &Opciones) -> Resultado<()> {
    win::ejecutar(o)
}

#[cfg(not(windows))]
pub fn ejecutar(_: &Opciones) -> Resultado<()> {
    Err("los enganches solo existen en Windows".into())
}

/// Cada cuánto se miran todas las ventanas (cuelgues y apps que se escaparan a los avisos).
const BARRIDO: Duration = Duration::from_secs(2);
const LOTE_ESPERA: Duration = Duration::from_millis(250);
const ESPERA_MAX: Duration = Duration::from_secs(60);

/// Envía los eventos en lotes. Si el bus no responde, van a la cola en disco y se vuelve a probar el bus tras 1 s,
/// 2, 4… hasta 60: un proceso que vive todo el día no puede quedarse escribiendo en la cola para siempre.
fn enviar_en_lotes(rx: Receiver<Evento>) {
    let cliente = Cliente::nuevo().con_espera(Duration::from_secs(2));
    let mut lote = Vec::new();
    let mut primero: Option<Instant> = None;
    let mut espera = Duration::from_secs(1);
    let mut reintento: Option<Instant> = None;
    loop {
        let plazo = primero.map_or(LOTE_ESPERA, |t| LOTE_ESPERA.saturating_sub(t.elapsed()));
        let fin = match rx.recv_timeout(plazo) {
            Ok(e) => {
                primero.get_or_insert_with(Instant::now);
                lote.push(e);
                false
            }
            Err(RecvTimeoutError::Timeout) => false,
            Err(RecvTimeoutError::Disconnected) => true,
        };
        let toca = fin || lote.len() >= 200 || primero.is_some_and(|t| t.elapsed() >= LOTE_ESPERA);
        if toca && !lote.is_empty() {
            if reintento.is_some_and(|t| Instant::now() < t) {
                if let Err(e) = cliente.encolar(&lote) {
                    eprintln!("ev enganchar: {} eventos no se pudieron encolar: {e}", lote.len());
                }
            } else {
                match cliente.publicar(&lote) {
                    Ok(Envio::Publicado(_)) => {
                        if reintento.take().is_some() {
                            eprintln!("ev enganchar: el bus vuelve a responder");
                        }
                        espera = Duration::from_secs(1);
                    }
                    Ok(Envio::EnCola { ruta, motivo }) => {
                        if reintento.is_none() {
                            eprintln!("ev enganchar: {motivo}; los eventos esperan en {}", ruta.display());
                        }
                        reintento = Some(Instant::now() + espera);
                        espera = (espera * 2).min(ESPERA_MAX);
                    }
                    Err(e) => eprintln!("ev enganchar: {} eventos no se pudieron ni enviar ni encolar: {e}", lote.len()),
                }
            }
            lote.clear();
            primero = None;
        }
        if fin {
            break;
        }
    }
}

#[cfg(windows)]
mod win {
    use super::{enviar_en_lotes, Opciones, BARRIDO};
    use crate::carpetas::Carpetas;
    use crate::enganches::{caida_desde_xml, cargar, orden_de_aviso, Aviso, Limitador, Vigia, CONSULTA_CAIDAS};
    use crate::evento::Evento;
    use crate::{puerto, Resultado};
    use std::cell::RefCell;
    use std::collections::{HashMap, HashSet};
    use std::ffi::c_void;
    use std::sync::mpsc::{self, Sender};
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, BOOL, ERROR_ALREADY_EXISTS, HANDLE, HWND, LPARAM, WAIT_OBJECT_0,
    };
    use windows_sys::Win32::System::EventLog::{
        EvtClose, EvtRender, EvtRenderEventXml, EvtSubscribe, EvtSubscribeActionDeliver, EvtSubscribeToFutureEvents,
        EVT_HANDLE, EVT_SUBSCRIBE_NOTIFY_ACTION,
    };
    use windows_sys::Win32::System::Threading::{
        CreateMutexW, GetCurrentProcessId, GetExitCodeProcess, OpenProcess, QueryFullProcessImageNameW,
        WaitForSingleObject, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    };
    use windows_sys::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, EnumWindows, GetAncestor, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
        IsHungAppWindow, IsWindowVisible, MsgWaitForMultipleObjects, PeekMessageW, TranslateMessage, CHILDID_SELF,
        EVENT_OBJECT_NAMECHANGE, EVENT_OBJECT_SHOW, GA_ROOT, MSG, OBJID_WINDOW, PM_REMOVE, QS_ALLINPUT,
        WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
    };

    thread_local! {
        /// Lo que dejan los avisos de ventanas (tipo y ventana); el bucle lo recoge después de bombear los mensajes.
        static AVISOS: RefCell<Vec<(u32, isize)>> = const { RefCell::new(Vec::new()) };
    }

    unsafe extern "system" fn al_aviso(_: HWINEVENTHOOK, tipo: u32, hwnd: HWND, objeto: i32, hijo: i32, _: u32, _: u32) {
        if !hwnd.is_null() && objeto == OBJID_WINDOW && hijo == CHILDID_SELF as i32 {
            AVISOS.with(|a| a.borrow_mut().push((tipo, hwnd as isize)));
        }
    }

    /// Lo llama el registro de Windows desde un hilo suyo con cada evento nuevo que casa con la consulta.
    unsafe extern "system" fn al_registro(accion: EVT_SUBSCRIBE_NOTIFY_ACTION, contexto: *const c_void, evento: EVT_HANDLE) -> u32 {
        if accion == EvtSubscribeActionDeliver && !contexto.is_null() {
            let tx = &*(contexto as *const Sender<String>);
            if let Some(xml) = xml_de(evento) {
                let _ = tx.send(xml);
            }
        }
        0
    }

    fn xml_de(evento: EVT_HANDLE) -> Option<String> {
        let (mut usado, mut propiedades) = (0u32, 0u32);
        unsafe { EvtRender(0, evento, EvtRenderEventXml, 0, std::ptr::null_mut(), &mut usado, &mut propiedades) };
        if usado == 0 {
            return None;
        }
        let mut buf = vec![0u16; usado as usize / 2 + 1];
        let bytes = (buf.len() * 2) as u32;
        if unsafe { EvtRender(0, evento, EvtRenderEventXml, bytes, buf.as_mut_ptr().cast(), &mut usado, &mut propiedades) } == 0 {
            return None;
        }
        let n = (usado as usize / 2).min(buf.len());
        Some(String::from_utf16_lossy(&buf[..n]).trim_end_matches('\0').to_string())
    }

    fn ancho(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    unsafe extern "system" fn juntar(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let v = &mut *(lparam as *mut Vec<isize>);
        v.push(hwnd as isize);
        1
    }

    fn ventanas() -> Vec<HWND> {
        let mut v: Vec<isize> = Vec::new();
        unsafe { EnumWindows(Some(juntar), &mut v as *mut Vec<isize> as LPARAM) };
        v.into_iter().map(|h| h as HWND).collect()
    }

    /// Ventana visible de nivel superior: las que tiene una app abierta (no menús ocultos ni controles hijos).
    fn principal(hwnd: HWND) -> bool {
        unsafe { IsWindowVisible(hwnd) != 0 && GetAncestor(hwnd, GA_ROOT) == hwnd }
    }

    fn pid_de(hwnd: HWND) -> u32 {
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        pid
    }

    fn titulo_de(hwnd: HWND) -> String {
        let largo = unsafe { GetWindowTextLengthW(hwnd) };
        if largo <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; largo as usize + 1];
        let n = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }

    /// Lanza la orden del aviso sin consola y sin esperarla: el bucle de ventanas no se para por ella.
    fn avisar(orden: &[String]) {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let Some((programa, args)) = orden.split_first() else { return };
        let lanzado = std::process::Command::new(programa)
            .args(args)
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        if let Err(e) = lanzado {
            eprintln!("ev enganchar: no puedo lanzar el aviso «{programa}»: {e}");
        }
    }

    fn ruta_de(h: HANDLE) -> Option<String> {
        let mut buf = vec![0u16; 1024];
        let mut n = buf.len() as u32;
        (unsafe { QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut n) } != 0)
            .then(|| String::from_utf16_lossy(&buf[..n as usize]))
    }

    struct Enganches {
        vigia: Vigia,
        limitador: Limitador,
        tx: Sender<Evento>,
        /// Procesos adoptados, con el asa que dice cuándo terminan.
        procesos: HashMap<u32, HANDLE>,
        propio: u32,
        aviso: Option<Aviso>,
    }

    impl Enganches {
        fn publicar(&mut self, eventos: Vec<Evento>, ahora: Instant) {
            for e in eventos {
                for salida in self.limitador.admitir(e, ahora) {
                    if let Some(orden) = self.aviso.as_ref().and_then(|a| orden_de_aviso(a, &salida)) {
                        avisar(&orden);
                    }
                    let _ = self.tx.send(salida);
                }
            }
        }

        /// Una ventana que puede ser de una app vigilada: si lo es y su proceso es nuevo, lo adopta.
        fn mirar(&mut self, hwnd: HWND, al_arrancar: bool, ahora: Instant) {
            if !principal(hwnd) {
                return;
            }
            let pid = pid_de(hwnd);
            if pid == 0 || pid == self.propio || self.vigia.vigila(pid) {
                return;
            }
            // Las apps que corren como administrador no se dejan abrir sin serlo: se quedan fuera.
            let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE, 0, pid) };
            if h.is_null() {
                return;
            }
            match ruta_de(h).filter(|r| self.vigia.app_de_exe(r).is_some()) {
                Some(ruta) => {
                    let eventos = self.vigia.ventana(pid, &ruta, al_arrancar);
                    self.procesos.insert(pid, h);
                    self.publicar(eventos, ahora);
                }
                None => unsafe {
                    CloseHandle(h);
                },
            }
        }

        fn titulo(&mut self, hwnd: HWND, ahora: Instant) {
            if !principal(hwnd) {
                return;
            }
            let pid = pid_de(hwnd);
            if self.vigia.vigila(pid) {
                self.vigia.titulo(pid, &titulo_de(hwnd), ahora);
            }
        }

        fn terminados(&mut self, ahora: Instant) {
            let mut fin = Vec::new();
            for (&pid, &h) in &self.procesos {
                if unsafe { WaitForSingleObject(h, 0) } == WAIT_OBJECT_0 {
                    let mut codigo = 0u32;
                    unsafe { GetExitCodeProcess(h, &mut codigo) };
                    fin.push((pid, h, codigo));
                }
            }
            for (pid, h, codigo) in fin {
                unsafe { CloseHandle(h) };
                self.procesos.remove(&pid);
                let eventos = self.vigia.termina(pid, codigo);
                self.publicar(eventos, ahora);
            }
        }

        /// Todas las ventanas: adopta las apps que no avisaron (o que ya estaban abiertas) y mira si están colgadas.
        fn barrer(&mut self, al_arrancar: bool, ahora: Instant) {
            let mut colgados: HashMap<u32, bool> = HashMap::new();
            let mut mirados = HashSet::new();
            for hwnd in ventanas() {
                if !principal(hwnd) {
                    continue;
                }
                let pid = pid_de(hwnd);
                if !self.vigia.vigila(pid) && mirados.insert(pid) {
                    self.mirar(hwnd, al_arrancar, ahora);
                }
                if self.vigia.vigila(pid) {
                    *colgados.entry(pid).or_default() |= unsafe { IsHungAppWindow(hwnd) } != 0;
                }
            }
            for pid in self.vigia.pids() {
                let eventos = self.vigia.respuesta(pid, colgados.get(&pid).copied().unwrap_or(false), ahora);
                self.publicar(eventos, ahora);
            }
        }
    }

    pub fn ejecutar(o: &Opciones) -> Resultado<()> {
        let config = cargar(&o.config)?;
        // Uno por bus: dos a la vez publicarían todo dos veces.
        let nombre = ancho(&format!("Local\\ev-enganchar-{}", puerto()));
        let mutex = unsafe { CreateMutexW(std::ptr::null(), 0, nombre.as_ptr()) };
        if mutex.is_null() || unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            return Err(format!("ya hay un `ev enganchar` en marcha para el bus del puerto {}", puerto()).into());
        }

        let (tx, rx) = mpsc::channel();
        let enviador = std::thread::spawn(move || enviar_en_lotes(rx));
        let mut e = Enganches {
            vigia: Vigia::new(&config),
            limitador: Limitador::new(&config),
            tx,
            procesos: HashMap::new(),
            propio: unsafe { GetCurrentProcessId() },
            aviso: config.aviso.clone(),
        };
        // Carpetas vigiladas: `notify` mira en sus hilos y el bucle de abajo recoge los ficheros nuevos.
        let mut carpetas = Carpetas::new(&config)?;
        let inicio = Instant::now();
        let tanteos = e.vigia.tanteos();
        e.publicar(tanteos, inicio);
        let tanteos = carpetas.tanteos();
        e.publicar(tanteos, inicio);

        // Caídas: suscripción al registro Application. El contexto (el emisor) vive mientras viva la suscripción.
        let (tx_caidas, rx_caidas) = mpsc::channel::<String>();
        let contexto = Box::into_raw(Box::new(tx_caidas));
        let (canal, consulta) = (ancho("Application"), ancho(CONSULTA_CAIDAS));
        let suscripcion = unsafe {
            EvtSubscribe(
                0,
                std::ptr::null_mut(),
                canal.as_ptr(),
                consulta.as_ptr(),
                0,
                contexto as *const c_void,
                Some(al_registro),
                EvtSubscribeToFutureEvents,
            )
        };
        if suscripcion == 0 {
            eprintln!("ev enganchar: no puedo suscribirme al registro Application (error {}); sigo sin avisos de caídas", unsafe {
                GetLastError()
            });
        }

        // Ventanas: avisos de Windows fuera de contexto (no se inyecta nada en las apps). Sin títulos, no se piden.
        let mut ganchos = vec![unsafe {
            SetWinEventHook(EVENT_OBJECT_SHOW, EVENT_OBJECT_SHOW, std::ptr::null_mut(), Some(al_aviso), 0, 0, WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS)
        }];
        if config.apps.iter().any(|a| a.titulo) {
            ganchos.push(unsafe {
                SetWinEventHook(EVENT_OBJECT_NAMECHANGE, EVENT_OBJECT_NAMECHANGE, std::ptr::null_mut(), Some(al_aviso), 0, 0, WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS)
            });
        }
        if ganchos.iter().any(|g| g.is_null()) {
            eprintln!("ev enganchar: Windows no aceptó los avisos de ventanas; las apps se verán cada {} s", BARRIDO.as_secs());
        }

        let nombres: Vec<&str> = config.apps.iter().map(|a| a.nombre.as_str()).chain(carpetas.rutas()).collect();
        eprintln!("ev enganchar: vigilo {} (bus en el puerto {})", nombres.join(", "), puerto());
        e.barrer(true, inicio);
        let mut proximo_barrido = inicio + BARRIDO;
        let fin = o.segundos.map(|s| inicio + Duration::from_secs(s));
        loop {
            unsafe {
                MsgWaitForMultipleObjects(0, std::ptr::null(), 0, 250, QS_ALLINPUT);
                let mut msg: MSG = std::mem::zeroed();
                while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            let ahora = Instant::now();
            for (tipo, hwnd) in AVISOS.with(|a| std::mem::take(&mut *a.borrow_mut())) {
                match tipo {
                    EVENT_OBJECT_SHOW => e.mirar(hwnd as HWND, false, ahora),
                    EVENT_OBJECT_NAMECHANGE => e.titulo(hwnd as HWND, ahora),
                    _ => {}
                }
            }
            while let Ok(xml) = rx_caidas.try_recv() {
                if let Some(c) = caida_desde_xml(&xml) {
                    let eventos = e.vigia.caida(&c);
                    e.publicar(eventos, ahora);
                }
            }
            e.terminados(ahora);
            if ahora >= proximo_barrido {
                e.barrer(false, ahora);
                proximo_barrido = ahora + BARRIDO;
            }
            let titulos = e.vigia.tick(ahora);
            e.publicar(titulos, ahora);
            if !carpetas.is_empty() {
                let ficheros = carpetas.recoger(ahora);
                e.publicar(ficheros, ahora);
            }
            if fin.is_some_and(|f| ahora >= f) {
                break;
            }
        }

        unsafe {
            for g in ganchos.into_iter().filter(|g| !g.is_null()) {
                UnhookWinEvent(g);
            }
            if suscripcion != 0 {
                EvtClose(suscripcion);
            }
            // Cerrada la suscripción ya no hay llamadas que usen el contexto.
            drop(Box::from_raw(contexto));
            for (_, h) in e.procesos.drain() {
                CloseHandle(h);
            }
            CloseHandle(mutex);
        }
        drop(e);
        let _ = enviador.join();
        Ok(())
    }
}
