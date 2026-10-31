# Enganches del bus `ev`: diseño

2026-10-30 · XANA, para Victoria. Documento de diseño, no código.

## Resumen (5 líneas)

1. Un **enganche** conecta al bus una app que no tiene salida propia; vive en un proceso aparte, **`ev enganchar`**, y publica como cualquier otra fuente (HTTP a `127.0.0.1:8870`, con caída a la cola `pendientes.jsonl`). El bus no se toca.
2. Cuatro capas, de menos a más íntimo: **Grupo 1 y 2 desde fuera** (ciclo de vida de la app, título, *tail* de registros, notificaciones de Windows, carpetas), **Grupo 3** (texto de ventana por UI Automation, solo esbozado) y **Modo Upgrade desde dentro** (CDP para apps Electron, Frida para el resto).
3. Todo es **reversible y opt-in**: ninguna app viene activada, cada una se declara en `enganches.toml`, y el Upgrade vive solo en memoria (ni un parche en disco). Rechazo el principio 7 (escalada de irreversibilidad): la trayectoria escala en especificidad, nunca en permanencia.
4. Cada enganche nace con su **antídoto** (orden de soltarse) y se suelta solo si la app se cuelga, se actualiza o inunda el bus. Una **verificación previa** por app (licencia, antitrampas, servicio en línea, hash/versión) es obligatoria antes de cualquier Upgrade.
5. No se reinventa el sobre: se reutilizan los campos reales de `Evento` (`fuente`, `tipo`, `gravedad`, `cuerpo`, `atributos`, `correlacion`, `causa`, `confianza`). Los enganches solo son fuentes nuevas, baratas, una por tipo de captura.

**Estado (2026-10-31): F1 hecha.** `ev enganchar` en `src/enganchar.rs` (Win32) y `src/enganches.rs` (piezas puras); `enganches.toml` con Constructo, Obsidian y Brave; 17 pruebas en `tests/enganches.rs` (3 de escritorio, `-- --ignored`) con el doble `ejemplos/doble_enganche.rs`. Instalado en `D:\Programas\BusEventos\ev.exe`. Lo que cambió al construirla está marcado «Cambio en F1» en §3.2, §3.4 y §9. Añadido a petición suya (2026-10-31): `[aviso]` en `enganches.toml`. Cuando una app deja de responder o se cae, sale la ventanita de avisos de XANA. Se probó de verdad: salió a los 4 s. **F2a (seguir ficheros de registro) hecha el 2026-10-31** en la rama `pulso/ev-tail`, sin fusionar todavía: §4.1.

---

## 1. Qué es un enganche y dónde vive

### 1.1 Decisión de arquitectura: proceso aparte `ev enganchar`

Los enganches **no viven dentro de `ev servir`**. Viven en un proceso separado, `ev enganchar --config D:\dev\eventos\enganches.toml`, que publica por el mismo camino que todas las fuentes: `POST /eventos` con el `Cliente` existente (`src/cliente.rs`), que ya resuelve la caída del bus escribiendo en `datos\pendientes.jsonl`.

Por qué aparte y no dentro del bus:

- **`ev servir` es el eslabón caro y siempre encendido.** Los enganches necesitan cosas que pueden colgarse o requieren su propio apartamento: un bombeo de mensajes Win32 (`SetWinEventHook`), un apartamento COM (UI Automation), un runtime de WebSocket (CDP) o de Frida. Meter eso en el bucle HTTP del bus arriesga el servicio entero por culpa de una sola app. Esto es la **geometría de correa** (principio 6): contacto pequeño en la punta, filamento largo, cuerpo (el bus) desacoplado.
- **Reutiliza la redacción y la cola gratis.** Todo lo que publica un enganche pasa por `ingerir` → `redactar_evento` → almacén, igual que un hook. Si el bus está caído, va a `pendientes.jsonl` y se importa al arrancar. El enganche **nunca bloquea la app** que observa. La cola en disco es la **elasticidad en serie** (principio 5): el elemento que cede entre la punta y el bus.
- **Decisión reversible.** Si un día conviene, un enganche concreto puede moverse dentro del bus sin cambiar el formato de evento. El contrato es el POST, no el proceso.

### 1.2 Supervisor + obreros (avidez, principio 3)

`ev enganchar` es un supervisor ligero. Dentro:

- Los observadores **desde fuera** (Grupos 1 y 2) son hilos baratos del propio supervisor: un hook de ventanas, una suscripción al registro de eventos, un vigía de ficheros. Son muchos enlaces débiles.
- Cada app en **Upgrade** (CDP/Frida) y cada app en **Grupo 3** (UIA) corre como **obrero aislado** (un hilo dedicado con su runtime; a futuro, un subproceso `ev enganchar --obrero <app>`). Así, si la sonda de WhatsApp se cae, no arrastra a la de Discord ni al supervisor. Se sueltan de uno en uno.

Esto realiza la **avidez frente a afinidad**: preferimos muchos agarres flojos y soltables a un único agarre fuerte.

### 1.3 Cómo publica

| Camino | Uso |
|---|---|
| `POST http://127.0.0.1:8870/eventos` (vía `Cliente`) | Normal. Lote cuando llegan ráfagas. |
| `datos\pendientes.jsonl` | Automático si el bus no responde (lo hace ya `Cliente::publicar`). |

El supervisor **no** abre puerto propio ni escucha nada: solo produce. Respeta la regla de seguridad del bus (solo `127.0.0.1`).

---

## 2. La forma del evento (sin inventar campos)

Se reutiliza `Evento` tal cual está en `src/evento.rs`. Convenciones:

- **`fuente`**: prefijo por tipo de captura, como ya hace el bus (`hook:claude/…`, `proc:…`). Para enganches:
  - `app:<slug>` (ciclo de vida y título), `fichero:<ruta>` (tail), `notif:<slug>` (notificaciones), `carpeta:<ruta>` (carpetas vigiladas), `uia:<slug>` (Grupo 3), `upgrade:<slug>` (dentro).
- **`correlacion`**: `app:<slug>:<arranque>`, donde `<arranque>` es un ULID que el enganche genera al detectar que la app abre. Así `app.inicio`, cada `app.titulo`, las notificaciones y, si lo hay, el Upgrade de **esa misma ejecución** quedan agrupados y se ven con `ev cadena`. Al cerrarse la app, el `<arranque>` muere.
- **`causa`**: el id del evento de ciclo de vida que abrió el enganche, cuando aplica (p. ej. `enganche.fijado` apunta a `app.inicio`).
- **`gravedad`**: `info` (título, notificación, línea de log normal), `aviso` (app sin respuesta, enganche soltado por carga), `error` (app caída). Son los tres valores que ya valida `suscripciones.rs`.
- **`confianza`**: `local` para lo que observa el propio enganche (hecho técnico del equipo); **`externa`** para el **contenido de terceros** que viaja dentro (el cuerpo de una notificación, una línea de log con texto de fuera, el texto de un mensaje capturado por Upgrade). Esto marca para las reglas qué es dato no confiable.
- **`cuerpo`** y **`atributos`**: el dato en bruto y sus metadatos (pid, exe, versión, ruta, código de salida…).

**No hace falta ningún campo nuevo.** Si en el futuro se quisiera un identificador de ventana (HWND) estable, iría en `atributos`, no en el sobre.

---

## 3. Grupo 1 — ciclo de vida y título, desde fuera

### 3.1 La app se abre / se cierra

**API elegida: `SetWinEventHook` (WinEvents), no sondeo.** Crate **`windows`** (oficial de Microsoft), módulo `Win32::UI::Accessibility`.

- Eventos `EVENT_OBJECT_CREATE` / `EVENT_OBJECT_DESTROY` (filtrando a ventanas de nivel superior) y `EVENT_SYSTEM_FOREGROUND` para saber qué app está delante. Enganche fuera de contexto (`WINEVENT_OUTOFCONTEXT`): no se inyecta ninguna DLL en las apps.
- Requiere un **hilo con bombeo de mensajes** (`GetMessage`/`DispatchMessage`). Es el motivo técnico de que los enganches vivan fuera del bus.
- Para la identidad del proceso (exe, ruta, versión): `GetWindowThreadProcessId` → `QueryFullProcessImageNameW` y `GetFileVersionInfoW`. Reconciliación de arranques cortos con `CreateToolhelp32Snapshot` (módulo `Win32::System::Diagnostics::ToolHelp`) o el crate **`sysinfo`** para el inventario inicial.

Por qué no las alternativas:

- **`sysinfo` por sondeo**: pierde procesos que viven menos que el intervalo y gasta CPU en un portátil con RAM justa. Rechazado como mecanismo principal; se usa solo para el censo inicial y para el exe/versión.
- **WMI `Win32_ProcessStartTrace`/`Win32_ProcessStopTrace`** (crate `wmi`): da el proceso exacto, pero es una traza ETW que **exige administrador** y añade coste. Rechazado por defecto; queda como opción si ella arranca `ev enganchar` elevado y quiere precisión de proceso en vez de ventana.

Tipos de evento: **`app.inicio`** / **`app.fin`** (no reutilizo `proceso.fin`, que es de las órdenes que lanza `ev run`; conviene distinguir «app que observo» de «proceso que yo lancé»). `app.fin` lleva en `atributos` si el cierre fue normal o por caída.

### 3.2 La app se cuelga

**Varias señales convergen (principio 2, con el matiz de XANA).** «Colgada» no es urgente: interrumpe sin prisa, así que exige **umbral**. Señales:

1. Ventana fantasma: `IsHungAppWindow(hwnd)` o `SendMessageTimeoutW(hwnd, WM_NULL, …, SMTO_ABORTIFHUNG|SMTO_BLOCK, timeout)` sin respuesta.
2. Registro parado: su *tail* (Grupo 2) no crece en N segundos, si la app tiene registro vigilado.
3. CPU quieta: uso ~0 % durante N segundos (`sysinfo`).

Regla: **2 de 3** (o las 3 si solo hay 2 disponibles) durante una ventana sostenida → **`app.sin_respuesta`**, gravedad `aviso`. Se desarma solo cuando la ventana vuelve a responder (`app.respuesta`).

**Cambio en F1 (XANA, 2026-10-31): la CPU quieta no es señal obligatoria.** Un bucle infinito en el hilo de la ventana la cuelga con la CPU al máximo; exigir CPU quieta lo dejaría sin aviso. El umbral queda como **persistencia de la señal 1**: `IsHungAppWindow` en todas las pasadas (cada 2 s) durante `colgada_segundos` (10 por defecto), además de los 5 s que ya espera Windows. Avisa una vez y otra al volver. Cuando exista el *tail* (F2), «registro parado» puede sumarse como confirmación.

### 3.3 La app cae (crash)

**API elegida: `EvtSubscribe` del registro de eventos de Windows, no sondeo.** Crate **`windows`**, módulo `Win32::System::EventLog`.

- Suscripción push (`EvtSubscribeToFutureEvents`) al canal **Application** con consulta XPath:
  `*[System[Provider[@Name='Application Error'] and (EventID=1000)]]` para los cuelgues duros (Id 1000). Opcionalmente Id 1002 (`Application Hang`) y el proveedor `.NET Runtime`.
- **Verificado en este equipo hoy**: el canal Application tiene 28.971 registros y se lee **sin elevación**. El canal Application es legible por `Users`: **no requiere administrador** (el canal Security sí lo requeriría, pero no lo usamos).
- **Corrección del 2026-10-30 (XANA): filtrar por proveedor, no solo por Id.** Los «4 eventos Id 1000 en 7 días» no eran cuelgues: los escribió `CodexSandboxService.OpenAI.Codex`, con mensajes informativos («The Codex sandbox service is running»). Con el proveedor `Application Error` hay 0 caídas en 7 días y 7 en 30: `Lector.exe`, `Sanctuary.exe`, `Sanctuary Shattered Sun.exe`, `RtkAudUService64.exe`, `ArmouryCrate.UserSessionHelper.exe`, `krita.exe` y `chrome-headless-shell.exe`. La consulta XPath de arriba ya filtra por proveedor y es correcta. Hace falta una prueba que lo fije: un Id 1000 de otro proveedor no puede dar `app.caida`.

Tipo de evento: **`app.caida`**, gravedad `error`. En `atributos`, el exe y la firma del fallo que da el registro. Esto cruza con la app vigilada por `app:<slug>` para cerrar su `<arranque>`.

Una sola señal basta aquí: una caída es **urgente** y no necesita umbral (principio 2, matiz de XANA: una señal para lo urgente).

### 3.4 El título de la ventana cambia

**API elegida: `SetWinEventHook` con `EVENT_OBJECT_NAMECHANGE`** sobre la ventana de nivel superior (mismo hook del 3.1, otro filtro). Nada de sondear `GetWindowTextW` en bucle.

Tipo: **`app.titulo`**, gravedad `info`, cuerpo = título nuevo. **Deduplicación y antirrebote obligatorios** (ver §8): los títulos parpadean (progreso, relojes), así que se descarta el título idéntico consecutivo y se agrupa con un respiro de ~500 ms.

**Cambio en F1:** `confianza: externa`, no `local`: el título lo pone la app y a menudo viene de fuera (el de una página web en Brave). Está apagado por defecto (`titulo = false`) en las tres apps.

---

## 4. Grupo 2 — registros, notificaciones y carpetas, desde fuera

### 4.1 Seguir ficheros de registro (tail)

**Crate elegido: `notify` v6** (usa `ReadDirectoryChangesW` por debajo), no sondeo de `stat`.

- `notify` avisa de que el fichero creció; el enganche lee desde el **desplazamiento guardado** hasta el final y emite una línea por evento.
- **Rotación**: se guarda, además del desplazamiento, el tamaño y una huella del arranque del fichero; si el fichero encoge o cambia la huella, se reabre desde cero (como recomienda la investigación §1). Decodificación: UTF-8 por defecto, con caída a cp850/OEM como ya hace `ev run` (la app puede escribir en la página OEM).

Tipo: **`registro.linea`**, fuente `fichero:<ruta>`, `confianza` según el origen de la app (una línea de log puede traer texto de terceros → `externa`). Un tope de líneas por segundo por fichero evita que un log ruidoso inunde el bus (§8).

**Hecho el 2026-10-31 (F2a, rama `pulso/ev-tail`)** en `src/tail.rs`, con 20 pruebas en `tests/tail.rs`. Se usa de dos formas: `ev tail <ruta>`, sin configuración, que es lo que XANA lanza en segundo plano mientras trabaja en una app, y las entradas `[[fichero]]` de `enganches.toml`, que sigue un hilo de `ev enganchar`. Lo que cambió al construirlo:
- **Huella de dos trozos.** Se comparan los primeros 64 bytes y también los 64 de justo antes de donde se iba. Muchas apps empiezan su registro siempre con la misma cabecera, y con solo el principio no se notaría que lo han vuelto a empezar.
- **El sondeo no es opcional.** Medido en este equipo: con la app manteniendo el fichero abierto y vaciando solo su búfer, `ReadDirectoryChangesW` no avisó en 20 s, tres veces seguidas. Por eso se miran todos los ficheros cada segundo, y el tamaño se pide al fichero abierto, no a la carpeta.
- **No se mantiene abierto.** Se abre en cada pasada, compartido, y se cierra: si se tuviera abierto, la app no podría rotarlo y crear otro con el mismo nombre.
- **Eventos de apoyo:** `registro.inicio` (desde dónde se lee; sirve también para saber que ya está siguiendo), `registro.rotado` (con su `motivo`), `registro.tope` (cuántas líneas se descartaron y cuáles) y `registro.error` (una vez por error distinto).
- **En `ev enganchar`**, una línea con `app` de una app abierta va con la correlación de su arranque. El `Limitador` no trata dos líneas iguales como una repetición.
- **Por defecto** empieza tras la última línea completa, con un tope de 20 líneas por segundo. La ruta tiene que ser absoluta y sin comodines (aún no se admiten). No guarda por dónde iba entre un arranque y otro.

### 4.2 Notificaciones de Windows de apps elegidas

**Fuente: `%LOCALAPPDATA%\Microsoft\Windows\Notifications\wpndatabase.db` (SQLite). Crate: `rusqlite` (ya es dependencia del bus).**

- **Verificado hoy** (sin leer el contenido, solo metadatos del fichero): existe, 1 MiB, **en modo WAL** (hay `wpndatabase.db-wal` y `-shm` vivos) y se actualiza al minuto. Es una base **viva y bloqueada** por el servicio de notificaciones de Windows (WpnUserService).
- **Cómo leerla sin bloquearla ni corromperla** (decisión):
  1. **Copia atómica** de los tres ficheros (`.db`, `-wal`, `-shm`) a una carpeta temporal del propio bus, y se abre **la copia** con `PRAGMA query_only` / `?mode=ro`. Nunca se abre la original en modo que pueda tomar un bloqueo de escritor ni forzar un *checkpoint*.
  2. Se leen las filas de la tabla `Notification` con **`Id` mayor que el último visto** (cursor), se publican y se **borra la copia**.
  3. El disparo de la copia lo marca `notify` sobre `-wal` (cambió) o un sondeo lento (p. ej. cada 3–5 s). Esquema de la tabla: por **documentación pública** del formato WNS/`wpndatabase` (no se inspecciona el contenido real de Victoria para diseñarlo).
- **Nunca** se escribe en la base original, **nunca** se mantiene abierta, **nunca** se hace *checkpoint*.

Tipo: **`notificacion.windows`**, fuente `notif:<slug>`, gravedad `info`, **`confianza: "externa"`** (las notificaciones traen mensajes de terceros).

**Redacción y conservación de notificaciones** (restricción no negociable):
- Pasan por la redacción de secretos del bus al entrar (ya existe), igual que todo.
- `enganches.toml` permite por app el modo **`solo_metadatos`** (guardar app, título corto y hora, no el texto del cuerpo) frente a **`texto`** (cuerpo recortado a N caracteres). Por defecto, **`solo_metadatos`**: es lo más conservador para mensajes privados.
- **Retención corta** configurable para esta fuente (p. ej. purgar `notif:*` a los X días), porque es lo más sensible que recoge el sistema.
- Son **dato, nunca instrucciones**: el bus ya mete el cuerpo en `<evento>…</evento>` como dato; `confianza: externa` lo refuerza para las reglas.

### 4.3 Carpetas vigiladas (ficheros nuevos)

**Crate: `notify` v6** (mismo que el tail). Observa creación/renombrado; antirrebote para los programas que escriben en varios pasos (crear → escribir → renombrar).

Tipo: **`carpeta.fichero`**, fuente `carpeta:<ruta>`, gravedad `info`. Cuerpo = ruta relativa y tamaño; **no** se lee el contenido del fichero (eso sería otra fuente, explícita).

---

## 5. Grupo 3 — texto de ventana por UI Automation (solo esbozo)

Para apps gráficas **sin ningún texto accesible de otra forma**, como un lector de pantalla, app por app.

- **Crate: `uiautomation`** (envoltorio Rust puro sobre la API COM `UIAutomation` de Windows), en un **hilo STA** propio (obrero aislado, §1.2).
- Se suscribe a `AutomationPropertyChangedEvent` (p. ej. `Name`, `Value`) y `StructureChangedEvent` sobre el árbol de la app, con `CacheRequest` para no machacar la app a llamadas COM.
- Lee texto con `TextPattern` / `ValuePattern` del elemento que cambió.
- Tipo: **`ventana.texto`**, fuente `uia:<slug>`, `confianza: externa`.

Es **caro y ruidoso** (la investigación lo marca como último recurso): opt-in explícito por app, con lista de elementos de interés y un tope de eventos agresivo. **No se implementa en las primeras fases**; queda esbozado. No requiere administrador (apps del mismo nivel de integridad).

---

## 6. Modo Upgrade — desde dentro, app por app

Solo con las apps que Victoria elija, y **solo tras la verificación previa** (§7). Lo que hace la app llega al bus como eventos; **actuar dentro de la app solo cuando un encargo lo pida** (principio 10: el ancla va fusionada con la operación siguiente, pero observar es lo primero).

### 6.1 Apps web (Electron): CDP, sin inyectar nada

WhatsApp, Discord, VS Code y demás Electron.

- **Mecanismo**: se **relanza** la app con `--remote-debugging-port=<puerto>` (un flag de arranque, nada en disco, totalmente reversible) y el enganche **se conecta al CDP** existente. Si la app ya está abierta sin el flag, no se puede adjuntar: hay que reabrirla (se publica `enganche.requiere_reinicio`).
- **Crate**: cliente WebSocket ligero con **`tungstenite`** + `serde_json` hablando CDP crudo (se listan los *targets* con `GET http://127.0.0.1:<puerto>/json`). Alternativa de más alto nivel: **`chromiumoxide`**; se prefiere `tungstenite` por dependencias mínimas y porque solo se necesitan unos pocos dominios.
- **Observación (por defecto, sin inyección)**: `Log.entryAdded`, `Runtime.consoleAPICalled`, `Network.requestWillBeSent`/`responseReceived`, `Page.*`. Son hechos que la app ya emite; no se ejecuta JavaScript propio.
- **Actuar (solo con encargo)**: `Runtime.evaluate` / `Input.*`, y siempre por las **interfaces de la propia app** (es lo que exige el criterio legal: para actuar dentro se prefieren accesibilidad/CDP/API, no modificar el binario).

Tipo: **`upgrade:<slug>`** como fuente; tipo por app, p. ej. `upgrade.whatsapp.mensaje`, `upgrade.discord.evento`, `upgrade.vscode.diagnostico`. `confianza: externa` para el contenido.

Esto reutiliza lo que Victoria ya tiene: REA sabe capturar Electron (`mcp__rea__capture_electron_scenario`, `inspect_electron_page`, `observe_web_session`) y puede servir de referencia para qué dominios CDP interesan.

### 6.2 El resto: Frida, mínimo y sin bloquear

Para apps nativas sin CDP ni API útil.

- **Motor: REA con Frida, que Victoria ya tiene en local.** El enganche **no** se mete a embeber enlaces de Frida: usa las herramientas `mcp__rea__start_frida_session`, `mcp__rea__instrument_with_frida`, `mcp__rea__load_frida_script`, `mcp__rea__observe_native_calls` y las cierra con `mcp__rea__close_frida_session`. (Alternativa si se quisiera autónomo: el crate **`frida`**, enlaces Rust de frida-core; se deja como respaldo por ser más pesado.)
- **La sonda es mínima y nunca bloquea** (principio: antídoto y geometría de correa): el script de Frida solo **captura y manda** (`send({...})`) en los puntos de interés; **todo el trabajo pesado se hace fuera**, en el bus. Nada de lógica costosa dentro del proceso observado.
- El bridge del enganche lee el flujo de mensajes de Frida y publica **`upgrade:<slug>`** con el tipo por app.
- **En memoria, reversible**: Frida inyecta en memoria y se desadjunta; **no se parchea el binario en disco**. Esto cumple el rechazo del principio 7.

### 6.3 Ciclo de vida del enganche (contraste con la propuesta de XANA)

La propuesta de XANA es correcta; la adopto con dos precisiones:

```
búsqueda → contacto reversible (tanteo fuera) → verificación (§7)
→ compromiso (Upgrade) → disipación de carga → liberación (antídoto)
```

- **`enganche.tanteo`** (fuera): se observa desde Grupos 1/2 primero. Barato y soltable.
- **`enganche.verificacion`**: ver §7. Se publica el resultado **siempre**.
- **`enganche.fijado`**: Upgrade activo (CDP/Frida). `causa` = el `app.inicio` de ese arranque.
- **`enganche.soltado`**: antídoto disparado; `atributos.motivo` ∈ {`manual`, `caida`, `actualizacion`, `inundacion`, `app.fin`}.

**Precisión 1 (mía, sobre la de XANA):** la verificación de versión/hash **se repite antes de cada sesión de Upgrade**, no una sola vez, porque la app puede haberse actualizado entre arranques. Si el hash/versión del exe no casa con el verificado → **no se fija**, se vuelve a fuera y se publica `enganche.verificacion` con el desajuste.

**Precisión 2:** «si la app se actualiza, volver solo al modo de fuera» se implementa atando el antídoto a dos señales: `app.caida` del proceso en Upgrade **y** desajuste de hash/versión en el siguiente arranque. Las dos sueltan a `fuera` automáticamente.

---

## 7. Verificación previa por app (obligatoria antes del Upgrade)

Una función `verificar(app)` que **registra y decide**, y publica `enganche.verificacion`. Debe comprobar y dejar constancia de, al menos:

| Punto | Qué registra | Efecto |
|---|---|---|
| **Licencia** | Qué permite la licencia de la app sobre observar y sobre **cambiar comportamiento** (LPI art. 100.1). | Observar (100.3) siempre permitido. Actuar dentro, solo por interfaces propias y dentro de lo que permita la licencia. |
| **Antitrampas** | Si la app tiene anticheat (EAC, BattlEye, nProtect, Vanguard, etc.). | **Si lo tiene, Upgrade PROHIBIDO.** No se tantea siquiera desde dentro. |
| **Servicio en línea** | Si tocar la app en marcha incumpliría las condiciones del servicio en línea. | No se incumplen; TPM/DRM intactas (art. 196.4). |
| **Identidad** | Hash + versión del exe (superficie vs receptor, principio 8). | Debe casar con lo verificado o no se fija (§6.3). |

- **Riot Games: excluido por completo, en todo modo.** Ni fuera ni dentro. Nada de su ecosistema (cliente, juegos, servicios). Prevalece sobre cualquier regla general.
- **Juegos con antitrampas: nunca**, por el riesgo para su cuenta.
- **World of Warcraft**: no hay nada que pulse teclas (pendiente el permiso que ha pedido a Blizzard). **Sí** está bien seguir por *tail* (§4.1) el **registro de combate** que el juego escribe a disco: eso es Grupo 2 desde fuera, lectura de un fichero que el juego genera.
- **Marco legal como requisito de diseño, no asesoría**: observar y estudiar en uso (100.3) y descompilar lo indispensable para interoperar (100.5-6; Directiva 2009/24 art. 8) valen aunque la licencia diga lo contrario, porque el fin es **accesibilidad** (el bus y XANA son su tecnología de apoyo). Pero aquí **no se descompila nada**: se prefieren las interfaces de la app. Todo en su PC, sin distribuir.

Mientras la verificación no esté hecha y en verde, `enganche.fijado` no puede ocurrir. Es la **fase de verificación** del compromiso en dos fases (principio 1).

---

## 8. Límites

Se calcan los patrones que el bus ya tiene (`suscripciones.rs`, `reglas.rs`): ventana deslizante por hora, tope global, recorte de cuerpo.

- **Tope de eventos**:
  - Por enganche: `por_hora` en ventana deslizante (como las suscripciones) y un `por_segundo` para ráfagas (tail ruidoso, parpadeo de títulos). Al llenarse: se **cuenta lo que se calla** y se publica un `enganche.tope` una vez, igual que las suscripciones avisan del tope.
  - Global del supervisor: tope de eventos/segundo agregado; si se supera, los enganches menos prioritarios ceden primero.
- **Deduplicación**: se descarta el evento idéntico consecutivo (misma `fuente`+`tipo`+`cuerpo`) dentro de una ventana corta. Imprescindible para `app.titulo` y para notificaciones que Windows reescribe.
- **Antirrebote (debounce)**: títulos y cambios de carpeta se agrupan con ~300–500 ms de respiro.
- **Espera con retroceso**: reconexión a CDP/Frida/bus con retroceso exponencial y tope (p. ej. 1 s → 2 → 4 … hasta 60 s). Nada de reintentos en bucle cerrado.
- **Si el bus está caído**: `Cliente::publicar` ya escribe en `pendientes.jsonl`; el enganche sigue observando y encolando, sin bloquear la app. Se prefiere duplicar antes que perder (como el resto del bus).
- **La carga como señal (principio 4, adaptado)**: la «tensión» que lee el enganche es su propio contador por hora + la presión de la cola (`EnCola` repetido = bus saturado). Si un enganche en Upgrade supera su tope sostenidamente → **antídoto por `inundacion`**: se suelta y vuelve a fuera. El ancla se calibra con la carga.

---

## 9. Pruebas (sin apps reales)

- **Doble de proceso que se cuelga a propósito**: un `.exe`/script de prueba que (a) abre una ventana, (b) deja de escribir en su log, (c) deja la CPU a cero y no bombea mensajes → dispara las 3 señales de §3.2. Comprueba `app.sin_respuesta` por umbral. Variante que `abort()` para provocar un Id 1000 y comprobar `app.caida`.
- **Tail**: fichero de prueba al que un test añade líneas (incluida una en cp850) y una rotación (truncar/reabrir). Verifica orden, codificación y reanudación por desplazamiento.
- **Notificaciones**: **NO se toca la base real**. Se crea una **SQLite de fixture** con el esquema público de `Notification`, se le insertan filas y se comprueba el cursor por `Id`, la redacción, `confianza: externa` y el modo `solo_metadatos`. La copia-y-lee-copia se prueba contra esa fixture en WAL.
- **Carpetas**: carpeta temporal; el test deja caer un fichero (creado en varios pasos) y comprueba un solo `carpeta.fichero` tras el antirrebote.
- **CDP**: una **app Electron mínima** (unas líneas) lanzada con `--remote-debugging-port`; el test provoca un `console.log` y una petición de red y comprueba `upgrade.*`. Sirve de banco para el bridge sin tocar WhatsApp/Discord.
- **Frida**: un `.exe` nativo de prueba con una función conocida; se instrumenta vía REA y se comprueba que el `send()` llega como evento. (Marcado `#[ignore]` si Frida no está disponible en CI local.)
- **WinEvents / EvtSubscribe**: integración opcional (lanzar `notepad`, `New-WinEvent` en un canal de prueba); se aíslan tras una bandera porque necesitan escritorio real.
- **Supervisor**: tests de los límites (tope por hora, dedup, antirrebote, antídoto por inundación) como piezas puras, igual que `suscripciones.rs` prueba su tope sin red.

Convención del repo: integración en `tests/` contra un bus de verdad en puerto libre y carpeta propia; piezas sueltas aparte.

**Cambio en F1, dos trampas del doble (comprobadas el 2026-10-31):**
- `cargo test` pasa a sus procesos hijos un modo de errores que apaga el Informe de errores de Windows. El doble se caía (`0xC0000005`), pero el registro no lo anotaba. Lanzado a mano, sí lo anotaba. Por eso las pruebas lanzan el doble con `CREATE_DEFAULT_ERROR_MODE`.
- Una ventana de una clase de control («Static») cambia el título sin avisar a los ganchos (`EVENT_OBJECT_NAMECHANGE`). El doble registra su propia clase con la ventana por defecto de Windows, como las apps de verdad.

---

## 10. Configuración por app: `enganches.toml`

Nada activado por defecto. Una entrada por app, opt-in. Borrador:

```toml
# D:\dev\eventos\enganches.toml
# Cargado por: ev enganchar --config D:\dev\eventos\enganches.toml

por_segundo_global = 50          # tope agregado del supervisor

[[app]]
slug   = "obsidian"
nombre = "Obsidian"
coincide_exe = "Obsidian.exe"    # identidad de superficie (genérica)
modo   = "fuera"                 # fuera | upgrade
titulo = true                    # app.titulo
tail   = ['D:\Notas-Obsidian\.obsidian\logs\*.log']
carpetas = ['D:\Notas-Obsidian\Secretario\Sesiones']
por_hora = 120

[[app]]
slug   = "whatsapp"
nombre = "WhatsApp"
coincide_exe = "WhatsApp.exe"
modo   = "upgrade"
notificaciones = { activo = true, guardar = "solo_metadatos", retencion_dias = 3 }
[app.upgrade]
tecnologia = "cdp"               # cdp (Electron) | frida
puerto     = 9333
observar   = ["log", "consola", "red"]   # nunca inyecta JS salvo encargo
[app.upgrade.verificacion]
hash_sha256 = "…"                # receptor concreto; se revalida cada arranque
version     = "2.x"
antitrampas = "ninguno"
licencia    = "revisada: observar OK; actuar solo por interfaz propia"
servicio_en_linea = "no incumplir condiciones"

[[app]]
slug   = "wow"
nombre = "World of Warcraft"
coincide_exe = "Wow.exe"
modo   = "fuera"                 # Upgrade PROHIBIDO (antitrampas); solo tail del log de combate
tail   = ['D:\...\Logs\WoWCombatLog*.txt']
```

El cargador valida como `reglas.rs`: `deny_unknown_fields`, slugs únicos, y **rechaza** `modo = "upgrade"` si la verificación no está completa o si `antitrampas != "ninguno"`.

---

## 11. Nombres de los tipos de evento (resumen)

| Grupo | Tipo | Gravedad | `fuente` | `confianza` |
|---|---|---|---|---|
| 1 | `app.inicio` / `app.fin` | info | `app:<slug>` | local |
| 1 | `app.titulo` | info | `app:<slug>` | externa |
| 1 | `app.sin_respuesta` / `app.respuesta` | aviso / info | `app:<slug>` | local |
| 1 | `app.caida` | error | `app:<slug>` | local |
| 2 | `registro.linea` | info | `fichero:<ruta>` | externa |
| 2 | `registro.inicio` / `registro.rotado` | info | `fichero:<ruta>` | local |
| 2 | `registro.tope` / `registro.error` | aviso | `fichero:<ruta>` | local |
| 2 | `notificacion.windows` | info | `notif:<slug>` | externa |
| 2 | `carpeta.fichero` | info | `carpeta:<ruta>` | local |
| 3 | `ventana.texto` | info | `uia:<slug>` | externa |
| Upgrade | `upgrade.<slug>.<accion>` | según app | `upgrade:<slug>` | externa |
| Ciclo | `enganche.tanteo` / `.verificacion` / `.fijado` / `.soltado` / `.tope` / `.requiere_reinicio` | info/aviso | `enganche:<slug>` | local |

---

## 12. Permisos de administrador (resumen)

| Fuente | ¿Admin? |
|---|---|
| `SetWinEventHook` (abrir/cerrar/título) | **No** (misma integridad; no ve apps elevadas si el enganche no lo está). |
| `EvtSubscribe` al canal Application (caídas) | **No** (verificado hoy: leído sin elevación). |
| `IsHungAppWindow` / CPU (colgada) | No. |
| WMI `Win32_ProcessStartTrace` (opción de precisión) | **Sí** (ETW). Por eso es opcional, no el mecanismo por defecto. |
| `notify` (tail, carpetas) | No. |
| `rusqlite` sobre copia de `wpndatabase.db` | No (está en su `LOCALAPPDATA`). |
| UI Automation (Grupo 3) | No (misma integridad). |
| CDP (Electron) | No (flag de arranque). |
| Frida | Normalmente no para procesos del mismo usuario; **sí** puede necesitarlo para apps de mayor integridad. |

Si Victoria quiere observar apps que corren como administrador (p. ej. algún servicio), tendría que arrancar `ev enganchar` elevado. Por defecto, sin elevar.

---

## 13. Principios de anclaje biológico: qué adopto, adapto y rechazo

| # | Principio | Decisión | Porqué |
|---|---|---|---|
| 1 | Compromiso en dos fases | **Adopto** | Fuera (tanteo, barato) → verificación → Upgrade (compromiso). Es el ciclo de vida del enganche. |
| 2 | Umbral de confirmaciones | **Adopto, acotado** | Varias señales para lo que interrumpe sin prisa (`app.sin_respuesta` = 2 de 3); una sola para lo urgente (`app.caida`). Es la precisión de XANA, y es correcta. |
| 3 | Avidez frente a afinidad | **Adopto** | Muchos observadores débiles desde fuera; un obrero por app en Upgrade, soltables de uno en uno. |
| 4 | La carga como señal | **Adapto** | No hay «tensión» mecánica; su equivalente es el contador por hora + la presión de la cola. La carga dispara el antídoto por inundación. |
| 5 | Elasticidad en serie | **Adopto** | La cola `pendientes.jsonl` y los canales acotados ceden entre la punta y el bus; la sonda nunca bloquea la app. |
| 6 | Geometría de correa | **Adopto** | Contacto mínimo en la app, filamento largo (`ev enganchar`), cuerpo (el bus) desacoplado. Justifica el proceso aparte. |
| 7 | Escalada de irreversibilidad | **Rechazo** | Restricción no negociable y acierto de XANA: **todo reversible**. El Upgrade vive en memoria, no se parchea nada en disco. La trayectoria escala en **especificidad**, no en permanencia. |
| 8 | Superficie frente a receptor | **Adopto** | Fuera agarra la **superficie** (cualquier app: ventana, log, notificación); dentro agarra al **receptor** (este exe, este hash, esta versión). La verificación los separa. |
| 9 | Antídoto incorporado | **Adopto** | Cada enganche nace con su `enganche.soltado`; se diseña la liberación a la vez que la fijación. Suelta solo por caída, actualización o inundación. |
| 10 | El ancla nunca es el fin | **Adopto** | El enganche alimenta al bus y a las reglas (observar); actuar dentro solo cuando un encargo lo pida. El enganche va fusionado con lo siguiente, no es el objetivo. |

**La sofisticación está en la trayectoria** (empezar reversible y genérico, acabar específico), no en el extremo de ningún eje. Por eso la fuerza del agarre nunca es el valor a maximizar; lo es la capacidad de soltar.

---

## 14. Fases y estimación (días de trabajo efectivo)

| Fase | Alcance | Días |
|---|---|---|
| **F1 — Grupo 1** (hecha 2026-10-31) | `ev enganchar` (supervisor + reutilización de `Cliente`), `SetWinEventHook` (abrir/cerrar/título), `EvtSubscribe` (caídas), detección de colgada por umbral, `enganches.toml` mínimo, límites y dedup. Doble de proceso que se cuelga. | 4–5 |
| **F2 — Grupo 2** | Tail (`notify` + desplazamiento + rotación + cp850), notificaciones (copia-y-lee-copia de `wpndatabase.db`, `solo_metadatos`, retención), carpetas. Fixtures de prueba. | 3–4 |
| **F3 — Verificación + Upgrade CDP** | Función `verificar` (licencia/antitrampas/servicio/hash), ciclo de vida completo, bridge CDP (`tungstenite`) para Electron, app Electron mínima de prueba. | 4–5 |
| **F4 — Upgrade Frida** | Bridge a Frida vía REA, sonda mínima `send()`, antídoto por inundación/actualización, `.exe` nativo de prueba. | 3–4 |
| **F5 — Grupo 3 (UIA)** | Solo cuando se quiera: `uiautomation` en obrero STA, por app, con topes agresivos. | 3 |

**Total F1–F4: ~14–18 días.** F5 aparte (~3). Escalonado: F1 y F2 ya dan valor sin ningún Upgrade.

---

## 15. Decisiones que quedan para Victoria

1. **¿`ev enganchar` elevado?** Recomendación: **no** por defecto (todo lo esencial va sin admin, verificado hoy). Elevar solo si quiere observar apps que corren como administrador o la precisión de proceso de WMI.
2. **Notificaciones: ¿`solo_metadatos` o `texto`?** Recomendación: **`solo_metadatos` por defecto** con retención corta (es lo más privado del sistema), y `texto` solo en apps donde ella lo pida explícitamente.
3. **Primera app para el Upgrade.** Recomendación: **VS Code** como banco (Electron, sin antitrampas, sin servicio sensible, contenido propio), antes que WhatsApp/Discord, que traen mensajes de terceros.
