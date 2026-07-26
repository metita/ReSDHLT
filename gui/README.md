# ReSDHLT GUI

Compilador de mapas para CS 1.6 con interfaz gráfica, tema oscuro, y una
explicación de qué hace cada opción al pasar el mouse por encima.

Hecho en Rust con [egui](https://github.com/emilk/egui). Un solo ejecutable, sin
instalador ni runtime.

---

## ⚠️ Leé esto primero

**Este código no está compilado ni probado.** Todo lo demás en este repositorio se
verificó compilando y midiendo; esto no. El entorno donde lo escribí no tiene
toolchain de Rust y tiene `crates.io` bloqueado, así que no pude ni siquiera
correr `cargo check`.

Está escrito con cuidado, con dependencias mínimas y versiones fijas, usando
únicamente API de egui que lleva años estable. Pero es muy probable que el primer
`cargo build` tire uno o dos errores.

**Si falla, pasame el error completo y lo arreglo.** Los errores de compilación de
Rust son precisos: casi siempre es un tipo o un nombre de método.

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

1. **Mapa**: el `.map` que exporta tu editor. Los `.bsp`, `.prt` y logs salen al
   lado de ese archivo.
2. **Herramientas**: la carpeta con `sdHLCSG`, `sdHLBSP`, `sdHLVIS`, `sdHLRAD`.
   Si compilaste este repo, es `tools/`.
3. Elegí un preset y dale a **Compilar**.

"Guardar preferencias" deja un `resdhlt-gui.json` al lado del ejecutable.

## Los presets

| Preset | Para qué |
|---|---|
| **Borrador** | Ver si el mapa carga y no tiene leaks. VIS rápido, RAD rápido, 1 bounce. La luz y los FPS no son representativos. |
| **Recomendado** | El de todos los días, y el default al abrir. Calidad completa con las mejoras medidas de este fork. |
| **Publicar** | Igual que Recomendado pero con `-skylevel 7`. Cuesta ~1.65× más en RAD por una diferencia invisible; solo tiene sentido si necesitás reproducir exactamente la salida del SDHLT original. |

## Qué trae de este fork

La pestaña **"Qué usar siempre"** resume las reglas con los números medidos
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
└── src/
    ├── main.rs      interfaz, tema, pestañas
    ├── options.rs   opciones, descripciones, presets, línea de comandos
    └── runner.rs    ejecuta las 4 etapas y streamea la salida
```

Si querés agregar un flag: va en `options.rs` (campo + a `*_args()`) y una fila
en la pestaña que corresponda en `main.rs`.

### Detalles que quizás no se ven

- Las etapas se ejecutan en un hilo aparte; la UI nunca se congela.
- El log se lee cortando en `\n` **y** en `\r`, porque las herramientas dibujan
  el progreso reescribiendo una línea con retornos de carro. Sin eso el log
  quedaría vacío hasta el final de cada etapa y después vomitaría todo junto.
- `stdout` y `stderr` se drenan en hilos separados: si solo leyeras uno, el otro
  puede llenar su buffer y bloquear al proceso hijo.
- En Windows se usa `CREATE_NO_WINDOW`, para que no aparezca una consola negra
  por cada etapa.
