# Sistema universal de eventos: investigación

2026-10-29 · XANA, para Victoria. `eventos` es un nombre de trabajo para la carpeta.

## Resumen

1. Ningún mecanismo único recoge «el output de cualquier cosa». Hacen falta adaptadores sencillos para cada tipo de fuente, un sobre común y un único almacén. Así lo resuelven todos los sistemas existentes (OpenTelemetry Collector, Fluent Bit, Vector).
2. Recomendación para tu caso (un solo portátil con poca RAM, todo en local y SQLite en casi todos tus proyectos): un núcleo propio pequeño, formado por el sobre, una base SQLite en D:, una entrada HTTP local y un envoltorio de procesos. Fluent Bit u OTel Collector solo harían falta como adaptadores para el registro de eventos de Windows.
3. Medido hoy en este equipo: un programa en Python leído por tubería entrega todas las líneas juntas al terminar, y los programas de consola de Windows escriben en cp850, no en UTF-8. El envoltorio tiene que resolver las dos cosas (apartado 4).
4. Ampliación: el bus se engancha a Claude Code, Codex, opencode y Grok Build (los cuatro están instalados y los he comprobado). Cada modelo publica y lee eventos mediante un único servidor MCP del bus, y el bus llama a los modelos en modo sin interfaz según reglas con límites contra bucles, gasto e inyección (apartados 7 a 9).

## 1. Qué abarca «cualquier cosa»: fuentes y cómo se captura cada una

| Fuente | Cómo se captura | Límite |
|---|---|---|
| Programa que lanzas tú (consola) | Un envoltorio que lo lanza y lee su salida estándar y de errores | El hijo acumula la salida en un búfer y usa la codificación OEM (las dos cosas, medidas). Los programas interactivos necesitan una pseudoconsola (ConPTY) |
| Programa ya en marcha que no lanzaste tú | Su salida estándar no se puede leer desde fuera sin depurarlo o inyectarle código | Se recoge lo que deja: ficheros de log, el registro de eventos o ETW |
| Ficheros de log | Seguir el final del fichero, recordando la posición y una huella del contenido para detectar rotaciones | No sirve con ficheros que se reescriben enteros |
| Windows | Registro de eventos (canales), ETW (procesos, red, ficheros) y Sysmon | Leer ETW en tiempo real exige privilegios de administrador con muchos proveedores |
| Programas tuyos | Que emitan directamente: un POST con JSON a un puerto local, u OTLP | Hay que modificarlos (una línea por evento) |
| Webs | Yautja y Sombra ya ven la red, la consola y el DOM | Solo falta reenviar lo que ya capturan |
| Aplicaciones gráficas sin texto | UI Automation, OCR de Windows (ya lo tienes) o capturas de pantalla | Caro y ruidoso: último recurso |
| Carpetas | Avisos de cambios del sistema de ficheros | Dicen qué cambió, no qué significa |
| Juegos | Observatorios propios (CoI) y lectores de memoria (EVE) | Un adaptador por juego |
| Claude Code | Hooks y transcripciones `.jsonl` | Ninguno relevante |

Consecuencia: lo universal no sale de un capturador mágico, sino de dos decisiones. Se acepta cualquier entrada como dato en bruto, y cada fuente nueva cuesta solo un adaptador barato.

## 2. Anatomía común

```
fuente -> adaptador -> sobre -> búfer -> almacén -> consulta / reacción
```

Todos los sistemas del apartado 5 tienen esta forma. Se diferencian en qué parte resuelven bien.

## 3. El sobre (formato de cada evento)

Hay dos estándares que conviene seguir:

- **CloudEvents 1.0.2** (proyecto graduado de la CNCF desde enero de 2024): `id`, `source`, `type`, `specversion`, `time`, `subject`, `datacontenttype` y `data`. Está pensado para mover eventos entre sistemas.
- **Modelo de datos de logs de OpenTelemetry**: separa el momento del suceso (`Timestamp`) del momento en que se observó (`ObservedTimestamp`), y añade `SeverityNumber`, `Body`, `Attributes`, `Resource` y `TraceId`/`SpanId`.

Recomendación: tomar CloudEvents como base, añadirle los dos relojes del modelo de OTel y guardar siempre el dato en bruto. La interpretación se hace al leer, no al recoger (*schema-on-read*); así, una fuente nueva no obliga a diseñar un esquema antes de empezar a recogerla.

Borrador de campos:

```
id           ULID (se puede ordenar por tiempo)
fuente       "proc:pytest", "fichero:D:/dev/Radiant/app/datos/registro.log", "http:yautja"
tipo         "linea.stdout", "linea.stderr", "proceso.fin", "windows.evento", ...
t_evento     cuándo ocurrió (si la fuente lo dice)
t_visto      cuándo llegó al recolector
gravedad     opcional
cuerpo       texto o JSON
atributos    JSON (pid, código de salida, máquina, directorio...)
correlacion  id de ejecución o de sesión, para agrupar
```

## 4. Experimentos de hoy

Script: `D:\dev\eventos\experimentos\prueba_tuberia.py`. Lanza un proceso hijo que escribe una línea por segundo y anota cuándo llega cada línea al proceso padre.

| Hijo | Llegada de las 3 líneas (s) |
|---|---|
| Python leído por tubería | 3,06 · 3,06 · 3,06 (todas juntas, al terminar) |
| Python con `-u` | 0,08 · 1,08 · 2,08 |
| Python con `PYTHONUNBUFFERED=1` | 0,06 · 1,06 · 2,06 |
| `ping.exe` (nativo) | Una por segundo, sin retraso |

- **Búfer.** Python acumula la salida cuando no escribe en una consola, así que un recolector por tubería vería los eventos tarde, todos juntos o, si el proceso muere, nunca. Con `PYTHONUNBUFFERED=1` en el entorno del hijo se corrige (verificado). Los programas en C que usan el runtime de Microsoft se comportan igual según su documentación, pero esto no lo he medido. Para ellos, el remedio general es lanzarlos dentro de una pseudoconsola (ConPTY), que tampoco he probado aún.
- **Codificación.** La página de códigos OEM de este equipo es 850, la ANSI es 1252 y la de la consola de Claude Code, 65001. La salida de `ping` leída como UTF-8 da `Estad�sticas`; leída como cp850, `Estadísticas` (verificado). El envoltorio debe decodificar con la página OEM, salvo que el hijo declare UTF-8.

## 5. Lo que ya existe

- **OpenTelemetry Collector** (Go). Tiene los receptores `filelog` (en beta según fuentes secundarias de agosto de 2026), `windowseventlog` (solo en la distribución contrib, no en el binario base) y `otlp`. Es el estándar de facto, pensado para enviar los datos a un backend.
- **Fluent Bit** (C; el más ligero de los tres). La versión 5.1.0 se publicó el 5 de agosto de 2026. Tiene las entradas `exec`, `tail`, `winevtlog` y `http`, y soporte nativo como servicio de Windows.
- **Vector** (Rust, de Datadog). Su fuente `exec` tiene un modo `streaming` que distingue stdout de stderr en `data_stream`, y transforma los datos con su lenguaje VRL. Su comportamiento en Windows no lo he comprobado hoy.
- **Redpanda Connect** (antes Benthos). El núcleo es Apache 2.0; algunos conectores tienen licencia comunitaria o enterprise. Existe un fork libre, Bento.
- **Almacenes:**
  - SQLite con FTS5, que ya usas en Spur, Radiant y cotilleando.
  - DuckDB, para análisis.
  - VictoriaLogs: un solo binario, Apache 2.0. Sus cifras frente a Loki salen de un benchmark de parte.
  - Loki y OpenObserve.

Lo que ninguno trae hecho para ti:

- Un envoltorio «`ev run -- lo-que-sea`» que resuelva el búfer y la codificación en Windows.
- Un almacén local que tus herramientas (MCP, Claude) puedan consultar sin montar un backend.

## 6. Decisiones de diseño

- **Núcleo propio en vez de montar uno existente.** Los recolectores existentes son tuberías hacia un backend: con cualquiera de ellos seguiría faltando el almacén, y tu RAM es justa (Sanctuary y la compilación de Cassandra ya chocaron con ella). La decisión es reversible: si la entrada acepta también OTLP, más adelante se puede poner un OTel Collector o un Fluent Bit delante sin tocar nada más.
- **Lenguaje: Rust** (a su «no me gusta Python», 2026-10-29). Da un `.exe` sin runtime y con poca RAM en marcha, que es lo que importa en un proceso que está siempre encendido. Llega directamente a ConPTY, a ETW y al registro de eventos (crates `windows` y `ferrisetw`) y trae SQLite con FTS5 incluido (`rusqlite` con `bundled`). Además, ya usas Rust en Spur y Cassandra (cargo 1.97 instalado). Riesgo: la memoria al compilar. Se controla con pocas dependencias, `--jobs 2` y el target en D:. Alternativa: Go 1.26 (también instalado). Compila en segundos y con poca memoria, y SQLite sin cgo (`modernc.org/sqlite`), pero gasta algo más de RAM en marcha y tiene un acceso a ETW menos maduro. Python queda fuera, salvo como programa vigilado: lo de `PYTHONUNBUFFERED` se refiere a los programas que se observan, no al recolector.
- **Almacén.** SQLite en modo WAL, en D: (C: está casi lleno), con FTS5 sobre `cuerpo`. Si crece demasiado, un fichero por mes.
- **Nunca bloquear la fuente.** Si el almacén va lento, el recolector escribe en un fichero intermedio y reintenta. Un programa no debe pararse porque el recolector no dé abasto.
- **Secretos.** Un recolector universal es también una fuga universal. Por eso necesita:
  - Redacción a la entrada (patrones de claves y tokens, cabeceras `Authorization`).
  - Una lista de exclusión (p. ej. `F:\claves*` y los perfiles de navegador).
  - Escuchar solo en 127.0.0.1.
  - Una retención configurable.
- **Bucles.** El recolector no debe recoger su propia salida: sus eventos llevan una marca y se descartan a la entrada.
- **Relojes.** Se guardan los dos. Tu reloj de Windows iba unos 0,92 s retrasado (comprobado durante Cassandra), así que, al mezclar fuentes de varias máquinas o de servicios externos, el orden por debajo del segundo no es fiable.

## 7. El bus y los arneses de agentes (ampliación del 2026-10-29)

Tu idea, en tus palabras: el sistema se engancha a cualquier arnés (Codex, opencode, Claude Code, Grok), se puede programar para que llame a esos modelos, y los modelos también publican eventos. Con esto queda contestada la pregunta de «consultar después o reaccionar»: hay que reaccionar. El almacén deja de ser el centro y pasa a ser la memoria de un bus.

Entre el bus y cada arnés hay tres caminos. Lo marcado como comprobado lo verifiqué hoy en tu equipo, con `--help` y en el código fuente.

| | Claude Code 2.1.312 | Codex 0.153.4 | opencode 1.18.35 | Grok Build 1.0.44 |
|---|---|---|---|---|
| **1. Lo que hace el agente pasa al bus** (automático) | Hooks: ya los usas | Hooks: existen, con un sistema de confianza (`--dangerously-bypass-hook-trust`) | Plugins con eventos (según su documentación, sin comprobar) | Hooks `PreToolUse`, `PostToolUse`, etc. (comprobado en `D:\dev\grok-build\crates`) |
| **2. El modelo publica eventos** (cuando él decide) | Cliente MCP | Cliente MCP (`codex mcp`) | Cliente MCP (`opencode mcp`) | Cliente MCP (`grok mcp`) |
| **3. El bus llama al modelo** (sin interfaz) | `claude -p --output-format stream-json`, `--session-id`, `--resume`, `--permission-mode`, `--strict-mcp-config` | `codex exec --json` (JSONL), `exec resume`, `--sandbox`, `--output-schema` | `opencode run --format json`, `--session`, `--attach` a un `opencode serve` | `grok -p --output-format streaming-json` (actualizaciones ACP), `--resume`, `--session-id`, `--json-schema` |

Consecuencias:

- **Un único servidor MCP del bus sirve para los cuatro arneses.** Con herramientas como `publicar`, `buscar` y `esperar`, cualquier modelo de cualquier arnés lee y escribe eventos sin adaptadores.
- **Al llamar al modelo, los eventos salen solos.** Los cuatro tienen salida JSON en modo sin interfaz, así que, cuando es el bus quien lanza al agente, cada paso (herramienta usada, mensaje, final, coste) se convierte en evento sin necesidad de hooks. Los hooks solo hacen falta para las sesiones que abres tú a mano.
- **Unificación posterior: ACP** (Agent Client Protocol, de Zed). opencode lo trae (`opencode acp`) y Grok Build también (su `streaming-json` es ACP). Claude Code y Codex lo tienen mediante adaptadores de Zed, que no he comprobado con tu suscripción. Para la v0 basta un lanzador por arnés; ACP es la vía si llegan a ser muchos.
- **Meter un evento en una sesión que ya está abierta** existe, pero en pruebas:
  - Los «channels» de Claude Code son una vista previa de investigación y no aparecen en el `--help` de la 2.1.312.
  - En opencode, `opencode serve` con `prompt_async`.
  - En Codex, `app-server`, todavía experimental.
  
  En la v0, el bus crea sesiones o retoma las existentes con `--resume`; no inyecta en sesiones abiertas.

## 8. Reglas: cómo se programa que el sistema llame a los modelos

Una regla dice: cuando llegue un evento con estas condiciones, llama a este arnés con este encargo y estos límites. Ejemplo (formato provisional):

```toml
[[regla]]
nombre      = "diagnosticar-pruebas-rotas"
cuando      = 'tipo = "proceso.fin" and atributos.codigo != 0 and fuente glob "proc:cargo test*"'
arnes       = "claude"          # claude | codex | opencode | grok
modelo      = "sonnet"
permisos    = "lectura"         # lectura | escritura-en-carpeta (nunca «saltar permisos» si la entrada no es de confianza)
encargo     = "Diagnostica por qué fallaron estas pruebas. Publica un evento «diagnostico»."
sesion      = "nueva"           # nueva | continuar:<nombre>
limites     = { a_la_vez = 1, por_hora = 6, profundidad = 2 }
```

También el paso del tiempo es un evento (`reloj.cada`), así que las tareas programadas son reglas como cualquier otra.

Riesgos que el diseño resuelve desde el primer día, no después:

- **Bucles y gasto.**
  - Cada evento lleva `causa`, el id del evento que lo provocó, y `profundidad`.
  - Lo que publica un agente lanzado por una regla hereda la profundidad + 1, y ninguna regla se dispara por encima de su límite.
  - Hay un tope por hora, una pausa entre disparos y una concurrencia global (1 o 2 agentes a la vez en tu portátil), además de `ev pausa` como interruptor general.
  - Tu memoria lo confirma: `claude -p` sin `--strict-mcp-config` carga unos 184.000 tokens de MCP en cada llamada. El lanzador de Claude pasará siempre `--strict-mcp-config` con solo el MCP del bus.
- **Inyección.** Si el bus recoge «cualquier cosa» (webs, foros, logs) y eso se convierte en el encargo de un agente con herramientas, cualquier texto de fuera puede intentar darle órdenes. Por eso:
  - Cada evento lleva su procedencia y un nivel de confianza.
  - Lo que no es de confianza entra en el encargo como dato entrecomillado, nunca como instrucción.
  - Las reglas alimentadas con fuentes no fiables solo pueden lanzar agentes de lectura: nada de `--always-approve`, `--auto` ni `--dangerously-*`.
- **Acciones con efecto** (enviar, publicar, pagar, borrar). Una regla no las ejecuta. Publica un evento `aprobacion.pendiente`, tú lo contestas y la respuesta es otro evento.
- **Responsabilidad.** Cada evento de un agente lleva el arnés, el modelo y el id de sesión. Así, la cadena causal «evento → regla → agente → lo que hizo → lo que publicó» se puede consultar entera.

## 9. Primer prototipo propuesto (v0), en Rust

1. **`ev` (servicio).** SQLite WAL + FTS5 en D:, `POST /eventos`, consulta por tiempo y texto, y un flujo en vivo (SSE) para quien quiera escuchar.
2. **`ev mcp`.** Servidor MCP por stdio con `publicar`, `buscar` y `esperar`, para los cuatro arneses.
3. **`ev run -- <orden>`** (con `PYTHONUNBUFFERED`, decodificación cp850 y código de salida) y **`ev hook`** (adaptador para los hooks de Claude Code).
4. **Motor de reglas** con un único lanzador, el de Claude (`claude -p`, solo lectura, con los límites del apartado 8). Codex, opencode y Grok llegan después: cada uno es otro lanzador pequeño, porque los cuatro tienen salida JSON.

Criterio de éxito de la v0:

- `ev run -- cargo test` con una prueba rota hace que la regla lance a Claude en modo lectura. Claude lee los eventos por MCP y publica un `diagnostico`, y una sola consulta muestra la cadena completa (fallo → regla → agente → diagnóstico).
- Una regla preparada para dispararse a sí misma se detiene en su límite de profundidad.

Fuera de la v0: los otros tres lanzadores, inyectar eventos en sesiones abiertas, ACP, `ev tail`, el registro de eventos y ETW de Windows, y varias máquinas.

## 10. Lo que queda abierto

- **¿Solo este portátil, o también Urus?** Con varias máquinas hace falta un transporte (OTLP o un bus tipo NATS, no evaluado) y un reloj común. Supuesto: solo este portátil.
- **Cuotas.** Claude va con el plan Max, sin API; las cuotas de Codex y Grok no las he mirado. El motor de reglas debe saber cuánto le queda a cada arnés antes de llamarlo.

## Fuentes consultadas (2026-10-29)

- CloudEvents: [cloudevents.io](https://cloudevents.io/), [graduación en la CNCF (InfoQ, 2024)](https://www.infoq.com/news/2024/04/cncf-cloudevents-graduation), [README de la especificación](https://cdn.jsdelivr.net/gh/cloudevents/spec@main/README.md)
- OTel Collector: [receptores](https://opentelemetry.io/ro/docs/collector/components/receiver/), [windowseventlog (OneUptime, 2026)](https://oneuptime.com/blog/post/2026-02-06-windows-event-log-receiver-opentelemetry-collector/markdown), [filelog (simplified.guide)](https://www.simplified.guide/_export/xhtml/opentelemetry/collector-collect-file-logs), [Elastic sobre filelog](https://www.elastic.co/observability-labs/blog/elastics-collaboration-opentelemetry-filelog-receiver)
- Fluent Bit: [anuncios de versiones](https://fluentbit.io/announcements/v5.0.6/), [winevtlog](https://docs.fluentbit.io/manual/pipeline/inputs/windows-event-log), [ciclo de vida (Versio.io)](https://www.versio.io/de/product-release-end-of-life-eol-fluent-bit-fluent-bit.html)
- Vector: [fuente exec](https://vector.dev/docs/reference/configuration/sources/exec/), [presentación de exec](https://vector.dev/highlights/2021-06-03-exec-source/)
- Redpanda Connect: [licencias](https://docs.redpanda.com/connect/get-started/licensing/), [fork Bento](https://www.warpstream.com/blog/announcing-bento-the-open-source-fork-of-the-project-formerly-known-as-benthos)
- Codex: [hooks (OpenAI)](https://developers.openai.com/codex/hooks), [hooks tras su versión estable (Vaughan, 2026)](https://codex.danielvaughan.com/2026/05/25/codex-cli-hooks-after-ga-event-model-trust-verification-production-patterns/), [eventos de `codex exec --json`](https://codex.danielvaughan.com/2026/04/08/codex-exec-jsonl-reference/)
- opencode: [servidor](https://www.mintlify.com/anomalyco/opencode/server), [plugins](https://v2.opencode.ai/docs/build/plugins), [CLI](https://opencode.ai/v2/docs/cli/commands/)
- Grok Build: [modo sin interfaz (xAI)](https://docs.x.ai//build/cli/headless-scripting), [órdenes](https://toolsbase.dev/en/reference/grok-build-commands), y su código en `D:\dev\grok-build`
- ACP: [zed.dev/acp](https://zed.dev/acp), [registro de ACP](https://zed.dev/blog/acp-registry), [ACP, MCP y A2A](https://www.morphllm.com/comparisons/acp-vs-mcp-vs-a2a)
- Channels de Claude Code: [referencia](https://code.claude.com/docs/en/channels-reference)
- VictoriaLogs: [benchmark frente a Loki (TrueFoundry, de parte)](https://www.truefoundry.com/es/blog/victorialogs-vs-loki), [anuncio de VictoriaMetrics](https://victoriametrics.com/blog/victorialogs-release/)
