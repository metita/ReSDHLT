# ReSDHLT GUI

Compilador de mapas para CS 1.6 con interfaz gráfica, tema oscuro, y una
explicación de qué hace cada opción al pasar el mouse por encima.

Hecho en Rust con [egui](https://github.com/emilk/egui). Un solo ejecutable, sin
instalador ni runtime.

---

## Estado

Compila y corre con `eframe 0.28.1` en Windows, sin warnings.

Si tocas algo y no compila, los errores de Rust son precisos: casi siempre es un
tipo o un nombre de método.

---

## Compilar

Necesitás Rust: https://rustup.rs

```powershell
cd gui
cargo run --release
```

El ejecutable queda en `gui/target/release/resdhlt-gui.exe`.

En Linux, egui necesita las librerías de desarrollo de X11/Wayland:

```sh
sudo apt install libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev \
                 libxkbcommon-dev libssl-dev
```

## Usar

1. **Mapa**: el `.map` que exporta tu editor. También puedes arrastrarlo sobre
   la ventana.
2. **Herramientas**: la carpeta con `sdHLCSG`, `sdHLBSP`, `sdHLVIS`, `sdHLRAD`.
   Si compilaste este repo, es `tools/`. La GUI la detecta sola al arrancar si
   está junto al ejecutable.
3. **Carpeta de salida** (opcional pero recomendada): dónde quieres el `.bsp`.
4. Elige un preset y pulsa **Compilar** (o `F5`; `Esc` cancela).

### Proyectos

Un proyecto es un nombre (`zm_hola`) con su `.map`, sus carpetas y **todas** las
opciones de las demás pestañas. Se abre y queda listo para compilar.

- **Guardar como proyecto** toma lo que tengas cargado en ese momento. Si dejas
  el nombre vacío, usa el del mapa: `zm_hola.map` → `zm_hola`.
- Doble click en la lista lo abre; también está el botón **Abrir**.
- **Renombrar**, **Duplicar**, **Borrar** (con confirmación) y **Actualizar con
  lo actual**. Borrar quita el proyecto de la lista y no toca ningún archivo.
- **Se guarda solo.** Cualquier cambio de opción con un proyecto abierto se
  escribe en él; el archivo se actualiza poco después del último cambio, así que
  arrastrar un slider es una escritura y no cientos. También se guarda al
  compilar y al cerrar. La cabecera muestra el proyecto abierto y si está
  guardado.
- Al abrir la GUI vuelve el último proyecto que usaste.
- La lista muestra los proyectos en varias columnas, sin scroll propio: la
  página entera scrollea. Click elige, doble click abre, y cada fila tiene su
  botón **Abrir**. Con más de 8 proyectos aparece un filtro por nombre o mapa.
- La pestaña usa el ancho completo de la ventana: el panel del log se oculta
  ahí, porque no tiene nada que decir mientras administrás proyectos.
- **Carpeta del proyecto**: lista lo que hay ahí ahora mismo — el `.bsp`, y el
  contenido de `intermedios/` — con tamaño y antigüedad. Click elige, doble
  click abre, y **click derecho** da Abrir, Mostrar en la carpeta, Copiar ruta,
  Renombrar y Borrar (con confirmación aparte, y aviso especial si el archivo
  es el `.map` fuente del proyecto).
- **Limpiar intermedios** borra `.p0`-`.p3`, `.lin`, `.pts`, `.wa_`, `.ext` y
  `.max` sin tocar el `.map`, el `.bsp` ni los logs.

Todo vive en `resdhlt-projects.json`, junto al ejecutable y separado de
`resdhlt-gui.json`, que sigue guardando las opciones sueltas de siempre.

### Actualizaciones

La GUI se actualiza sola desde las **releases** de
`github.com/metita/ReSDHLT`. Nunca desde los commits: mientras no publiques un
tag, nadie ve una actualización, por más que empujes cien veces a master.

- Comprueba **una vez por día** al abrir, en segundo plano. Se puede apagar
  desde el menú **Actualizaciones**, abajo a la derecha, que además tiene
  "Buscar ahora" y el link a las releases.
- Cuando hay una, aparece un aviso verde en la cabecera. Al abrirlo se ven la
  versión, el tamaño de la descarga y las notas de la release; nada se descarga
  hasta que pulses **Actualizar ahora**.
- Al aceptar: se baja el `.zip`, se comprueba que el tamaño coincida con lo que
  declara la release, se descomprime, y un script reemplaza el ejecutable y la
  carpeta `tools` cuando la GUI ya cerró (Windows no deja pisar un `.exe` en
  uso). Después la vuelve a abrir sola.
- **Tus proyectos y preferencias no se tocan**: `resdhlt-projects.json` y
  `resdhlt-gui.json` no están en el paquete. Si algo falla queda un
  `resdhlt-update.log` junto al ejecutable.

No hay dependencias nuevas para esto: usa el `curl.exe` que trae Windows 10+ y
el `serde_json` que ya estaba. Las descargas se aceptan sólo si la URL es de
`github.com`. **No hay verificación de firma**: la confianza es la misma que
tienes en tu cuenta de GitHub.

#### Publicar una versión

```powershell
# 1. subí la versión en gui/Cargo.toml (el workflow verifica que coincida)
# 2. commit y tag
git tag v0.2.0
git push origin v0.2.0
```

`.github/workflows/release.yml` compila las tools con CMake y la GUI con cargo,
corre los tests, arma `ReSDHLT-Windows-x64.zip` con la misma estructura de
siempre y publica la release. Si al paquete le falta algo esencial
(`resdhlt-gui.exe`, las cuatro tools o `sdhlt.wad`) el workflow falla en vez de
publicar algo roto.

### Interfaz

- **Arrastrar y soltar**: un `.map` va al campo de mapa; una carpeta va a
  herramientas, WADs o salida según lo que contenga.
- **WADs automáticos**: las rutas rotas de la lista del mapa se reubican por
  nombre de archivo. Ver más abajo.
- **Puntos de estado** verde/rojo en cada ruta: sabes si existe antes de
  compilar, y el botón Compilar solo se habilita cuando el `.map` y las
  herramientas son válidos.
- **Responsive**: en ventanas anchas el log va a la derecha; en angostas pasa
  abajo. Las columnas de opciones se recalculan con el ancho, y el contenido se
  centra con un ancho máximo para que no se estire en pantallas 4K.
- **Escala** con `A+` / `A-` (se guarda en el perfil), para monitores de alta
  densidad.
- **Log** con filtro de texto, modo "solo problemas", contador de errores y
  avisos, y botones de copiar y guardar a archivo.
- **Barra de progreso** con el tiempo en vivo de la etapa en curso y el total.
- **Línea de comandos** exacta visible en la pestaña Compilar, para reproducir
  el compilado desde un script.

Las preferencias se guardan solas al cerrar la ventana y al empezar cada
compilación, en un `resdhlt-gui.json` junto al ejecutable. Si trabajas en varios
mapas, la pestaña **Proyectos** te evita reconfigurar cada vez.

### Carpeta de salida

Si la dejas vacía, las herramientas escriben junto al `.map`: el `.bsp`, el
`.prt`, los logs y los intermedios `.p0`-`.p3` se mezclan con tus fuentes.

Si la indicas, el `.map` se copia ahí y se compila esa copia, así que todo lo
generado queda en esa carpeta. **El `.map` original nunca se modifica.**

Con **Carpeta por proyecto** (activado por defecto), esa carpeta no se llena de
archivos sueltos. Apuntando la salida a `Escritorio\Mapas`:

```
Escritorio\Mapas└── zm_hola\                     ← el nombre del proyecto
    ├── zm_hola.bsp              ← el resultado, solo
    └── intermedios        ├── zm_hola.log, .prt, .ext, .wa_, .p0-.p3, .lin, .pts
        ├── zm_hola.map           (la copia que se compiló)
        └── resdhlt-wads.cfg
```

El compilado corre dentro de `intermedios`, así que todo lo que ensucian las
herramientas queda ahí; al terminar bien, el `.bsp` se mueve un nivel arriba.
Podés apuntar diez mapas a la misma carpeta de salida sin que se mezclen.

### Carpeta de WADs y "WADs automáticos"

Un `.map` guarda las rutas absolutas de los WADs de la máquina donde se hizo:

```
"wad" "/Program Files (x86)/Steam/.../valve/zhlt.wad;/Users/Admin/Documents/Mapping/Mapas/ar_azteca/ar_azteca.wad"
```

Esas rutas casi nunca sirven en otra PC. Peor todavía cuando no tienen letra de
unidad, como arriba: Windows las resuelve contra la unidad actual, así que un
mapa guardado en `E:` busca sus WADs en `E:\Users\...`. CSG entonces muere con
`Could not open wad file`.

**WADs automáticos** (activado por defecto) resuelve la lista antes de compilar:

1. Cada entrada de la clave `wad` se verifica. Las que existen se dejan como
   están.
2. Las rotas se buscan **por nombre de archivo** en la carpeta de WADs (hasta 3
   niveles de subcarpetas), junto al `.map` y en la carpeta de herramientas.
3. Siempre se agrega `sdhlt.wad`.
4. Si con eso siguen faltando texturas, se leen las que el mapa usa realmente
   (están en cada cara del `.map`) y se abre el directorio de cada `.wad` de tus
   carpetas para ver cuál las tiene. Se eligen los que aporten texturas que
   falten, del que más aporta al que menos, y se para en cuanto no falta
   ninguna. Un `.wad` que no aporta nada no se carga aunque esté al lado del
   mapa.
5. La lista se recorta a 500 entradas. CSG aborta con *"too many wad files"* al
   pasarse de `MAX_WADPATHS`, que este fork subió de 128 a 512 justamente
   porque 128 se alcanza sin querer con una biblioteca de texturas grande.
6. La lista se escribe en `resdhlt-wads.cfg` junto al mapa y se le pasa a CSG
   como `-wadcfgfile`, que hace que **ignore la clave del mapa**.

Sólo se leen la cabecera y el directorio de lumps de cada WAD, así que una
carpeta con mil WADs se revisa en menos de un segundo, y sólo cuando hace falta.

Tu `.map` no se modifica, no hace falta carpeta de salida, y lo que no aparezca
se avisa en el log en vez de matar el compilado.

A **RAD** la carpeta de WADs se le pasa aparte como `-waddir`.

## Los presets

| Preset | Para qué |
|---|---|
| **Borrador** | Ver si el mapa carga y no tiene leaks. VIS rápido, RAD rápido, 1 bounce. La luz y los FPS no son representativos. |
| **Recomendado** | El de todos los días, y el default al abrir. Calidad completa con las mejoras medidas de este fork. |
| **Publicar** | Igual que Recomendado pero con `-skylevel 7`. Cuesta ~1.65× más en RAD por una diferencia invisible; solo tiene sentido si necesitas reproducir exactamente la salida del SDHLT original. |

## Qué trae de este fork

La pestaña **"Guía"** resume las reglas con los números medidos
detrás. Lo esencial:

- **Hilos en automático.** Este fork arregló dos bugs por los que las tools
  corrían monohilo (en Linux siempre; en Windows con CPUs de más de 32 hilos
  lógicos). Medido: 1.68× en una máquina de 2 núcleos.
- **`-skylevel 6` por defecto.** El loop de luz de cielo es el 96% de todos los
  rayos que lanza RAD. El nivel 6 usa 4× menos rayos que el 7 con una diferencia
  máxima de 1/255 por luxel. **RAD ~1.65× más rápido.**
- **VIS en `full` para publicar.** Es lo único que decide cuánto le pide el mapa
  al motor por frame.
- **`-subdivide` en 240.** No es conservador: es el techo de
  `MAX_SURFACE_EXTENT`. Subirlo hace que el mapa no cargue en software renderer
  ni en HLDS.
- **`-pre25` activado por defecto.** Baja el umbral de recorte de luz de 255 a
  188. Casi nadie usa el cliente del 25 aniversario, y el error es asimétrico:
  sin `-pre25` las zonas brillantes se rompen en clientes antiguos, mientras que
  al revés solo se ven un poco menos brillantes.
- **Texturas embebidas por defecto.** El jugador no necesita el WAD; a cambio el
  `.bsp` crece.

Los números están en `docs/BENCHMARKS.md`. Y lo que de verdad baja `wpoly` está en
`docs/FPS_Y_TOOL_TEXTURES.md`: el compilador ya hace casi todo lo que puede.

## Alcance

Hace tres cosas: configurar, compilar, y mostrar el log en vivo con errores y
warnings resaltados. **No** copia el mapa al juego, no lo lanza, y no genera
`.res`. Fue una decisión deliberada para reducir la superficie de bugs en algo
que no puedo probar.

## Estructura

```
gui/
├── Cargo.toml
├── build.rs        compila assets/icon.rc con rc.exe del SDK y lo linkea
├── assets/
│   ├── make_icon.ps1   genera el .ico y el RGBA crudo (editá esto, no el .ico)
│   ├── resdhlt.ico     icono del .exe, 16 a 256 px
│   ├── icon_64.rgba    mismo icono ya decodificado, para la ventana
│   (el .rc con el icono y la versión lo genera build.rs)
└── src/
    ├── main.rs      pestañas, layout y estado de la app
    ├── theme.rs     paleta y estilo de widgets (todo el color vive acá)
    ├── widgets.rs   fila de opción, tarjeta, switch, chip, tab strip
    ├── projects.rs  proyectos guardados y vista de la carpeta
    ├── update.rs    releases de GitHub: comprobación e instalación
    ├── options.rs   opciones, descripciones, presets, línea de comandos
    └── runner.rs    ejecuta las 4 etapas y streamea la salida
```

Si quieres agregar un flag: va en `options.rs` (campo + a `*_args()`) y una fila
en la pestaña que corresponda en `main.rs`, usando `row` o `toggle_row`.

### Detalles que quizás no se ven

- Las etapas se ejecutan en un hilo aparte; la UI nunca se congela.
- El log se lee cortando en `\n` **y** en `\r`, porque las herramientas dibujan
  el progreso reescribiendo una línea con retornos de carro. Sin eso el log
  quedaría vacío hasta el final de cada etapa y después vomitaría todo junto.
- `stdout` y `stderr` se drenan en hilos separados: si solo leyeras uno, el otro
  puede llenar su buffer y bloquear al proceso hijo.
- En Windows se usa `CREATE_NO_WINDOW`, para que no aparezca una consola negra
  por cada etapa.
- Todas las filas de opción pasan por `widgets::row`, que reserva las mismas
  tres columnas (etiqueta, control, recomendación) con anchos calculados una vez
  por frame. Por eso los sliders y switches de las cinco pestañas caen sobre las
  mismas líneas verticales a cualquier tamaño de ventana.
- Los paneles de log lateral e inferior usan ids distintos: egui recuerda el
  tamaño de un panel por id, y compartirlo hacía que el log de abajo heredara el
  ancho del de la derecha como altura.
