[English](README.md) · [简体中文](README.zh-CN.md) · **Español** · [Português (BR)](README.pt-BR.md) · [Русский](README.ru.md) · [日本語](README.ja.md) · [Français](README.fr.md) · [Deutsch](README.de.md) · [Türkçe](README.tr.md)

> Esta es una traducción del README en inglés. En caso de discrepancia,
> prevalece la [versión en inglés](README.md).

# maskrun

Ejecuta comandos con los secretos del llavero de tu sistema operativo — y
mantén los valores fuera del contexto de tu agente de programación con IA.

```bash
maskrun put myapp-database-url        # guardado en el llavero, nunca en disco
maskrun run -- npm run dev            # inyectado en el proceso hijo, enmascarado en la salida
```

```
$ maskrun run -- node -e 'console.log(process.env.DATABASE_URL)'
<masked:DATABASE_URL>
```

Un solo binario, sin dependencias en tiempo de ejecución, sin demonio. Linux,
macOS y Windows.

---

## El problema

Mover secretos de los archivos `.env` al llavero del sistema es la mitad fácil.
La mitad difícil aparece en cuanto un agente de IA maneja tu shell.

Inyectar un secreto en un proceso hijo impide que el agente *lea la caja
fuerte*. No hace nada contra el valor que *sale de vuelta del proceso*:

- un servidor de desarrollo imprime su cadena de conexión al arrancar
- `curl -v` repite la cabecera `Authorization`
- una traza de pila lleva el DSN
- `psql` cita la URL a la que no pudo conectarse

Cualquiera de esos casos pone el secreto en la transcripción, donde ya forma
parte de la conversación, del historial del terminal y de dondequiera que se
almacene esa transcripción.

maskrun cierra ese camino, y los que están a su lado.

## Instalación

**Linux / macOS** — descarga un binario precompilado, verifica su suma de
comprobación, sin compilador:

```bash
curl -fsSL https://raw.githubusercontent.com/furkanakyol/maskrun/main/install.sh | sh
```

**Windows (PowerShell)**

```powershell
irm https://raw.githubusercontent.com/furkanakyol/maskrun/main/install.ps1 | iex
```

**Con una toolchain de Rust:**

```bash
cargo install maskrun
```

**Desde el código fuente:**

```bash
git clone https://github.com/furkanakyol/maskrun
cd maskrun && cargo build --release
# binario en target/release/maskrun
```

## Inicio rápido

```bash
# 1. mueve un .env existente al llavero — mira antes de saltar
maskrun import .env --dry-run
maskrun import .env

# 2. comprueba que cada nombre que pide el manifiesto está realmente presente
maskrun status

# 3. ejecuta tu aplicación sin ningún .env en disco
maskrun run -- npm run dev

# solo ahora borra el .env
```

`import` escribe un manifiesto `.maskrun` junto a tu código:

```
DATABASE_URL=myapp-database-url
JWT_SECRET=myapp-jwt-secret
```

**Ese archivo contiene nombres, no valores. Súbelo al repositorio.** Es el
`.env.example` que de verdad hace algo: `maskrun status` le dice a alguien
recién llegado al equipo exactamente qué secretos le faltan.

## Cómo funciona

Tres mecanismos, para tres necesidades distintas.

| Necesidad | Mecanismo | Lo que ve el agente |
|---|---|---|
| Un servidor de desarrollo, una migración o una CLI que necesita `DATABASE_URL` | `maskrun run` inyecta en el proceso hijo | salida enmascarada (`<masked:VAR>`) |
| El valor en sí — rotar una clave, pegarla en un panel | tú, en tu propio terminal | nada; el freno lo rechaza |
| Saber *qué* secretos existen | `maskrun list`, `maskrun status` | solo nombres, nunca valores |

### Inyección, no lectura

Los valores viven en el llavero. `maskrun run` resuelve el manifiesto, entrega
los valores a exactamente un proceso hijo y no guarda nada en disco. Si falta
algún secreto, se niega a arrancar en lugar de levantar tu aplicación con medio
entorno.

### Enmascarado de la salida

`run` y `exec` pasan la stdout y la stderr del hijo por un filtro que sustituye
los valores de los secretos por `<masked:VAR>`.

- Los valores llegan al filtro a través de **su entorno**, nunca por argv —
  argv se puede leer con `/proc/<pid>/cmdline`.
- Enmascara el valor en crudo más sus formas en base64, codificada para URL y
  escapada con barras invertidas.
- Detecta un valor **partido entre dos escrituras**, de modo que un secreto que
  quede a caballo de un vaciado de búfer no se cuele.
- Deja pasar de inmediato las líneas completas, para que la salida de un
  servidor de desarrollo no se quede en el búfer mientras la miras.
- Se niega a enmascarar valores de menos de 6 caracteres, y lo dice: enmascarar
  `5432` corrompería cualquier número sin relación en la salida.
- El código de salida, stdin y Ctrl-C llegan al hijo sin cambios.

Activo por defecto en una sesión de agente de IA y siempre que stdout no sea un
terminal; desactivado en tu propio terminal interactivo para que sobrevivan los
colores. `--raw` lo fuerza a apagado, `--mask` lo fuerza a encendido.

### El freno del agente

```bash
maskrun install-guard        # registra un hook PreToolUse para Claude Code
```

Lo importante de un hook es que **lo aplica el harness, no el modelo**. Una
regla escrita en un prompt la aplica el modelo, así que el modelo puede
convencerse a sí mismo de saltársela. Esto no se puede esquivar hablando.

Rechaza los comandos que pondrían un valor en la transcripción:

- `maskrun get`, `maskrun import`
- `secret-tool lookup/search`, `security find-generic-password`
- `--raw`, `MASKRUN_MASK=0`, `MASKRUN_ALLOW_READ=1` (desactivar el enmascarado)
- `/proc/<pid>/environ`
- un `env` / `printenv` pelado, `Get-ChildItem Env:`
- leer un `.env` / `.envrc`, ya sea por Bash o con las herramientas de archivos
  del agente

Y deja en paz el trabajo normal: `maskrun run`, `env VAR=x cmd`,
`cat .env.example`, `cat .maskrun`, `printenv PATH`, `ls -la .env`, `rm .env`.

`maskrun install-guard` se fusiona con tu `settings.json` existente, hace antes
una copia de seguridad, es idempotente, se niega a tocar un archivo que no sea
JSON válido, y `--remove` lo deshace. Otros harnesses: ejecuta `maskrun hook`
como hook previo a la herramienta, que recibe la llamada como JSON por stdin —
ver [integrations/claude-code](integrations/claude-code/).

La CLI aplica ella misma los mismos rechazos, así que un agente que corra en un
harness sin soporte de hooks tampoco puede hacer `maskrun get`.

### Guiar a un agente que no tiene mecanismo de hooks

```bash
maskrun install-rules        # escribe un bloque corto en AGENTS.md/CLAUDE.md/reglas .cursor
```

`install-guard` solo funciona donde el harness ejecuta un hook PreToolUse por
ti. En otros sitios no hay nada que aplique nada — así que `install-rules`
escribe un bloque corto y marcado en el archivo de `AGENTS.md`, `CLAUDE.md` o
`.cursor/rules/` que ya exista en el proyecto (creando `AGENTS.md` si no existe
ninguno), explicándole al agente cómo usar maskrun aquí. **Esto es orientación,
no cumplimiento forzado**: un modelo puede ignorarlo igual que puede ignorar
cualquier otra instrucción. Es un recurso para harnesses a los que
`install-guard` no llega, no un sustituto suyo.

El bloque va delimitado por las marcas
`<!-- maskrun:start -->`/`<!-- maskrun:end -->`, hace primero una copia de
seguridad del archivo, es idempotente (una segunda ejecución lo actualiza en el
sitio en vez de duplicarlo) y `--remove` lo retira sin tocar el resto del
archivo. Con un manifiesto `.maskrun` presente, el bloque lista los nombres de
variable reales que el proyecto espera; `--file <path>` apunta directamente a
un archivo y se salta la búsqueda.

<a id="interactive-view"></a>

### La vista interactiva

```bash
maskrun            # en tu propio terminal, sin argumentos
```

Un terminal de verdad sin nada más en la línea de comandos abre una vista
navegable con las flechas: las plataformas a la izquierda, y a la derecha los
secretos de esa plataforma y el detalle del resaltado (plataforma, nota, valor).

```
↑↓ move   → enter   ← back   e edit   d delete   v reveal   y copy   q quit
```

Los valores están enmascarados (`••••••••••••••••`) hasta que pulsas `v`, y un
valor solo se lee del llavero en ese preciso momento — moverse por la lista no
lo toca nunca. Pasar a otro secreto vuelve a enmascarar automáticamente. `e`
edita la plataforma, la nota y el valor en el sitio (dejarlo en blanco conserva
lo actual); `d` pide confirmación por el nombre (`delete 'name'? [y/N]`) y solo
una `y`/`Y` literal continúa — cualquier otra tecla, Enter incluido, cancela.
`y` copia el valor a tu portapapeles.

Funciona sobre un [búfer de pantalla
alternativo](https://en.wikipedia.org/wiki/Terminal_emulator#Alternate_screen_buffer):
nada de lo que dibuja — ni siquiera un valor revelado — acaba nunca en el
historial de tu terminal, a diferencia de la salida actual de `maskrun get`.
Esto es una mejora real, con independencia de todo lo demás que sigue.

Como cualquier otro camino que toque valores, **nunca se abre en una sesión de
agente de IA** — con la salida redirigida o sin ella, una sesión de agente
detectada recibe siempre el mismo resumen solo-nombres que imprime un `maskrun`
pelado en modo no interactivo. También se niega en un terminal menor de 60x15,
y te dice por qué antes de recurrir a ese resumen.

#### Copia al portapapeles

`y` copia el secreto actual a tu portapapeles, porque revelar un valor que
luego no puedes pegar en ningún sitio solo traslada el problema — lo volverías
a teclear o lo seleccionarías con el ratón, ambas cosas peores. Tres límites
incorporados:

- **Se borra solo a los 45 segundos.** La TUI muestra una cuenta atrás en vivo
  en cuanto hay algo copiado; `c` lo borra al instante en lugar de esperar, y
  salir de la TUI con algo todavía copiado también lo borra. Al borrar se
  restaura lo que hubiera en el portapapeles antes de la copia, o se vacía si
  no había nada.
- **Se envía una pista de «no registrar» en todas las plataformas**, mediante
  el `exclude_from_history` de arboard: el tipo MIME
  `x-kde-passwordManagerHint` de KDE en Linux, la convención comunitaria
  `org.nspasteboard.ConcealedType` en macOS y el formato de portapapeles nativo
  `CanIncludeInClipboardHistory` en Windows. Verificado aquí contra un Klipper
  real: una copia normal aparece en su historial, una etiquetada no, y Klipper
  ni siquiera informa de la etiquetada como contenido *actual* del
  portapapeles. Cada una es una pista que una herramienta concreta decide
  respetar, no algo que maskrun imponga — ver más abajo.
- **Nunca se abre en una sesión de agente**, la misma puerta que el resto de la
  TUI.

**Lo que esto no arregla:** cualquier gestor de historial de portapapeles que no
busque esa pista (el de GNOME, la mayoría de configuraciones Wayland fuera de
KDE y — observado directamente al construir esto — hasta el propio Klipper de
KDE cuando no está vigilando el transporte de portapapeles que usa tu
compositor) sigue registrando el valor de forma permanente, y el borrado
automático a los 45 segundos no hace nada contra esa copia una vez está en un
archivo de historial. Trata la copia al portapapeles igual que
`/proc/<pid>/environ` y que un humano pegando a mano un valor sin enmascarar,
más abajo: un límite real, no un problema resuelto.

## Lo que esto no es

**maskrun no es una frontera de seguridad.** Es endurecimiento frente a
accidentes, y no debería vendértelo nadie — ni deberías venderlo tú — como algo
más.

- **Un llavero resuelve el almacenamiento, no el acceso.** Cualquier proceso que
  corra con tu usuario puede llamar a `secret-tool lookup` o a
  `security find-generic-password`, tu agente incluido. El freno encarece
  hacerlo por accidente; no lo hace imposible. Contigo pasa lo mismo: si una
  persona pega a mano un valor sin enmascarar en la transcripción, ninguna
  herramienta posterior a esa pulsación puede atraparlo.
- **Código que el freno no puede leer.** Los cuerpos de heredoc y los archivos
  de script se tratan como datos, no como comandos — deliberadamente, porque
  analizarlos producía falsos positivos. Así que un script que lee `.env` él
  mismo desde su propio código pasa. Un buscador de patrones nunca atrapa
  código arbitrario.
- **Entorno del proceso.** Mientras `maskrun run` está en marcha, el entorno de
  su proceso hijo se puede leer con `/proc/<pid>/environ`. El freno bloquea ese
  camino directamente, pero la inyección por entorno tiene esta forma por
  diseño.
- **El borrado automático no deshace la copia al portapapeles (`y` en la vista
  interactiva).** El plazo de 45 segundos vacía *el portapapeles*; no hace nada
  contra un gestor de historial que ya haya registrado el valor de forma
  permanente antes de que venciera. maskrun etiqueta la copia para que el
  Klipper de KDE la omita — algo real y verificado, pero no todas las
  herramientas que guardan historial respetan esa etiqueta, y no se sabe que lo
  hagan el historial de portapapeles de GNOME, la mayoría de configuraciones
  Wayland fuera de KDE ni el Historial del Portapapeles de Windows. En el mismo
  estante que `/proc/<pid>/environ` arriba y que el humano pegando a mano un
  valor sin enmascarar abajo: un límite dicho con claridad, no resuelto en
  silencio.
- **`ps` durante una escritura — cerrado.** En macOS, el valor llegaba a
  `security` como argumento de línea de comandos, brevemente visible en la
  lista de procesos con `ps`. Ese camino ha desaparecido: maskrun ahora llama
  directamente a la API generic-password de Security.framework, así que el
  valor nunca llega a ser un argv que el sistema tenga que exponer a nadie.
- **Coincidencia por segmento de tubería, no un analizador de shell.** El freno
  evalúa cada segmento de una tubería por separado, y eso es lo que deja pasar
  `sed 's/maskrun get/x/' notes.md` — ese texto nunca invoca `maskrun get`,
  solo lo menciona. Ese mismo alcance implica que el freno no sigue un comando
  a través de una indirección: `echo 'maskrun get x' | sh` se lee como un
  `echo`, no como el comando que `sh` acaba ejecutando.

Si quieres una frontera de verdad, el shell del agente tiene que correr en algún
sitio que no pueda alcanzar el llavero en absoluto — sin socket de sesión D-Bus
en Linux, en una cuenta de usuario separada o en un contenedor. Entonces
maskrun funciona desde tu terminal y no desde el del agente. El radio de daño se
gestiona mejor en el proveedor: una clave distinta por herramienta, límites de
gasto, rotación y credenciales de vida corta donde existan.

## Backends

| Plataforma | Nombre del backend | Almacenamiento |
|---|---|---|
| Linux | `secret-service` | el Secret Service de libsecret, directamente por D-Bus (crate `dbus-secret-service`) — gnome-keyring, KWallet, KeePassXC |
| macOS | `keychain` | llavero de inicio de sesión mediante la API generic-password de Security.framework (crate `security-framework`) |
| Windows | `dpapi` | Administrador de credenciales de Windows (crate `windows`, `Win32_Security_Credentials`) — el nombre se conserva del antiguo backend de archivos DPAPI por compatibilidad con la variable de anulación; el almacenamiento que hay debajo ya no son archivos DPAPI |

Las versiones anteriores lanzaban `secret-tool` en Linux y `security` en macOS
para cada operación, y en Windows implementaban a mano el almacenamiento en
archivos DPAPI. Los tres pasan ahora por un enlace de biblioteca a la API de la
plataforma para `put`/`get`/`delete`, en lugar de por un subproceso o
criptografía escrita a mano — en Linux no hace falta instalar `secret-tool`, y
en macOS el valor del secreto ya no se convierte en argumento de línea de
comandos. (En macOS, `list` sigue lanzando `security dump-keychain` para
enumerar nombres — esa llamada no toma ningún valor de secreto como argumento,
así que no se expone nada nuevo; `security-framework` no tiene una llamada de
enumeración acotada al servicio con la que sustituirla.)

El almacenamiento en Linux conserva el esquema que espera
`secret-tool lookup service maskrun name <name>` (atributos `service` +
`name`), de modo que un secreto guardado por maskrun se sigue pudiendo leer con
la CLI estándar si necesitas comprobarlo a mano — simplemente maskrun ya no
depende de que esa CLI esté instalada.

### Dónde viven la etiqueta de plataforma y la nota

| Plataforma | Dónde |
|---|---|
| Linux | Dos atributos extra de Secret Service, `platform` y `note`, junto a los `service`/`name` existentes — `secret-tool lookup` solo casa con los atributos que le des, así que estos viajan de acompañantes sin afectar a nada que ya lea `service`+`name`. |
| macOS | Los campos `kSecAttrDescription` (plataforma) y `kSecAttrComment` (nota) de la generic password, escritos y leídos mediante las API de búsqueda por atributos y de actualización solo-atributos de `security-framework` — el valor guardado nunca se toca al reetiquetar. |
| Windows | Ambos codificados en el único campo `CREDENTIALW.Comment` que ofrece el Administrador de credenciales (`platform=<p><US>note=<n>`, `<US>` = U+001F): aquí no hay un almacén de atributos aparte, y reetiquetar obliga a reescribir la entrada entera (valor incluido) porque `CredWriteW` no tiene una llamada de actualización parcial. |

Anula la detección con `MASKRUN_BACKEND=secret-service|keychain|dpapi`.

### Qué está verificado de verdad

Los tres backends se ejercitan contra un llavero real en CI: Secret Service en
Linux, Keychain en macOS, Administrador de credenciales en Windows. Las pruebas
del llavero guardan y recuperan un secreto real en vez de simular el backend, y
un trabajo sin ningún llavero instalado demuestra que el freno sigue
respondiendo.

Con la etiqueta de plataforma y la nota pasa lo mismo: ejercitadas contra un
Secret Service real en Linux (incluida la comprobación de compatibilidad de
`secret-tool` con el esquema de atributos), verificadas de tipos en compilación
cruzada para macOS y Windows igual que el resto del código de plataforma, pero
ejecutadas de verdad contra un Keychain o un Administrador de credenciales
reales solo cuando corran esos trabajos de CI.

La primera ejecución de CI que llegó a arrancar encontró tres fallos reales en
el código de plataforma, todos en rutas que una compilación de Linux nunca
verifica de tipos porque están detrás de `cfg`: dos argumentos de Win32 mal
tipados, una llamada a `CredEnumerateW` que pasaba a la vez un filtro y el flag
de todas-las-credenciales (inválidos juntos), y una comparación de
`ERROR_NOT_FOUND` contra el código Win32 en crudo en lugar del `HRESULT` que
llega realmente — lo que hacía que un secreto ausente y un almacén vacío
salieran ambos como un error en crudo. El trabajo de lint ahora comprueba en
cruzado los objetivos de Windows y macOS desde Linux, para que esa clase de
error no pueda volver a llegar a un runner de plataforma.

Lo que *no* está cubierto: las pruebas de manejo del bloqueo son opcionales
(`MASKRUN_LOCK_TESTS=1`), porque lanzar un `gnome-keyring-daemon` desechable en
un escritorio real le pide al usuario crear un llavero y sobrevive a la prueba.

## Comandos

```
maskrun put <name>                    store a secret (prompts; not echoed)
maskrun put                           fully interactive: asks name, platform, note, value
maskrun put <name> --stdin            store from stdin (for scripts)
maskrun put <name> --for X --note Y   tag it with a platform and a note while storing
maskrun label <name> --for X          tag (or retag) an existing secret's platform
maskrun label <name> --for ""         clear its platform
maskrun get <name>                    print it (refused in an agent session)
maskrun list                          grouped by platform, with notes, never values
maskrun list --plain                  flat, sorted names only, one per line (for scripts)
maskrun rm <name>                     delete (no name: pick from a numbered list)
maskrun status                        check the manifest against the keyring
maskrun run [--raw|--mask] -- CMD     run with the manifest injected
maskrun exec VAR=name -- CMD          run with explicit pairs, no manifest
maskrun -- CMD                        shorthand for run
maskrun import <.env> [--dry-run]     move a .env into the keyring
maskrun completions <fish|bash|zsh>   print a shell completion script
maskrun install-guard [--remove]      register the agent guard
maskrun hook                          the guard itself (reads JSON on stdin)
maskrun install-rules [--remove]      tell agents how to use maskrun here (guidance, not enforcement)
```

`import` omite las variables `VITE_`, `NEXT_PUBLIC_`, `PUBLIC_`, `REACT_APP_`,
`NUXT_PUBLIC_`, `EXPO_PUBLIC_` y `GATSBY_`: se compilan en tu bundle de cliente
y se envían a todos los visitantes, así que son configuración y no secretos, y
moverlas no aporta nada. `--all` lo anula.

### Etiquetas de plataforma y notas

Pasada una media docena de secretos, un nombre plano no basta para recordar
para qué sirve cada uno — sobre todo cuando una plataforma tiene más de una
clave y lo que las distingue es el alcance, no el nombre. `--for` etiqueta un
secreto con una plataforma o servicio; `--note` añade una descripción corta de
texto libre:

```
$ maskrun put gh-release-token --for github --note "fine-grained, contents+plan"
stored: gh-release-token

$ maskrun label vault-token-github --for github --note "fine-grained, repo+plan"
labelled: vault-token-github

$ maskrun list
github
  gh-release-token       fine-grained, contents+plan
  vault-token-github     fine-grained, repo+plan
(no platform)
  pos-terminal-electron-api-url
```

Ambas opciones son opcionales y se aplican a secretos que todavía no tienen
ninguna de las dos. `label` reetiqueta a posteriori un secreto existente; en
cualquiera de los dos comandos, dar `--for ""` (o `--note ""`) vacía ese campo,
y omitir del todo una opción deja intacto el valor existente — hacer `put`
sobre el valor de un secreto nunca pierde su etiqueta en silencio. **La
plataforma y la nota no son secretos**: se guardan sin enmascarar, son visibles
en una sesión de agente de IA y las muestran `list` y `status`. No pongas un
valor secreto en `--note`; está limitado a 200 caracteres y no puede contener
saltos de línea.

`maskrun` sin argumentos abre [la vista interactiva](#interactive-view) en un
terminal de verdad; redirigido (`maskrun | cat`), en modo no interactivo o en
una sesión de agente, imprime en su lugar un resumen breve: el estado del
manifiesto y la lista agrupada de secretos de arriba, para que no tengas que
llevar toda la superficie de comandos en la cabeza.

### Autocompletado de shell

```bash
maskrun completions fish > ~/.config/fish/completions/maskrun.fish
maskrun completions bash > ~/.local/share/bash-completion/completions/maskrun
maskrun completions zsh > ~/.zfunc/_maskrun   # luego `fpath+=~/.zfunc` antes de compinit
```

Más allá de las opciones y los subcomandos, `maskrun get`/`rm`/`label`
completan nombres de secretos reales — fish lo hace de serie (mediante ese
mismo `maskrun list --plain` para el que existe esta opción); bash y zsh solo
reciben los completados estáticos.

## Variables de entorno

| Variable | Efecto |
|---|---|
| `MASKRUN_MASK` | `1` enmascarar siempre, `0` no enmascarar nunca |
| `MASKRUN_AGENT` | `1` tratar esto como sesión de agente, `0` tratarlo como humano |
| `MASKRUN_BACKEND` | forzar un backend |
| `MASKRUN_ALLOW_READ` | `1` reactiva `get`/`import` en una sesión de agente |

Las sesiones de agente se detectan por `CLAUDECODE`, `CLAUDE_CODE_ENTRYPOINT`,
`AI_AGENT`, `AIDER_CHAT`, `CURSOR_AGENT`, `OPENAI_CODEX`, `GEMINI_CLI` y
`REPLIT_AGENT`. Para cualquier otro caso, define `MASKRUN_AGENT=1` en el
harness.

## Plataformas soportadas

- **Linux** — glibc 2.35 o posterior (los binarios de release se compilan en
  Ubuntu 22.04). Probado en CI contra un Secret Service en vivo.
- **macOS** — Intel y Apple Silicon. Probado en CI contra un Keychain real.
- **Windows** — x86_64. Probado en CI contra el Administrador de credenciales
  real.

## Trabajo previo

[`envchain`](https://github.com/sorah/envchain) puso los secretos en el llavero
y los inyectó en el entorno hace años, y es el antepasado directo de la mitad
`run` de esta herramienta. [`direnv`](https://direnv.net/) gestiona entornos por
directorio, [`sops`](https://github.com/getsops/sops) y
[`dotenvx`](https://github.com/dotenvx/dotenvx) cifran secretos en reposo dentro
del repositorio, y [`aws-vault`](https://github.com/99designs/aws-vault) hace el
baile del llavero para un proveedor.

Lo que añade maskrun es la mitad que mira hacia el agente: enmascarar la salida
del proceso hijo, y un freno aplicado por el harness sobre los comandos que
filtrarían un valor. Si no trabajas con un agente de IA, puede que `envchain`
te baste.

## Desarrollo

```bash
cargo test -- --test-threads=1   # un solo hilo: los backends comparten el estado real del llavero
cargo clippy --all-targets -- -D warnings
cargo fmt --check
make test                        # la misma tanda de pruebas
make lint                        # fmt --check + clippy
```

Las pruebas del llavero se saltan a sí mismas cuando no hay ningún backend
alcanzable, para que las pruebas del freno sigan corriendo en un contenedor
pelado.

Los valores secretos de la suite de pruebas se generan al azar y nunca se
imprimen — las aserciones comprueban ausencia o presencia, nunca igualdad
contra un valor registrado.

Las contribuciones son bienvenidas. Añadir un backend significa implementar
`put`/`get`/`delete`/`list` del trait `Backend`; añadir un harness significa una
entrada bajo `integrations/`.

## Licencia

Copyright (C) 2026 Furkan Akyol.

maskrun es software libre: puedes redistribuirlo y modificarlo bajo los términos
de la Licencia Pública General de GNU, versión 3 o cualquier versión posterior,
publicada por la Free Software Foundation. Se entrega sin garantía alguna.
Consulta [LICENSE](LICENSE) para ver los términos completos.

Una consecuencia práctica: si distribuyes un maskrun modificado — como código
fuente, como binario o dentro de un producto — debes poner tu código fuente
modificado a disposición de aquellos a quienes se lo distribuyas, bajo esta
misma licencia.
