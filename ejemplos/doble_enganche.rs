//! Doble de prueba de los enganches (`tests/enganches.rs`): una ventana de verdad que hace lo que se le pide.
//!
//!   doble_enganche abrir         abre, cambia el título a «doble 2» y se cierra bien (código 0)
//!   doble_enganche colgarse N    abre, deja de atender mensajes N segundos, vuelve a responder y se cierra
//!   doble_enganche caerse        abre y escribe en la dirección 0: Windows anota la caída (Application Error 1000)

#[cfg(windows)]
fn main() {
    use std::time::{Duration, Instant};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, PeekMessageW, RegisterClassW, SetWindowTextW,
        TranslateMessage, MSG, PM_REMOVE, WNDCLASSW, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
    };

    fn ancho(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Atiende los mensajes durante un rato, como cualquier ventana sana.
    fn atender(durante: Duration) {
        let fin = Instant::now() + durante;
        while Instant::now() < fin {
            unsafe {
                let mut msg: MSG = std::mem::zeroed();
                while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    let args: Vec<String> = std::env::args().skip(1).collect();
    // Una clase propia con la ventana por defecto de Windows, como las apps de verdad. Las clases de control (Static)
    // cambian el título sin avisar a los ganchos (comprobado el 2026-10-31).
    let (clase, titulo) = (ancho("DobleEnganche"), ancho("doble 1"));
    let hwnd = unsafe {
        let mut wc: WNDCLASSW = std::mem::zeroed();
        wc.lpfnWndProc = Some(DefWindowProcW);
        wc.hInstance = GetModuleHandleW(std::ptr::null());
        wc.lpszClassName = clase.as_ptr();
        assert!(RegisterClassW(&wc) != 0, "no se pudo registrar la clase de ventana");
        CreateWindowExW(
            0,
            clase.as_ptr(),
            titulo.as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            40,
            40,
            320,
            120,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            wc.hInstance,
            std::ptr::null(),
        )
    };
    assert!(!hwnd.is_null(), "no se pudo crear la ventana");
    atender(Duration::from_millis(1500));
    match args.first().map(String::as_str) {
        Some("abrir") => {
            let t = ancho("doble 2");
            unsafe { SetWindowTextW(hwnd, t.as_ptr()) };
            atender(Duration::from_millis(1500));
        }
        Some("colgarse") => {
            let segundos: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(15);
            // Sin atender mensajes: para Windows, la ventana deja de responder a los 5 s.
            std::thread::sleep(Duration::from_secs(segundos));
            atender(Duration::from_secs(4));
        }
        Some("caerse") => unsafe {
            std::ptr::null_mut::<u8>().write_volatile(1);
        },
        _ => {
            eprintln!("uso: doble_enganche abrir | colgarse N | caerse");
            std::process::exit(2);
        }
    }
}

#[cfg(not(windows))]
fn main() {}
