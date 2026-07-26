# ReSDHLT — Análisis técnico y estado de las mejoras

Fork de [seedee/SDHLT](https://github.com/seedee/SDHLT) (linaje Vluzacn ZHLT v34).
Herramientas de compilación de mapas GoldSrc (**CSG, BSP, VIS, RAD** + RIPENT) para **CS 1.6**.

Documentos relacionados:

- **`docs/FPS_Y_TOOL_TEXTURES.md`** — cómo bajar `wpoly` de verdad. Si hacés mapas, ese es el que importa.
- **`docs/BENCHMARKS.md`** — todas las mediciones, incluidos los resultados negativos.

---

## 0. Marco honesto: qué determina los FPS en CS 1.6

| Factor de FPS | Lo fija principalmente | ¿Lo puede mejorar el compilador? |
|---|---|---|
| `wpoly` (polígonos de mundo) | VIS (PVS) + conteo de caras del BSP | Muy poco: ver §3 |
| `epoly` (entidades/modelos) | El mapa y los modelos | No |
| Overdraw en zonas abiertas | Diseño del mapa + hint brushes | No |
| Carga de lightmaps | RAD (`chop`, `texchop`) | Indirecto, y no afecta FPS |

La conclusión, ahora respaldada con mediciones: el compilador ya hace casi todo lo que puede.
La fusión de caras del BSP **ya elimina entre el 19% y el 46% de las caras** (§3.2), VIS ya calcula un
PVS prácticamente exacto, y hay un **techo duro de 240 unidades** en `-subdivide` que no se puede
cruzar sin romper el software renderer y el HLDS.

**El retorno real de este fork está, por orden: (1) velocidad de compilación, (2) robustez,
(3) documentación para el mapper.** Los FPS los baja el mapper, no el compilador.

---

## 1. Implementado y verificado ✅

### Threading y correctitud

**1.1 Linux compilaba siempre con 1 hilo.** En POSIX `DEFAULT_NUMTHREADS` era `1`, no `-1`, así que la
rama de autodetección de `ThreadSetDefault()` era **código muerto**: ninguna build de Linux autodetectó
núcleos jamás.
*Medido:* `koth_sandy` pasó de **2.88s a 1.71s** (1.68×) al usar los 2 núcleos del entorno. En una CPU
de 8 núcleos la diferencia es mucho mayor.

**1.2 Windows con >32 procesadores lógicos caía a 1 hilo.** `if (n < 1 || n > 32) n = 1;`. Cualquier
Ryzen 9 / Threadripper compilaba monohilo. Verificado con stubs de la API Win32: 48 CPUs → 48 hilos;
512 → clamp a 256.

**1.3 Stack buffer overflow con `-threads` fuera de rango.** Ninguna de las 5 tools valida el límite
superior, y los handles vivían en arrays fijos en stack. **Reproducido: SIGSEGV con core dump** con
`-threads 5000`. Arreglado con `ClampNumThreads()` central.

**1.4 Dispatch con atomic en vez del lock global.** `GetThreadWork()` tomaba el lock global en cada
unidad de trabajo. Ganancia modesta acá (10–22% del dispatcher, con 2 núcleos no hay mucha contención);
lo importante fue verificar que **200.000 unidades se despachan exactamente una vez** con 1/2/4/16/64
hilos.

**1.5 Unificación en `std::thread`.** Había dos implementaciones casi idénticas tras `#ifdef` (Win32 y
pthread) más una tercera single-threaded — motivo por el cual las ramas divergieron y aparecieron 1.1 y
1.2. **723 → 448 líneas.** Los handles pasaron a `std::vector` dimensionado por el conteo real, lo que
elimina el riesgo de desborde *estructuralmente*, no por clamp.

### Build

**1.6 Release se degradaba de `-O3` a `-O2`.** El `-O2` hardcodeado en `add_compile_options()` iba
después del `-O3` de `CMAKE_CXX_FLAGS_RELEASE`, y GCC respeta el último `-O`. Verificado con
`g++ -Q --help=optimizers`: `-O3` → 157 pases, `-O2` → 142, `-O3 -O2` → **142**.

**1.7 `CMAKE_BUILD_TYPE` sin default.** Con generadores mono-config, un build type vacío significa cero
optimización. Ahora default a `Release`.

**1.8 `install` y CPack.** No había forma de obtener una carpeta usable: los binarios quedaban mezclados
con los archivos fuente en `tools/`. Ahora `cmake --install` deja los 5 binarios más
`sdhlt.wad`/`fgd`/`lights.rad` y CPack genera ZIP/TGZ.

**1.9 CI corría `ctest` sin tests** y fallaba siempre. Reemplazado por un smoke test de las 5 tools.

### Infraestructura de medición

**1.11 Builds reproducibles.** Dos compilaciones del mismo mapa daban BSPs distintos. Aislado a CSG:
`FindIntPlane()` numera planos según qué hilo gana el lock, y `WriteFace()` escribe las caras en orden
de finalización. VIS y RAD ya eran independientes del orden. Las tres fases paralelas de CSG ahora
corren en un hilo por defecto; cuesta **0.1%** del tiempo total y el output resultante es byte-idéntico
al baseline monohilo. `-nodeterministic` restaura lo anterior.

**1.12 `scripts/bspcheck.py`.** Validador geométrico del BSP: planaridad, convexidad, caras
degeneradas, área total (que la fusión debe preservar exactamente) y fusiones residuales. Probé que
tiene sensibilidad real: al mover un vértice compartido reporta las 3 caras afectadas.

**1.10 `scripts/compilebench.py`.** Cronometra las 4 etapas sobre un `.map` real y hace un fingerprint
de cada lump del BSP (conteos + hash), para poder demostrar que un cambio no alteró la salida. Es lo que
permitió verificar todo lo de arriba contra mapas reales de CS.

---

## 2. La verificación más fuerte

**Upstream original (commit de import) vs ReSDHLT final, `-threads 1`, lump por lump:**

| Mapa | Resultado |
|---|---|
| koth_sandy | **BSP byte-idéntico** (15 lumps) |
| ba_coliseum | **BSP byte-idéntico** (15 lumps) |

O sea: todos los cambios son **demostrablemente preservadores de salida**. Las tools son más rápidas,
más robustas y no crashean, y el `.bsp` que producen es exactamente el mismo. Eso también significa,
para ser claro: **los FPS en juego no cambian por este fork**. Cambia el tiempo que esperás compilando.

---

## 3. Lo que decidí NO hacer, y por qué

Esta sección importa tanto como la anterior. Tres ítems del roadmap original se investigaron y se
descartaron **con evidencia**, en lugar de implementarse a ciegas.

### 3.1 Optimizar RAD ⚠️ — perfilado sí, optimizado no
RAD es >95% del tiempo. `perf` está bloqueado acá y gprof dio datos falsos (reportó `MakeTnode()` con
93.301.204 llamadas; un contador mostró **78**). Así que el profiler ahora está **dentro de RAD**
(`-profile`), funciona en Windows sin instalar nada, y sus conteos coinciden con las entradas creíbles de
gprof.

Hot spot real: **`TestLine_r`, 1.849.616.672 entradas** por mapa. `GatherSampleLight` es ~94% del tiempo
de `BuildFacelights`.

**Intenté la optimización y falló.** Tres de los cuatro caminos recursivos son tail calls, así que
reescribí la función como loop. La iluminación salió **byte-idéntica** (correcta), pero más lenta: 2.69s
copiando endpoints y 2.76s con punteros, contra **2.59s** de la recursiva original. GCC ya emite menos
llamadas recursivas de lo que sugiere el código y las llamadas son baratas por el return stack buffer.
**Revertido**; quedó solo la instrumentación.

Lo que el perfil sí indica: **426 rayos `TestLine` por `GatherSampleLight`** y ~30 descensos de árbol por
rayo. El costo es *cuántos* rayos se lanzan, no cuánto cuesta cada uno, así que el próximo paso es
algorítmico. Guía completa en `docs/PERFILAR_RAD.md`.

### 3.2 Mejorar `TryMerge` ❌ — medido: 0.7% de margen
Investigado a fondo esta vez, no descartado por precaución. Cuatro líneas de evidencia:

1. **`maxedges = 0` rechazos** en los tres mapas → subir `MAXEDGES` no ganaría nada.
2. **0 texinfo duplicados** (34 entradas, 34 únicas) → no se pierden fusiones por texinfo redundante.
3. **`MergeFaceToList()` ya recursa** tras cada fusión, así que la lista final por plano es *pairwise*
   no-fusionable: un punto fijo real.
4. **Fusiones residuales en el BSP final: 32** en ba_coliseum. De esas, **26 están en nodos distintos**
   del BSP, donde fusionar es estructuralmente imposible porque la cara pertenece a su nodo. Quedan
   **6 sobre 815 caras = 0.7%**, y probablemente bloqueadas por `contents`/`detaillevel`/`facestyle`.

Clave para entender por qué parece haber más margen del que hay: `sdHLBSP` hace `MergePlaneFaces()` y
**después** `SubdivideFace()`. Las caras coplanares adyacentes del BSP final son subdivisiones
*deliberadas* para respetar `MAX_SURFACE_EXTENT`. Un conteo naive da 697 "fusiones perdidas"; casi todas
romperían el software renderer si se fusionaran.

### 3.3 `-O3` / LTO por defecto ❌ — sin mejora medible
La varianza *dentro* de cada variante (hasta 0.87s) supera la diferencia *entre* variantes (0.10s).
En una primera corrida LTO parecía ganar 12%; al repetir, el orden se dio vuelta. LTO queda **opt-in**.
Detalle completo en `docs/BENCHMARKS.md` §3.

---

## 4. Hallazgo lateral, ya arreglado: no-determinismo

Descubierto al armar el baseline de regresión: dos corridas del mismo binario sin modificar daban
clipnodes +2, marksurfaces +1, visdata −4 bytes.

Aislado a **CSG y solo CSG** (fijando CSG en 1 hilo con VIS multihilo el output es byte-idéntico):
`FindIntPlane()` asigna índices de plano según qué hilo gana el lock — el código original trae un
comentario *"BUG: there might be some multithread issue --vluzacn"* justo ahí — y `WriteFace()` escribe
las caras a `.p0`–`.p3` en orden de finalización. Ambos índices llegan a BSP, que es monohilo.

**Arreglado:** las fases paralelas de CSG corren en un hilo por defecto. Costo 0.1% del total. Los tres
mapas ahora compilan idénticos dos veces seguidas, y el resultado coincide byte a byte con el baseline
monohilo previo.

## 5. Qué queda por hacer

| # | Mejora | Área | Por qué no se hizo |
|---|--------|------|--------------------|
| 1 | Optimizar el ray casting de RAD | Compilación | Necesita `perf` en hardware real (§3.1). Es el único lugar con tiempo real que ganar |
| 2 | Rediseñar `TryMerge` | FPS | Medido: 0.7% de margen (§3.2). No vale el riesgo |
| 3 | Medir `-O3`/LTO en hardware real | Build | El entorno tiene ±8% de ruido (§3.3) |
| 4 | Probar el branch de Windows con MSVC | Portabilidad | Lo cubre el CI de GitHub Actions |
| 5 | Verificación visual en CS 1.6 | FPS/calidad | No puedo correr el juego. Es lo único que valida costuras de luz y grietas |

---

## 6. Limitaciones de la verificación

Para ser transparente:

- Todo se compiló y midió en **Linux/GCC 11 con 2 núcleos**. Las ganancias de threading están
  **subestimadas**; los resultados de `-O3`/LTO son inconclusos por ruido.
- El branch de Windows se verificó por **lógica con stubs**, no compilando con MSVC.
- Se compilaron **3 mapas reales** (`ba_dust_island`, `ba_coliseum`, `koth_sandy`). Otros dos
  (`db_snow`, `ar_azteca`) no compilan acá porque les faltan WADs externos.
- **Nunca se abrió un mapa en CS 1.6.** No hay verificación visual ni medición de `wpoly` en juego. Eso
  es precisamente por lo que no toqué BSP ni RAD.

*Código citado verificado contra el árbol del fork.*
