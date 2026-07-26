# Mediciones

Todo lo de acá se midió con `scripts/compilebench.py` sobre mapas reales de CS 1.6.
Registro también los **resultados negativos**, porque saber que algo *no* sirve evita perder tiempo
después.

## Entorno

- Linux, GCC 11.4, **2 núcleos** (sandbox)
- Mapas: `ba_dust_island`, `ba_coliseum`, `koth_sandy`
- El ruido de medición en este entorno es de **±8%**, lo cual condiciona qué conclusiones se pueden
  sacar (ver §3)

> Con 2 núcleos, todo lo que sea escalabilidad de hilos está **subestimado**. En una CPU de 8 o 16
> núcleos las ganancias de threading son bastante mayores.

## 1. Dónde se va el tiempo de compilación

| Mapa | CSG | BSP | VIS | RAD | Total |
|---|---|---|---|---|---|
| ba_dust_island | 0.03s | 0.04s | 0.01s | **8.57s** | 8.64s |
| ba_coliseum | 0.05s | 0.13s | 0.00s | **8.56s** | 8.75s |
| koth_sandy | 0.03s | 0.09s | 0.06s | **1.44s** | 1.62s |

**RAD es más del 95% del tiempo.** Optimizar CSG, BSP o VIS es irrelevante en la práctica;
cualquier trabajo de rendimiento tiene que ir a RAD.

## 2. Impacto del fix de threading (el resultado más importante)

`koth_sandy`, best of 3:

| | CSG | BSP | VIS | RAD | Total |
|---|---|---|---|---|---|
| `-threads 1` | 0.03s | 0.06s | 0.10s | 2.69s | **2.88s** |
| `-threads 2` | 0.04s | 0.07s | 0.06s | 1.53s | **1.71s** |

**1.68× más rápido** por usar los dos núcleos. Esto es exactamente lo que perdía toda build de Linux,
porque `DEFAULT_NUMTHREADS` era `1` en POSIX y la rama de autodetección de `ThreadSetDefault()` era
código muerto. En una máquina de 8 núcleos la diferencia es mucho mayor.

## 3. `-O2` vs `-O3` vs LTO: sin diferencia medible ❌

Primera corrida (una sola pasada, `ba_dust_island`) sugería una mejora clara de LTO:

| | RAD |
|---|---|
| `-O2` | 9.21s |
| `-O3` | 8.75s |
| LTO | 8.10s |

Pero al repetir, el orden **se dio vuelta**:

| | RAD (best of 2) |
|---|---|
| `-O2` | 8.51s |
| `-O3` | 8.26s |
| LTO | 8.68s |

Medición más limpia (`-threads 1`, 4 corridas, `koth_sandy`), mostrando todas las corridas:

| | totals | min |
|---|---|---|
| `-O2` | 2.75 / 3.22 / 2.81 / 2.81 | 2.75s |
| `-O3` | 2.74 / 2.76 / 2.82 / 2.89 | 2.74s |
| LTO | 2.73 / 3.52 / 3.10 / 2.65 | 2.65s |

**La varianza dentro de cada variante (hasta 0.87s) es mayor que la diferencia entre variantes
(0.10s).** No hay evidencia de que `-O3` o LTO mejoren nada acá.

**Decisión:** LTO queda **opt-in** (`-DSDHLT_LTO=ON`), no por defecto. Encender LTO por defecto
alargaría los builds sin beneficio demostrado. `-O3` queda simplemente porque es el default de
`Release` de CMake.

Tamaño del binario `sdHLRAD`: `-O2` 442KB, `-O3` 482KB, LTO 438KB.

> Si querés decidir para *tu* máquina, corré esto y mirá si la diferencia supera tu propia varianza:
> ```
> python3 scripts/compilebench.py --tools <dir> --map <mapa> --threads 1 --runs 5
> ```

## 4. Profiling de RAD: resuelto, con un intento fallido de optimización

RAD es >95% del tiempo, así que era el objetivo obvio. El primer intento con herramientas externas
falló y el segundo, instrumentando las tools, funcionó.

### 4.1 Por qué gprof no servía

- **`perf` no está disponible** en el entorno (`perf_event_paranoid` lo bloquea), y además es solo Linux.
- **`gprof` dio datos que no resisten verificación:** reportó `MakeTnode()` como el 49.83% del tiempo con
  **93.301.204 llamadas**. Le puse un contador: se llama **78 veces**. Con símbolos de otro build, llegó
  a atribuir 85 millones de llamadas a `_fini`.

`MakeTnode` es la única función `static` de `trace.cpp`, así que estaba absorbiendo las muestras del
código vecino.

### 4.2 El profiler propio (`-profile`)

Está dentro de las tools, funciona en Windows sin instalar nada. Dos niveles, porque el código caliente
tiene frecuencias muy distintas:

- `PROF_SCOPE` **cronometra** las funciones de fase; se activa en runtime con `-profile`.
- `PROF_CALL` **cuenta** las funciones internas del ray casting; se activa **en compilación** con
  `-DSDHLT_PROFILE=ON`. `TestLine_r` se entra ~1.800 millones de veces por mapa, así que incluso un
  chequeo apagado costaría casi un segundo por compilación a quien nunca perfila.

**Perfil de `ba_dust_island`, 1 hilo:**

| sitio | llamadas | share |
|---|---|---|
| `BuildFacelights` | 653 | 51.6% |
| `GatherSampleLight` | 146.152 | 47.9% (anidada en la anterior, ~94% de su tiempo) |
| `TestLine` | 62.245.301 | (solo conteo) |
| **`TestLine_r`** | **1.849.616.672** | (solo conteo) |
| `TestSegmentAgainstOpaqueList` | 38.965.499 | (solo conteo) |

Esto confirma la sospecha sobre gprof: el hot spot real es **`TestLine_r`**. Y los conteos coinciden con
las entradas creíbles de gprof (`TestSegmentAgainstOpaqueList` = 38.965.499 en ambos), lo que valida las
dos mediciones entre sí.

Verificado que la instrumentación apagada es gratis, midiendo A/B **intercalado** para cancelar la carga
de la máquina: **−1.1%**, o sea ruido. Una medición previa sin intercalar sugería 20% de regresión, que
era deriva por otros builds corriendo en la misma máquina — vale como advertencia sobre medir en un
entorno compartido.

### 4.3 El intento de optimización, y por qué se revirtió ❌

Tres de los cuatro caminos recursivos de `TestLine_r` son **tail calls**, así que lo reescribí como loop.
La iluminación salió **byte-idéntica** en los tres mapas, o sea la transformación era correcta. Pero fue
más lenta, dos veces:

| versión | RAD (min de 4, `koth_sandy`, 1 hilo) |
|---|---|
| original recursiva | **2.59s** |
| iterativa, copiando endpoints | 2.69s |
| iterativa, punteros a endpoints | 2.76s |

Razones: GCC ya emite menos sitios de llamada recursiva de los que sugiere el código (10 en el cuerpo),
las llamadas son baratas en CPUs modernas por el *return stack buffer*, y mis dos versiones gastaban más
en copias o indirección de punteros de lo que ahorraban.

**Revertido.** Se quedó solo la instrumentación.

### 4.4 Dónde estaría la ganancia real

El perfil apunta a otra cosa:

| | ba_dust_island | koth_sandy |
|---|---|---|
| rayos `TestLine` por `GatherSampleLight` | **426** | 374 |
| descensos de árbol por rayo | 30 | 20 |

El costo está en **cuántos rayos se lanzan**, no en el precio de cada uno. O sea el próximo paso es
**algorítmico** (lanzar menos rayos: culling, early-out, caching de visibilidad), no micro-optimización.
Y eso hay que hacerlo con menos ruido de medición del que tengo acá.

## 5. Fusión de caras: sin margen real (investigado a fondo)

Esta era la vía candidata a bajar `wpoly`. La respuesta, con evidencia, es que no hay margen.

**Lo que la fusión ya logra** (`MergeAll` / `TryMerge`, medido con `-verbose`):

| Mapa | Caras finales | Liberadas por fusión | Reducción |
|---|---|---|---|
| ba_coliseum | 1144 | 975 | **46%** |
| koth_sandy | 1259 | 303 | **19%** |
| ba_dust_island | 583 | 149 | **20%** |

**Rechazos de `TryMerge` instrumentados** (contador por cada uno de los 9 motivos):

| Mapa | fusiones | maxedges | tex | convex | plane |
|---|---|---|---|---|---|
| ba_coliseum | 2131 | **0** | 7522 | 934+934 | 96 |
| koth_sandy | 474 | **0** | 1884 | 10+10 | 177 |
| ba_dust_island | 1040 | **0** | 286 | 47+47 | 0 |

`maxedges = 0` en los tres mapas: **subir `MAXEDGES` no ganaría nada**. Los conteos de `tex` y
`contents` son altos pero engañosos, porque se chequean *antes* del test de arista compartida — la
mayoría de esos pares no comparte arista.

**Texinfo duplicados: 0.** Si CSG emitiera texinfos redundantes, caras visualmente idénticas fallarían
el chequeo `texturenum` y se perderían fusiones. Parseé el lump: 34 entradas, 34 únicas, 0 caras usando
un texinfo duplicado. Hipótesis descartada.

**Fusiones residuales en el BSP final** (`scripts/bspcheck.py`). Acá estuvo el hallazgo importante.
Un primer conteo naive dio **697** pares "fusionables" en ba_coliseum, lo cual habría sido enorme.
Pero `sdHLBSP` hace `MergePlaneFaces()` y **después** `SubdivideFace()`: las caras coplanares adyacentes
del BSP final son, en su mayoría, subdivisiones **deliberadas** para mantener el extent de lightmap
dentro de `MAX_SURFACE_EXTENT`. Fusionarlas rompería el software renderer y el HLDS.

Excluyendo los pares cuya unión excedería el límite de 240 unidades: **32**.
Y mapeando cada cara a su nodo del BSP:

| | ba_coliseum |
|---|---|
| pares residuales | 32 |
| en **nodos distintos** (imposible: la cara pertenece a su nodo) | 26 |
| en el **mismo nodo** (únicos candidatos reales) | 6 |

**6 de 815 caras = 0.7%.** Y esos 6 están probablemente bloqueados por `contents` / `detaillevel` /
`facestyle`, que son atributos de compilación que no quedan registrados en el BSP.

**Decisión: no tocar `TryMerge`.** No por precaución, sino porque está medido que no hay nada que ganar.
Sumado a que `MergeFaceToList()` ya recursa tras cada fusión exitosa, la lista final por plano es
*pairwise* no-fusionable: un punto fijo real.

> Nota metodológica: mi primer chequeo de convexidad reportó 84 caras no-convexas en ba_coliseum.
> Era un bug **mío**: comparaba un producto cruz crudo contra un epsilon fijo, y esa magnitud escala
> con el largo de las aristas, así que aristas largas con ángulos despreciables daban falsos positivos.
> Normalizando por los largos queda un test angular independiente de escala, y los tres mapas dan
> geometría limpia. Vale como recordatorio de sospechar de la herramienta antes que del compilador.

## 6. No-determinismo: encontrado y arreglado ✅

Descubierto al armar el baseline de regresión. Dos corridas del **mismo binario sin modificar** sobre
`koth_sandy`, con hilos autodetectados:

| lump | corrida 1 | corrida 2 | delta |
|---|---|---|---|
| clipnodes | 596 | 598 | +2 |
| marksurfaces | 613 | 614 | +1 |
| visibility | 1509 bytes | 1505 bytes | −4 |

**Aislamiento por etapa.** Fijando CSG en 1 hilo pero dejando VIS multihilo, el output es
byte-idéntico. O sea **VIS y RAD ya son independientes del orden**; toda la variación venía de CSG:

- `FindIntPlane()` agrega al arreglo global de planos bajo lock: el hilo que gana define el índice del
  plano. El código original incluso trae el comentario *"BUG: there might be some multithread issue
  --vluzacn"* en ese lugar.
- `WriteFace()` hace `fprintf` directo a los archivos `.p0`–`.p3` bajo lock: el orden de finalización
  define el orden de las caras.

Ambos índices llegan a BSP (que es monohilo), y de ahí salían los conteos distintos de clipnodes.

**Arreglo:** las tres fases paralelas de CSG (`CreateBrush`, `CSGBrush`, `CalculateBrushUnions`) corren
en un hilo cuando `g_deterministic` está activo, que ahora es **el default**.

**Costo, medido:** CSG solo 0.046s → 0.059s. CSG+BSP+VIS end-to-end 0.170s → 0.180s. Contra ~8.5s de
RAD en el mismo mapa, es **0.1%**. `-nodeterministic` restaura el comportamiento viejo.

**Verificado:**

| Mapa | dos compilaciones multihilo |
|---|---|
| ba_coliseum | idénticas |
| koth_sandy | idénticas |
| ba_dust_island | idénticas |

Y el output determinista es **byte-idéntico al baseline monohilo previo** — o sea se arregló el orden,
no se adoptó un resultado nuevo. Con `-nodeterministic` sigue variando, lo que confirma la atribución.

Consecuencia práctica: **ya no hace falta `-threads 1` para comparar regresiones.** Se mantiene la
recomendación igual, porque es la garantía más fuerte y no cuesta nada en mapas chicos.

## 7. Regresión del refactor de threading

Reemplazar los backends Win32/pthread por `std::thread` (723 → 448 líneas), verificado con
`--threads 1`:

| Mapa | Resultado |
|---|---|
| koth_sandy | BSP byte-idéntico, lump por lump |
| ba_coliseum | BSP byte-idéntico, lump por lump |

Más los tests unitarios: autodetección = `nproc`, `-threads 5000` clampeado sin crash, y 200.000
unidades de trabajo despachadas exactamente una vez con 1/2/4/16/64 hilos.

## Cómo reproducir

```sh
cmake -B build -S . && cmake --build build -j

# rendimiento (multihilo)
python3 scripts/compilebench.py --tools tools --map mapa.map --runs 3

# regresión (SIEMPRE con 1 hilo)
python3 scripts/compilebench.py --tools tools_antes --map mapa.map --threads 1 --json antes.json
python3 scripts/compilebench.py --tools tools_despues --map mapa.map --threads 1 --json despues.json
python3 scripts/compilebench.py --compare antes.json despues.json
```

Nota: los `.map` traen rutas absolutas de WAD de la máquina donde se hicieron. Hay que reescribir la
clave `"wad"` del worldspawn a rutas locales antes de compilar.
