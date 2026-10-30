# eventos (v0)

Bus de eventos universal. Recoge la salida de cualquier cosa (programas, hooks de Claude Code, agentes) en una base SQLite local. Según unas reglas, despierta a agentes como Claude Code, que pueden leer el bus y publicar en él.

El diseño y las razones están en `docs/2026-10-29-investigacion.md`.

## La app: «Bus de eventos»

Se abre con el acceso **Bus de eventos** del escritorio (`D:\Programas\BusEventos\ev-visor.exe`). Es una ventana propia con WebView2, el motor web de Windows; no usa el navegador.

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
- Lo que no llega al bus, porque está parado, colgado o rechaza el envío, espera en `datos\pendientes.jsonl`. El servicio lo importa al arrancar y cada 30 s mientras está en marcha (`EV_IMPORTAR_SEGUNDOS`). Una línea que no vale no se tira: va a `datos\pendientes.rechazados.jsonl`.

## Usar

| Orden | Qué hace |
|---|---|
| `ev run -- cargo test` | Lanza la orden. La salida se ve igual que siempre y cada línea, el inicio y el final (con su código) se guardan como eventos. |
| `ev ver --tipo proceso.fin --limite 20` | Consulta por tipo, fuente (prefijo), correlación, causa, desde y hasta. |
| `ev buscar media notas` | Búsqueda de texto completo en tipo, fuente y cuerpo. |
| `ev cadena <id>` | Árbol causal de un evento: qué lo provocó y todo lo que provocó él. |
| `ev seguir` | Muestra los eventos según llegan. |
| `ev publicar nota "texto"` | Publica un evento a mano. |
| `ev pausa` / `ev reanudar` / `ev estado` | Detienen o reanudan los agentes y muestran el estado del bus. |

## Engancharlo a Claude Code

- **Hooks (hecho el 2026-10-29):**
  - En los ajustes de usuario, `D:\Programas\BusEventos\ev.exe hook` va como hook asíncrono (`"async": true`) en `SessionStart`, `UserPromptSubmit`, `Notification`, `Stop` y `SessionEnd`, así que nunca hace esperar a Claude Code.
  - Publica `claude.<evento>` con la sesión como correlación.
  - Guarda solo lo necesario para saber qué sesión hace qué: sesión, carpeta, modo de permisos, motivo, aviso y herramienta. No copia lo que ella escribe, lo que entra y sale de las herramientas, la última respuesta ni la transcripción; de eso queda solo el nombre en `omitidos`.
  - Comprobado con una sesión real: llegan los cuatro eventos de una sesión `-p` (`Notification` no salta en ese modo).
- **MCP para que cualquier sesión publique y lea (no está hecho; es opcional):** `claude mcp add eventos -s user -- D:\dev\target\release\ev.exe mcp`.

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
- `windows-sys` (decodificar la página OEM, cp850).

## Pruebas

```
cargo test --jobs 2
```

Son 46:

- 25 contra el servicio de verdad, en un puerto libre y con su propia carpeta. Once de ellas salen de las dos revisiones independientes, una por cada objeción. Otras cuatro son del 2026-10-29: la comprobación de herramientas, las dos de qué guarda el hook (lo que no guarda y lo que sí) y la de `/tareas`.
- 21 de piezas sueltas; trece de ellas, del estado de las tareas (una, de las de quórum).

El arnés de Claude se prueba con un `claude` falso (un `.cmd`) que guarda sus argumentos y su entrada.

## Fuera de la v0

- Lanzadores de Codex, opencode y Grok.
- Meter eventos en sesiones ya abiertas.
- ACP.
- `ev tail` (seguir ficheros).
- Registro de eventos y ETW de Windows.
- Pseudoconsola (ConPTY) para los programas en C que acumulan su salida.
- Varias máquinas.
- Recargar las reglas sin reiniciar.
