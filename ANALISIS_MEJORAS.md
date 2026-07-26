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

### 3.1 Optimizar RAD ❌ — datos no confiables
RAD es **>95% del tiempo de compilación**, así que era el objetivo obvio. Pero:

- `perf` está bloqueado en el entorno (`perf_event_paranoid`).
- `gprof` dio resultados contradictorios: reportó `MakeTnode()` como **49.83% del tiempo con 93.301.204
  llamadas**. Instrumenté la función con un contador: se llama **78 veces**. Con símbolos de otro build,
  llegó a atribuir 85 millones de llamadas a `_fini`.

Optimizar un ray tracer con un profile que no resiste una verificación básica es la forma más directa de
romper la iluminación. El profile *sí* sugiere que el tiempo está en ray casting (`TestLine` 62M
llamadas, `CheckVisBitSparse` 43M). En `docs/BENCHMARKS.md` está el procedimiento para hacerlo bien con
`perf` en hardware real.

### 3.2 Mejorar la fusión de caras ❌ — ya es efectiva
| Mapa | Caras finales | Liberadas por fusión | Reducción |
|---|---|---|---|
| ba_coliseum | 1144 | 975 | **46%** |
| koth_sandy | 1259 | 303 | **19%** |
| ba_dust_island | 583 | 149 | **20%** |

Y `MergeFaceToList()` **ya llega a un punto fijo**: recursa tras cada fusión y reintenta contra toda la
lista. El margen restante exige rediseñar `TryMerge` (que exige arista compartida exacta para mantener
convexidad) y validarlo **requiere ver el mapa en el juego** — grietas y corrupción visual no se
detectan con un diff del BSP, y no tengo forma de correr CS acá.

### 3.3 `-O3` / LTO por defecto ❌ — sin mejora medible
La varianza *dentro* de cada variante (hasta 0.87s) supera la diferencia *entre* variantes (0.10s).
En una primera corrida LTO parecía ganar 12%; al repetir, el orden se dio vuelta. LTO queda **opt-in**.
Detalle completo en `docs/BENCHMARKS.md` §3.

---

## 4. Hallazgo lateral: las tools no son deterministas

Descubierto al armar el baseline de regresión. Dos corridas del **mismo binario sin modificar** sobre
`koth_sandy` con hilos autodetectados dieron: clipnodes +2, marksurfaces +1, visdata −4 bytes.
Con `-threads 1` el output es byte-idéntico.

Consecuencias: toda comparación de regresión debe ser con `-threads 1` (el script avisa si no), y los
builds de mapas no son reproducibles bit a bit. No es un bug de corrección — los BSPs son válidos —
pero es una limitación que conviene conocer.

---

## 5. Qué queda por hacer

| # | Mejora | Área | Por qué no se hizo |
|---|--------|------|--------------------|
| 1 | Optimizar el ray casting de RAD | Compilación | Necesita `perf` en hardware real (§3.1) |
| 2 | Rediseñar `TryMerge` | FPS | Necesita validación visual en el juego (§3.2) |
| 3 | Builds de mapas reproducibles | Calidad | Requiere ordenar la escritura de salida por índice (§4) |
| 4 | Medir `-O3`/LTO en hardware real | Build | El entorno tiene ±8% de ruido (§3.3) |
| 5 | Probar el branch de Windows con MSVC | Portabilidad | Lo cubre el CI de GitHub Actions al subir |

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
