# Pulso (eventos v0)

Pulso es el bus de eventos universal. Recoge la salida de cualquier cosa (programas, hooks de Claude Code, agentes) en una base SQLite local. Según unas reglas, despierta a agentes como Claude Code, que pueden leer el bus y publicar en él.

El diseño y las razones están en `docs/2026-10-29-investigacion.md`.

**Nombre:** se llama Pulso desde el 2026-10-31 (antes, «eventos»): los programas laten, porque la presencia va por latidos, y los avisos llegan a quien escucha. Cambiaron el nombre visible y el del servidor MCP: `pulso`, con herramientas `mcp__pulso__*`, canal `server:pulso` y avisos `<channel source="pulso" …>`. La carpeta, el crate, el binario `ev`, `D:\Programas\BusEventos` y los tipos de evento siguen igual. Las notas de antes de esa fecha dicen `eventos`.

## La app: «Pulso»

Se abre con el acceso **Pulso** del escritorio (`D:\Programas\BusEventos\ev-visor.exe`). Es una ventana propia con WebView2, el motor web de Windows; no usa el navegador.

- **Al abrir:** si el bus no está en marcha, lo arranca sin consola, con `D:\dev\eventos\reglas.toml`.
- **Al cerrar:** lo para, pero solo si lo arrancó ella. El título de la ventana dice cuál de los dos casos es.
- **Tareas (desde 2026-10-29):** arriba a la izquierda, una línea por tarea con su estado escrito.
  - Los estados son trabajando, necesita entrada, listo para revisar, fallido y parado. Primero sale lo que pide atención.
  - Las tareas salen de La Fábrica, de quórum, de las sesiones de Claude Code, de `ev run` y de los agentes. La nota del agente de diagnóstico va debajo de la tarea fallida.
  - Pulsar una línea abre su último evento. Los cambios a «necesita entrada» o «fallido» se anuncian al lector de pantalla, todos juntos en un solo aviso y lo más urgente primero. Al refrescarse, el foco se queda en la misma línea.
  - Dentro de un mismo estado, primero lo que llegó después.
  - Al reintentar una tarea, su nota anterior desaparece, y una nota que llegue después del reintento ya no se pega.
  - Los títulos de las órdenes van redactados.
  - Lo mismo en JSON: `http://127.0.0.1:8870/tareas?horas=24`, con 24 horas por defecto y una semana como mucho.
- **Topología:** fuentes → bus → reglas → agentes, con sobres de colores que viajan por las aristas.
- **Lista de eventos y detalle por capas:** sobre, causalidad, contenido y procedencia.
- **Modos:** Tiempo real, y Simulación, que repite lo guardado paso a paso o seguido, a ×0,5–×4. También se puede repetir solo la cadena causal de un evento.
- **Botón para pausar o reanudar las reglas.**
- **Si algo falla al arrancar:** la ventana lo dice y queda anotado en `datos\visor.log` (el bus escribe en `datos\servicio.log`).
- **Instalar una versión nueva:** copiar `ev.exe`, `ev-visor.exe` y `WebView2Loader.dll` (sale de `D:\dev\target-visor\release\build\webview2-com-sys-*\out\x64\`) a `D:\Programas\BusEventos`. Sin esa DLL, la app se queda parada antes de abrir la ventana.

El código de la app está en `visor\`, que es un paquete aparte para no cargar `ev` con dependencias de interfaz. Se compila con `CARGO_TARGET_DIR=D:\dev\target-visor cargo build --release`. La página que muestra es `src\visor.html`, que va dentro de `ev` y se sirve en `http://127.0.0.1:8870/`.

## Arrancar el bus a mano

```
cargo build --release                    # el binario queda en D:\dev\target\release\ev.exe
ev servir --reglas D:\dev\eventos\reglas.toml
```

- El servicio escucha solo en `127.0.0.1:8870` (se cambia con `EV_PUERTO`).
- Guarda los datos en `D:\dev\eventos\datos` (se cambia con `EV_DIR`).
- **Sin ventana, como servicio:** `D:\Programas\BusEventos\arrancar-bus.ps1` hace lo mismo que el visor al abrirse si no hay nada escuchando en 8870. La salida va a `datos\servicio.log` y a `servicio-errores.log`, y cada arranque queda en `datos\arranques.log`. Arranca al entrar en Windows desde el 2026-10-30 por el acceso «Pulso (servicio)» de la carpeta Inicio, que crea `instalar-arranque.ps1`. El modo automático de Claude Code bloqueó por «persistencia» que XANA lo creara por su cuenta, y se creó cuando ella lo pidió expresamente. Para quitarlo, se borra el acceso desde `shell:startup`.
- Lo que no llega al bus, porque está parado, colgado o rechaza el envío, espera en `datos\pendientes.jsonl`. El servicio lo importa al arrancar y cada 30 s mientras está en marcha (`EV_IMPORTAR_SEGUNDOS`). Una línea que no vale no se tira: va a `datos\pendientes.rechazados.jsonl`.

## Usar

| Orden | Qué hace |
|---|---|
| `ev run -- cargo test` | Lanza la orden. La salida se ve igual que siempre y cada línea, el inicio y el final (con su código) se guardan como eventos. |
| `ev tail D:\ruta\app.log --app constructo` | Sigue un fichero de registro y publica cada línea nueva hasta Ctrl+C. Ver «Seguir ficheros de registro». |
| `ev ver --tipo proceso.fin --limite 20` | Consulta por tipo, fuente (prefijo), correlación, causa, desde y hasta. Tipo, fuente y correlación admiten `*` como en las suscripciones (`--fuente "app:constructo*"`); sin `*`, el tipo y la correlación son exactos y la fuente, un prefijo. Da los últimos 100 (o `--limite`, como mucho 1000); con `--despues SEQ`, los primeros tras ese seq. Si llega al tope, avisa por stderr del `--despues` con el que seguir. |
| `ev buscar media notas` | Búsqueda de texto completo en tipo, fuente y cuerpo. |
| `ev cadena <id>` | Árbol causal de un evento: qué lo provocó y todo lo que provocó él. |
| `ev seguir` | Muestra los eventos según llegan. |
| `ev publicar nota "texto"` | Publica un evento a mano. |
| `ev pausa` / `ev reanudar` / `ev estado` | Detienen o reanudan los agentes y muestran el estado del bus, con cuántas sesiones y agentes hay conectados. |

## Engancharlo a Claude Code

- **Hooks (hecho el 2026-10-29):**
  - En los ajustes de usuario, `D:\Programas\BusEventos\ev.exe hook` va como hook asíncrono (`"async": true`) en `SessionStart`, `UserPromptSubmit`, `Notification`, `Stop` y `SessionEnd`, así que nunca hace esperar a Claude Code.
  - Publica `claude.<evento>` con la sesión como correlación.
  - No publica las sesiones internas de las apps, como las de Constructo (ver «Sesiones internas de las apps», más abajo).
  - Guarda solo lo necesario para saber qué sesión hace qué: sesión, carpeta, modo de permisos, motivo, aviso y herramienta. No copia lo que ella escribe, lo que entra y sale de las herramientas, la última respuesta ni la transcripción; de eso queda solo el nombre en `omitidos`.
  - Comprobado con una sesión real: llegan los cuatro eventos de una sesión `-p` (`Notification` no salta en ese modo).
- **MCP en todas sus sesiones (hecho el 2026-10-30; se llama `pulso` desde el 2026-10-31):** `claude mcp add --scope user pulso -- D:\Programas\BusEventos\ev.exe mcp` (copia previa de `~/.claude.json` en `D:\Backups`). Da `publicar`, `buscar` y `esperar`; en una sesión interactiva, también `suscribir`, `suscripciones` y `desuscribir`. Los agentes de las reglas, y cualquier `claude -p` o Agent SDK (`CLAUDE_CODE_ENTRYPOINT=sdk-*`), solo ven las tres primeras: ahí Claude Code no entrega canales.

### Sesiones internas de las apps (desde 2026-10-31)

`ev hook` no publica nada de una sesión cuyo `cwd` esté dentro de una carpeta ignorada: ni al bus ni a la cola en disco. Tampoco escribe nada y sigue terminando con 0, así que no frena a Claude Code. Así, en el bus se ven la app y las sesiones de verdad, no el ruido: el 2026-10-31, 216 de los 226 `SessionStart` del día eran sesiones internas de Constructo.

- **Por defecto:** `<carpeta temporal>\lector-ia-skills`, donde Constructo abre sus `claude -p` internas (`sesion-<pid>-<azar>`). La carpeta temporal es la de `TEMP`, la misma que usa Constructo (`os.tmpdir()` de Node); sin `TEMP`, la de `std::env::temp_dir()`.
- **Más carpetas, sin recompilar:** `EV_HOOK_IGNORAR`, con rutas absolutas separadas por `;`, que se suman a la de por defecto.
  - Se quitan los espacios y las comillas de cada ruta, y las entradas vacías no cuentan.
  - Una entrada `-` quita la de por defecto: `EV_HOOK_IGNORAR=-` no ignora nada, y `-;D:\otra` ignora solo `D:\otra`.
  - La variable tiene que estar en el entorno con el que Claude Code lanza el hook; por ejemplo, como variable de usuario de Windows. Lo que ya estaba abierto al crearla no la ve.
- **Cómo compara:** por componentes de la ruta, no por prefijo de texto.
  - `…\lector-ia-skills2` no casa con `…\lector-ia-skills`, ni otra carpeta `lector-ia-skills` fuera de la temporal.
  - En Windows no distingue mayúsculas, y `/` vale como `\`, también repetidas o al final.
  - Una entrada sin componentes, como `\`, no ignora nada.
  - Solo compara el texto: no resuelve nombres cortos (8.3), enlaces ni `..`.
- **Presencia:** no cambia. `conectados` sale de los latidos de `ev mcp`, no del hook. Las sesiones de Constructo no cargan `ev mcp`, porque van con `--strict-mcp-config` y sin `--mcp-config` (visto en su código el 2026-10-31); así que tampoco laten.
- **Tareas:** sin sus `claude.*`, esas sesiones ya no salen en el panel de tareas.

### Avisos por suscripción (desde 2026-10-30)

La sesión dice qué quiere que el bus le avise y `ev mcp` se lo empuja por el canal de Claude Code (`notifications/claude/channel`, *research preview*). Le llega solo, como `<channel source="pulso" suscripcion="s1" motivo="…" tipo="…" evento="…">`, sin esperar a que pregunte.

- **Filtros exactos, como las reglas:**
  - Tipo, fuente y correlación admiten `*`.
  - También se puede filtrar por una o varias gravedades, por atributos que valgan exactamente un valor y por texto en el cuerpo, sin distinguir mayúsculas.
  - Hace falta acotar al menos una de estas cosas: tipo, fuente, correlación o texto. Suscribirse a todo se rechaza.
  - Nada de modelos decidiendo qué avisar; d1 queda para una segunda fase, medida con eventos reales.
- **Límites:**
  - Cada suscripción caduca: 120 min por defecto, 24 h como mucho. Al caducar llega un último aviso.
  - Tope por hora en ventana deslizante: 6 por defecto, 60 como mucho. Al llenarse avisa una vez y luego cuenta lo que calla.
  - Hasta 10 suscripciones por sesión.
  - Lo que casa en la misma vuelta va en un aviso, con 5 eventos enseñados y el resto contado.
- **No se avisa a la sesión de lo suyo:** ni de lo que publicó ella misma ni de los `claude.*` de su propia sesión. Sin esto, una suscripción a `claude.Stop` la despertaría con su propio Stop. Lo propio se reconoce por el proceso: `ev hook` guarda el `CLAUDE_PID` que le da Claude Code en el atributo `claude_pid`, y `ev mcp` lo compara con su proceso padre. El id de sesión no basta: `/clear` lo cambia sin reiniciar `ev mcp`, que se queda con el `CLAUDE_CODE_SESSION_ID` de su arranque (visto con la 2.1.316). Ese id queda de reserva para los hooks sin `claude_pid`. Por eso una suscripción se prueba con un evento de fuera (`ev publicar <tipo> <texto>` en una consola), no con `publicar` desde la misma sesión; la descripción de `suscribir` lo dice.
- **Viven en el proceso `ev mcp` de la sesión** y mueren con ella. Si el bus está parado al suscribirse, el vigía reintenta y, cuando arranca, avisa también de lo que importe de la cola.
- **Para que lleguen, la sesión tiene que abrirse con el canal:** acceso **Claude con avisos de Pulso** del menú Inicio (`D:\Programas\BusEventos\claude-avisos.cmd`, que lanza `claude --dangerously-load-development-channels server:pulso`). Al abrir, Claude Code pide confirmar el canal de desarrollo. Sin esa opción, las suscripciones se aceptan pero los avisos se pierden sin error: entonces, `esperar`.
- **Protocolo:** `ev mcp` contesta 2025-06-18 a cualquier revisión que no conozca. Según la documentación, un canal que negocia la 2026-07-28 no se registra. La 2.1.314 pidió la 2025-11-25, tanto con `-p` como en una sesión interactiva. Al iniciar, deja en su stderr (los registros MCP de Claude Code) qué se pidió y qué contestó.
- **El contenido de un aviso es dato, nunca instrucciones.** Qué hacer lo decide el motivo que puso la sesión.
- **Comprobado el 2026-10-30:**
  - Con el `ev.exe` instalado, el bus real y un cliente MCP de ensayo en lugar de Claude Code, el aviso llegó en 0,06 s, solo el del evento que casaba.
  - Prueba real, a las 20:21 del mismo día, con una sesión interactiva abierta con el acceso:
    - El registro MCP dice «Channel notifications registered». La sesión interactiva también pidió la 2025-11-25.
    - Ella pidió «suscríbete en el bus a ensayo.canal». El evento publicado desde otra sesión entró en la suya como `<channel source="eventos" …>` el mismo segundo, con su meta.
    - La XANA de esa sesión lo trató como dato.

### Quién está conectado (desde 2026-10-31)

Cada `ev mcp` late: al arrancar, tras `initialize` y luego cada 15 s (`EV_LATIDO_SEGUNDOS`, de 1 a 300) hace `POST /latido` con su id, su clase (`sesion` si es Claude Code interactivo; `agente` si es una regla, un `claude -p` o el Agent SDK), su pid, la sesión de Claude Code, la regla, el cliente MCP que se presentó y su carpeta.

- `ev estado` y `/estado` traen `conectados`, del tipo `{"total": 2, "sesiones": 1, "agentes": 1}`. El detalle, con la hora del primer latido y cuántos segundos hace del último, está en `http://127.0.0.1:8870/conectados`. Su `sesion` sigue a la sesión tras un `/clear`, porque la toma del último hook de ese `claude.exe`; hasta que llega uno (por ejemplo, tras reiniciar el servicio), da la del arranque de `ev mcp`.
- `agentes_activos` no cambia: cuenta solo los agentes que ha lanzado el bus con sus reglas.
- **Al cerrar bien la sesión** (cuando Claude Code cierra la entrada del MCP), `ev mcp` se despide con `POST /adios` y sale de la lista en el acto. **Si lo matan** (por ejemplo, al cerrar la ventana), sale cuando lleva tres periodos sin latir: 45 s por defecto.
- Los hooks no sirven para contarlas: cerrar la ventana no deja `SessionEnd`. El 2026-10-31, contar `SessionStart` menos `SessionEnd` daba 27 sesiones «abiertas» con una sola viva.
- La lista vive en memoria y tiene un tope de 256. Tras reiniciar el servicio, cada uno vuelve a aparecer en su siguiente latido. Un `ev mcp` anterior a este cambio no late, así que no aparece hasta que su sesión se reabre o se reconecta con `/mcp`.
- Si el bus no responde, el latido se pierde sin error ni espera larga, porque usa un plazo de 2 s; la despedida usa 1 s.

## La Fábrica (desde 2026-10-29)

La Fábrica publica tres tipos de evento, todos con la correlación `fabrica:<proyecto>:<tarea>`:

- `fabrica.tarea.inicio`: la tarea pasa a en curso (cuerpo: el título).
- `fabrica.tarea.fin`: la tarea acaba; `atributos.puertas_ok` dice si pasaron sus puertas (nulo si no tenía).
- `fabrica.puerta.fallo`: uno por cada puerta que falló en la última pasada (cuerpo: el final de su salida).

Si el bus no responde, esperan en `datos\pendientes.jsonl` sin la salida de las puertas. El puente está en `D:\dev\Fabrica\src-tauri\src\bus.rs`. La regla `diagnosticar-puerta-fabrica` despierta con cada fallo a un agente de lectura que publica una `nota.reanudacion`: qué falla, por qué, qué cambió y la siguiente orden. Comprobado el 2026-10-29 con Claude real: 24 s y sin tocar ningún fichero.

## Quórum (desde 2026-10-29)

Quórum (`D:\dev\quorum`) publica cuatro tipos de evento, todos con la correlación `quorum:<proyecto>:<tarea>`:

- `quorum.tarea.inicio`: la tarea empieza (cuerpo: la tarea; `atributos.autor`: el modelo del autor).
- `quorum.puerta`: el resultado de las pruebas en una ronda (`atributos.ronda` y `atributos.ok`; cuerpo: el final de su salida).
- `quorum.voto`: el voto de un revisor (`atributos.revisor`, `atributos.modelo` y `atributos.voto`: APRUEBA, RECHAZA o ABSTENCIÓN).
- `quorum.tarea.fin`: el final (`atributos.estado`: `integrada`, `aprobada` cuando no se pudo integrar, `rechazada`, `sin_cambios` o `error`).

En el panel de tareas, integrada queda lista para revisar; aprobada sin integrar, a la espera de que alguien la integre a mano; rechazada o con error, fallida; sin cambios, parada. Si el bus no responde, esperan en `datos\pendientes.jsonl` solo con sus metadatos y la tarea: sin la salida de la puerta, las objeciones ni el detalle del final, que pueden traer texto del repositorio.

Las reglas `diagnosticar-quorum-rechazada` y `diagnosticar-quorum-error` (desde 2026-10-30) despiertan con esos dos finales al agente de lectura, que lee los eventos de la tarea (y, si hace falta, el código en `D:\dev`, donde están los proyectos y sus worktrees en `.quorum`) y publica una `nota.reanudacion`: qué se pedía, qué la tumbó, qué falta y la siguiente orden. El panel la engancha bajo la tarea. Una puerta fallida no dispara nada, porque Quórum aún tiene rondas. Comprobado el 2026-10-30 con Claude real en un bus aparte: 13 s, solo `buscar` y `publicar`, sin tocar ficheros. Si el bus estaba parado, la tarea llega de la cola sin el detalle del final y el agente tiene menos con qué trabajar.

## Enganches (desde 2026-10-31)

`ev enganchar --config D:\dev\eventos\enganches.toml` vigila desde fuera las apps de `enganches.toml` y publica, con la fuente `app:<slug>` y la correlación `app:<slug>:<arranque>`:

- `app.inicio` y `app.fin`: se abre (o ya estaba abierta al empezar) y se cierra. `app.fin` lleva el código de salida y `atributos.cierre`, que vale `excepcion` si el código es una excepción de Windows y `normal` en otro caso.
- `app.sin_respuesta` (aviso) y `app.respuesta`: la ventana lleva `colgada_segundos` sin responder, además de los 5 s que espera Windows, y luego vuelve.
- `app.caida` (error): la caída que anota Windows en el registro Application (proveedor `Application Error`, Id 1000).
- `app.titulo`: cambios de título. Está apagado por defecto y es `confianza: externa`.
- Además, `enganche.tanteo` (uno por app al arrancar) y `enganche.tope` (cuando una app llena su tope por hora).

La sección `[aviso]` de `enganches.toml` lanza una orden con cada evento de los tipos que elija, sin consola y sin esperarla. Hoy, con `app.sin_respuesta` y `app.caida`, lanza la ventanita de avisos de XANA (`D:\Ari\avisos`). En la orden, `{cuerpo}`, `{app}` y `{tipo}` se sustituyen por los datos del evento. Cada uno va como argumento aparte, sin pasar por ninguna consola.

### Carpetas vigiladas

Cada `[[carpeta]]` de `enganches.toml` hace que `ev enganchar` publique un `carpeta.fichero` (gravedad `info`, fuente `carpeta:<ruta>`) cuando aparece un fichero nuevo en ella. Nada se vigila si no está en el fichero.

```toml
[[carpeta]]
ruta = 'D:\algo\Descargas'      # obligatoria; tiene que existir y ser una carpeta
recursiva = false               # por defecto false: las subcarpetas no cuentan
ignorar = ["*.tmp", "*.crdownload", "*.part", "~$*"]   # por defecto, estos cuatro
antirrebote_ms = 2000           # por defecto 2000
por_hora = 120                  # por defecto 120, como las apps
```

- El cuerpo lleva la ruta relativa a la carpeta y el tamaño (`informe.pdf (3000 bytes)`). Los atributos son `carpeta`, `relativa`, `nombre` y `bytes`. Nunca se lee el contenido del fichero. Van con `confianza: externa`: el nombre lo puede haber elegido otro (una web para una descarga, quien manda un adjunto).
- Un fichero se publica una sola vez, cuando lleva `antirrebote_ms` sin cambiar de tamaño. Un programa que escribe en varios pasos da un solo evento. Una descarga que se crea como `.crdownload` y se renombra a `.pdf` da uno, con el nombre final.
- `ignorar` mira el nombre del fichero, con `*` y `?` y sin distinguir mayúsculas. Si se pone, sustituye a los cuatro de por defecto. Los directorios no son ficheros y no se publican.
- Con `recursiva`, una subcarpeta que llega ya llena (movida o descomprimida de golpe; Windows solo avisa de ella) se recorre, y cada fichero que trae se publica como nuevo, hasta 4096 por subcarpeta. Por lo mismo, renombrar una subcarpeta dentro de la vigilada vuelve a publicar lo suyo, con las rutas nuevas.
- Los eventos pasan por los mismos topes que los de las apps: `por_segundo_global` y el `por_hora` de cada carpeta.
- Al arrancar se publica un `enganche.tanteo` por carpeta (fuente `enganche:carpeta:<ruta>`).
- `ev enganchar` no arranca si una ruta está vacía, no existe, no es una carpeta o está repetida, y dice cuál.

No inyecta nada en las apps ni necesita administrador. Hoy vigila Constructo, Obsidian y Brave. Diseño completo y fases siguientes en `docs/2026-10-30-diseno-enganches.md`. Las pruebas de escritorio se lanzan con `cargo test --test enganches -- --ignored --test-threads=1`.

### Seguir ficheros de registro (desde 2026-10-31)

Para ver lo que una app escribe en su registro mientras XANA trabaja en ella con Victoria. Hay dos formas:

- **`ev tail <ruta>... [--app SLUG] [--desde-inicio] [--por-segundo N]`**, sin tocar configuración. Es lo que XANA lanza en segundo plano mientras trabaja en una app; sigue hasta Ctrl+C o hasta que se cierre el proceso. Las rutas relativas se toman desde la carpeta actual. `--segundos N` termina solo a los N segundos.
- **Entradas `[[fichero]]` en `enganches.toml`**, que `ev enganchar` vigila junto a las apps:

  ```toml
  [[fichero]]
  ruta = 'D:\Programas\Constructo-win32-x64\registro.log'   # obligatoria y absoluta; puede no existir aún
  app = "constructo"     # opcional: va en los atributos y, si la app está abierta, en su correlación
  desde_inicio = false   # leer también lo que ya tenía al empezar
  por_segundo = 20       # tope de líneas por segundo (de 1 a 1000)
  ```

  `ev enganchar` rechaza al arrancar una ruta vacía, relativa, con comodines (`*`, `?`), que sea una carpeta o repetida (sin distinguir mayúsculas ni el tipo de barra), un `por_segundo` fuera de 1 a 1000 y una `app` que no tenga forma de slug.

Qué publica, siempre con la fuente `fichero:<ruta>`:

- **`registro.linea`**: cada línea nueva, con `confianza: externa` (una línea de registro puede traer texto de terceros) y `atributos` `ruta`, `linea` (su número en el fichero), `app` si se dio, e `incompleta` si se quedó sin salto. Las líneas vacías cuentan para la numeración, pero no se publican. En `ev enganchar`, si la `app` está abierta, la línea va con la correlación de su arranque (`app:<slug>:<arranque>`), junto a su `app.inicio`; con `ev tail`, sin correlación.
- **`registro.inicio`**: uno por fichero al empezar, cuando ya está decidido desde dónde se lee. Dice si existe y cuántas líneas tenía (`saltadas`). Lo que se escriba después de verlo en el bus se publicará.
- **`registro.rotado`**: se vuelve a leer desde el principio. `motivo` vale `encogido`, `cambiado` o `desaparecido`.
- **`registro.tope`** (aviso): se han descartado líneas por pasar del tope. Lleva cuántas y cuáles (`desde_linea`, `hasta_linea`), para ir a leerlas al fichero. Sale una vez por racha, cuando acaba.
- **`registro.error`** (aviso): no se puede leer el fichero (por ejemplo, otro programa lo tiene en exclusiva). Sale una vez por cada error distinto.

Cómo lee:

- **Por defecto empieza al final**, tras la última línea completa: lo que ya había no se vuelve a publicar. Con `--desde-inicio` o `desde_inicio = true`, sí.
- **Solo líneas completas.** Lo que va tras el último salto espera en un búfer hasta que llega el suyo. Si el fichero se rota con una línea a medias, esa línea sale como `incompleta`: era lo último del fichero de antes.
- **Rotación y truncado:** si el fichero encoge, o si cambian sus primeros 64 bytes o los 64 de justo antes de donde iba, lo relee desde 0. La segunda comprobación sirve para las apps que empiezan su registro siempre con la misma cabecera.
- **Si aún no existe**, también su carpeta, espera a que aparezca y lo lee desde 0, porque todo lo que trae es nuevo.
- **Decodificación igual que `ev run`:** UTF-8 y, si no lo es, la página OEM (cp850). Quita los `\r` de los finales CRLF y la marca de orden de bytes del principio. Una línea de más de 64 KB se guarda recortada.
- **No estorba a la app.** Abre el fichero en cada pasada, compartido para leer, escribir y borrar, y lo cierra en seguida: la app puede rotarlo o borrarlo mientras se sigue.
- **`notify` y sondeo:** `notify` avisa de los cambios en la carpeta y la lectura es inmediata. Además se miran todos los ficheros cada segundo. Comprobado en este equipo el 2026-10-31: si la app mantiene el fichero abierto y solo vacía su búfer, Windows no avisa (tres veces, 20 s cada una, sin aviso). En ese caso la línea llega por el sondeo, en 1 s como mucho.
- **Tope por fichero:** 20 líneas por segundo por defecto, en ventana deslizante. En `ev enganchar` cuentan también para el tope global (`por_segundo_global`). Allí dos líneas iguales seguidas no se toman por una repetición, como sí se hace con los demás eventos.

Límites: no guarda por dónde iba entre un arranque y otro, así que al volver a lanzarlo empieza otra vez al final. No admite comodines ni registros en UTF-16. Si la app rota el fichero cambiándole el nombre, lo que escribiera entre la última pasada y la rotación se pierde. Si `ev tail` se mata sin Ctrl+C, se pueden perder los eventos del último cuarto de segundo, que aún no se habían enviado.

## Reglas

Las reglas están en `reglas.toml`. Cada una dice: cuando llegue un evento de este `tipo` (y, si se indica, con esta `fuente`, estos `atributos` o `atributos_distinto`), llama a este `arnes` con este `encargo`.

**Límites:**

- `profundidad`: cuántos agentes encadenados como máximo.
- `por_hora`.
- `a_la_vez` y `max_agentes` (este último, global). Cuando no hay hueco libre, el disparo espera en una cola; no se pierde.
- `minutos`: tiempo máximo de cada agente (admite fracciones; `0.5` son 30 s).
- Pausa del bus.

**Qué deja huella en el bus:**

- Cada disparo publica `regla.disparo`.
- Cada espera publica `regla.en_espera`.
- Cada descarte publica `regla.omitida`, con su motivo.

**Arneses de la v0:**

- `claude`: `claude -p --restricted --strict-mcp-config --tools "Read,Grep,Glob"`, solo de lectura. De las herramientas propias de Claude Code solo tiene Read, Grep y Glob; sin tus hooks ni tus ajustes de usuario, y con el MCP del bus como único MCP.
- `eco`: para pruebas.

El evento que dispara la regla entra en el encargo entre etiquetas `<evento>`, como dato y no como orden. Todo lo que hace el agente vuelve al bus como hijos de ese evento.

## Seguridad

- **Solo este equipo, y nunca desde una web:**
  - El servicio escucha únicamente en 127.0.0.1.
  - Rechaza (403) cualquier petición cuyo Host no sea `127.0.0.1` o `localhost` con su puerto, para que una web no pueda leer el bus con *DNS rebinding*.
  - También rechaza cualquier Origin que no sea el propio bus, para que una web abierta en el navegador no pueda publicar eventos ni quitar la pausa.
  - No hay autenticación: cualquier programa de este equipo puede publicar.
- **Redacción al entrar**, en la fuente, el cuerpo y los atributos, y también en la cola en disco:
  - Claves con pinta de secreto, también con prefijo o sufijo (`GITHUB_TOKEN=`, `DB_PASSWORD=`) y en JSON, incluso con las comillas escapadas, como llega dentro de una orden (`"password":"…"`, `\"password\":\"…\"`).
  - Opciones con el valor separado por espacio (`--password …`).
  - Credenciales dentro de una URL (`postgres://usuario:…@servidor`).
  - `Bearer …` y `Basic …`.
  - Tokens sueltos con prefijo conocido (`sk-`, `ghp_`, `xox…`).
- **El evento entra al agente solo como dato:**
  - Va dentro de `<evento>…</evento>` como JSON, con `<` y `>` escapados. Nada de él se copia fuera de ese bloque.
  - La procedencia (`confianza`) la declara quien publica; el bus no la verifica.
- **Agentes de solo lectura:**
  - Sin permiso para escribir, enviar ni publicar fuera.
  - Un plazo (`minutos`) que se cumple siempre: al agotarse se termina el árbol de procesos entero y se libera su hueco.

## Límites conocidos de la v0

- El cupo por hora y la cola de espera de las reglas viven en memoria: al reiniciar el servicio, la cuenta empieza de cero.
- Caben hasta 100 disparos esperando hueco. Si hay más, se omiten, y cada uno queda anotado como `regla.omitida`.
- No hay enfriamiento entre disparos de una misma regla. Lo frenan `por_hora`, `a_la_vez` y `max_agentes`.
- El flujo en vivo es una espera larga (`/esperar`, también desde `ev seguir` y desde el visor), no SSE.
- Lo que publica un agente lleva su regla y su modelo en `atributos`. El id de sesión va en `agente.inicio` y `agente.fin`, con la misma correlación.
- Si un agente termina por sí solo dejando un proceso nieto en marcha, ese nieto sobrevive. El corte por plazo sí termina el árbol entero.
- Si el bus guarda un evento pero tarda más de lo que espera el cliente, ese evento también puede acabar en la cola y quedar duplicado. Se prefiere duplicar antes que perder.
- **Lo que la redacción no reconoce sigue en claro.** Por ejemplo:
  - `{"name":"DB_PASSWORD","value":"…"}`;
  - `passphrase=` y `pwd=`;
  - opciones cortas como `docker login -p …`;
  - JSON con comillas simples o con tres niveles de escape.
- **La solo lectura la dan `--tools "Read,Grep,Glob"` y `--disallowedTools`, no `--restricted` por sí solo.** `--restricted` quita las herramientas que ejecutan órdenes, pero deja otras con efectos fuera (publicar artefactos, mensajes a otras sesiones, notificaciones, worktrees, subagentes): comprobado con claude 2.1.312 el 2026-10-29 en la lista que anuncia al arrancar. Las pruebas vigilan la lista exacta de herramientas y que no aparezcan las opciones peligrosas conocidas.

## Dependencias

Cada una cubre algo que la biblioteca estándar no trae:

- `rusqlite` (con `bundled`, SQLite con FTS5).
- `tiny_http` (servidor HTTP pequeño y síncrono).
- `ureq` (cliente HTTP, sin TLS).
- `serde` y `serde_json`.
- `toml` (reglas).
- `ulid` (ids ordenables).
- `regex` (redacción).
- `notify` (carpetas vigiladas de los enganches y avisos de cambios de los ficheros de registro que se siguen).
- `windows-sys` (decodificar la página OEM, cp850).

## Pruebas

```
cargo test --jobs 2
```

Son 60, más las de los enganches (`tests/enganches.rs`: 25, diez de ellas de las carpetas vigiladas, con carpetas temporales de verdad; y 3 de escritorio que van aparte) y las de los ficheros de registro (`tests/tail.rs`: 20, desde el 2026-10-31; 14 de piezas sueltas con ficheros temporales y 6 con `ev tail` o `ev enganchar` contra un bus de verdad):

- **32 contra el servicio de verdad**, en un puerto libre y con su propia carpeta.
  - Una es del 2026-10-31, del tope de `ev ver` y `ev buscar`: al llegar al tope avisan por stderr con el `--despues` con el que seguir, y por debajo no; `--despues` pagina. Falla con el `main.rs` de antes.
  - Tres son del 2026-10-31, de las sesiones que `ev hook` no publica: lo de la carpeta de Constructo no llega al bus ni a la cola y lo demás sí; la comparación va por componentes (un nombre que empieza igual no casa; mayúsculas y separadores dan igual); y `EV_HOOK_IGNORAR` suma carpetas y, con `-`, quita la de por defecto. Las tres fallaban sin el cambio, y detectaron cinco mutaciones: comparar por prefijo de texto, distinguir mayúsculas, aceptar solo `\`, dejar que una entrada vacía lo ignore todo y no hacer caso de `-`.
  - Una es del 2026-10-31, la de quién está conectado. Una sesión y un `-p` laten y se cuentan como sesión y como agente. El `-p` muerto sale de la lista al dejar de latir, y no antes. La sesión que cierra su entrada se despide y sale en el acto. Un latido que no vale se rechaza. Si se quita la despedida, la prueba falla (mutación comprobada).
  - Once salen de las dos revisiones independientes, una por cada objeción.
  - Cuatro son del 2026-10-29: la comprobación de herramientas, las dos de qué guarda el hook (lo que no guarda y lo que sí) y la de `/tareas`.
  - Dos son del 2026-10-30. La primera abre una sesión MCP que se suscribe y comprueba que solo le llega lo que pidió: nada suyo, el Stop de otra sesión sí y el suyo no, y un `-p` sin canal. La segunda se suscribe con el bus parado y recibe el aviso al arrancar.
- **28 de piezas sueltas**: trece, del estado de las tareas (una de ellas, de las de quórum); una, de las reglas del repositorio; cuatro, de las suscripciones (filtro, validación, tope y forma del aviso); dos, de la presencia (caducidad, renovación y despedida; validación del latido y tope).

Las pruebas de suscripción detectaron las cinco mutaciones probadas el 2026-10-30:

- contestar la revisión que pide el cliente;
- no excluir la propia sesión;
- buscarla en los atributos, que es donde no está;
- no excluir lo propio;
- dar canal a un `-p`.

El arnés de Claude se prueba con un `claude` falso (un `.cmd`) que guarda sus argumentos y su entrada.

## Fuera de la v0

- Lanzadores de Codex, opencode y Grok.
- Meter eventos en sesiones ya abiertas sin el canal: con el canal ya se hace (avisos por suscripción, 2026-10-30).
- ACP.
- Registro de eventos y ETW de Windows.
- Pseudoconsola (ConPTY) para los programas en C que acumulan su salida.
- Varias máquinas.
- Recargar las reglas sin reiniciar.
