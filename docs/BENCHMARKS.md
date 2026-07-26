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

## 4. Profiling de RAD: no concluyente ❌

RAD es el 95% del tiempo, así que era el objetivo obvio. No pude obtener un profile confiable:

- **`perf` no está disponible** en el sandbox (`perf_event_paranoid` lo bloquea).
- **`gprof` da resultados contradictorios.** Reportó `MakeTnode()` como el 49.83% del tiempo con
  **93.301.204 llamadas**. Instrumenté la función con un contador real: se llama **78 veces**
  (el mapa tiene 128 nodos, y `MakeTnode` recorre el árbol una única vez desde `MakeTnodes`).
  Resolviendo símbolos contra otro build, gprof llegó a atribuir 85 millones de llamadas a `_fini`.

`MakeTnode` es la única función `static` de `trace.cpp`, así que probablemente recibe las muestras del
código vecino inlineado. Lo que el profile *sí* sugiere, por las entradas con conteos de llamadas
coherentes, es que el tiempo está en **ray casting**: `TestLine` (62M llamadas),
`CheckVisBitSparse` (43M), `TestSegmentAgainstOpaqueList` (39M), `GatherSampleLight`.

**Decisión:** no optimizar RAD con estos datos. Optimizar un ray tracer a ciegas es la forma más
directa de romper la iluminación. Cómo hacerlo bien:

```sh
# en hardware real, con perf disponible:
cmake -B build -S . -DCMAKE_BUILD_TYPE=RelWithDebInfo
perf record -g --call-graph=dwarf ./tools/sdHLRAD -threads 1 <mapa>
perf report --no-children
```

Y validar cada cambio con `--threads 1` comparando el BSP lump por lump.

## 5. Fusión de caras: ya es efectiva

`MergeAll` / `TryMerge` en el BSP, medido con `-verbose`:

| Mapa | Caras finales | Liberadas por fusión | Reducción |
|---|---|---|---|
| ba_coliseum | 1144 | 975 | **46%** |
| koth_sandy | 1259 | 303 | **19%** |
| ba_dust_island | 583 | 149 | **20%** |

Además el algoritmo **ya llega a un punto fijo**: `MergeFaceToList()` recursa después de cada fusión
exitosa y reintenta contra toda la lista, así que no quedan fusiones "obvias" sin hacer.

**Decisión:** no tocarlo. El margen que queda exige un rediseño de `TryMerge` (que exige arista
compartida exacta para mantener convexidad), y validarlo requiere ver el mapa en el juego —
corrupción visual y grietas no se detectan con un diff del BSP. El camino con retorno real está en
`docs/FPS_Y_TOOL_TEXTURES.md`: que el mapper no genere esas caras.

## 6. Las tools no son deterministas con múltiples hilos ⚠️

Descubierto al armar el baseline de regresión. Dos corridas del **mismo binario sin modificar** sobre
`koth_sandy`, con hilos autodetectados:

| lump | corrida 1 | corrida 2 | delta |
|---|---|---|---|
| clipnodes | 596 | 598 | +2 |
| marksurfaces | 613 | 614 | +1 |
| visibility | 1509 bytes | 1505 bytes | −4 |

Con **`-threads 1` el output es byte-idéntico**.

Las unidades de trabajo se completan en orden no determinista y algunas etapas escriben a estructuras
compartidas en ese orden. Consecuencias prácticas:

1. **Toda comparación de regresión tiene que ser con `-threads 1`.** `compilebench.py` avisa si
   detecta que no fue así.
2. Dos compilaciones del mismo mapa no dan BSPs idénticos, lo cual complica los builds reproducibles.
   No es un bug de corrección (los BSPs son válidos), pero sí una limitación conocida.

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
