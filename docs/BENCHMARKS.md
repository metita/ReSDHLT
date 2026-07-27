# Mediciones

Todo lo de aquí se midió con `scripts/compilebench.py` sobre mapas reales de CS 1.6.
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
(0.10s).** No hay evidencia de que `-O3` o LTO mejoren nada aquí.

**Decisión:** LTO queda **opt-in** (`-DSDHLT_LTO=ON`), no por defecto. Encender LTO por defecto
alargaría los builds sin beneficio demostrado. `-O3` queda simplemente porque es el default de
`Release` de CMake.

Tamaño del binario `sdHLRAD`: `-O2` 442KB, `-O3` 482KB, LTO 438KB.

> Si quieres decidir para *tu* máquina, ejecuta esto y mira si la diferencia supera tu propia varianza:
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

Tres de los cuatro caminos recursitú de `TestLine_r` son **tail calls**, así que lo reescribe como loop.
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

### 4.4 De dónde salen los rayos, y la ganancia real ✅

Contando los rayos por origen apareció el dato que importaba. `koth_sandy`, de 17.068.356 `TestLine`:

| origen | rayos | % |
|---|---|---|
| **normales de skylight** | **16.358.753** | **95.8%** (67% de ellos ocluidos) |
| normales de spread del sol | 26.628 | 0.2% |
| todo lo demás | 682.975 | 4.0% |

O sea el **96% del ray casting es el loop de luz de cielo**, que lanza un rayo por cada normal de un
hemisferio subdividido. `g_numskynormals` es `{0, 6, 18, 66, 258, 1026, 4098, 16386, 65538}` y `-softsky`
eligea nivel 7 encendido o nivel 4 apagado: **16.386 rayos o 258**, un salto de 63× sin nada en medio.

**Curva medida** (`koth_sandy`, 2 hilos, comparado contra nivel 7):

| nivel | rayos | RAD | vs nivel 7 | dif media | dif max |
|---|---|---|---|---|---|
| 4 | 258 | 0.67s | **2.29×** | 0.43/255 | 5/255 |
| 5 | 1.026 | 0.70s | **2.19×** | 0.14/255 | 2/255 |
| **6** | **4.098** | **0.83s** | **1.84×** | **0.05/255** | **1/255** |
| 7 | 16.386 | 1.43s | 1.00× | 0 | 0 |

El nivel 6 es donde la curva quiebra: 4× menos rayos por una diferencia máxima de **1/255 por luxel**,
que no se puede ver. Confirmado en el mapa pesado `ba_dust_island`: 10.07s → 6.05s, máximo 1/255, y solo
3.4% de los luxels cambian.

**Implementado:** `-skylevel N` (4 a 8) y **default cambiado a 6**.

| mapa | default nuevo (6) | `-skylevel 7` | ganancia |
|---|---|---|---|
| koth_sandy | 0.99s | 1.64s | **1.65×** |
| ba_dust_island | 6.21s | 10.09s | **1.63×** |

Esta es **la única desviación deliberada** de la equivalencia con upstream en todo el fork. Verificado que
`-skylevel 7` reproduce el lump `lighting` de upstream **byte a byte** (sha1 `40c7dc1ac1a3da50` en ambos),
así que el comportamiento viejo está a un flag de distancia.

### 4.5 Culling de cielo por leaf: medido y descartado ❌

La idea siguiente parecía la mejor: el 67% de los rayos de cielo terminan ocluidos, así que para una
muestra en interior se lanzan miles de rayos para descubrir que no se ve el cielo. Un test de visibilidad
por leaf saltaría el loop entero **sin cambiar la iluminación**.

**Mide el techo antes de escribir una línea de código.** Conté cuántas entradas al loop terminan con
**cero** impactos, o sea el loop completo desperdiciado:

| mapa | entradas al loop | con cero impactos | techo del culling |
|---|---|---|---|
| koth_sandy | 2.028 | 466 | **23%** |
| ba_dust_island | 6.623 | 137 | **2.1%** |

**En el mapa pesado el 98% de las muestras consigue al menos un impacto de cielo.** No hay loop
desperdiciado que saltar. Y tiene sentido en retrospectiva: los mapas caros son caros *porque* son
exteriores y ven cielo. Las muestras de interior, donde el culling ayudaría, son minoría justo en los
mapas donde importaría.

El 67% de rayos ocluidos es desperdicio **dentro** de cada loop, no loops enteros desperdiciados: una
muestra ve algo de cielo pero la mayoría de sus 4.098 direcciones están bloqueadas. Eso no lo arregla un
culling por muestra; haría falta culling por dirección, que es el mismo problema que el test del rayo.

**Decisión: no implementarlo.** El techo de 2.1% en el mapa que importa no justifica el riesgo.

### 4.6 Lo que quedaría

Una vía sigue abierta pero **cambia la iluminación**, así que ya no se valida con hash: los niveles de
normales de cielo son una **jerarquía de subdivisión** (258 → 1.026 → 4.098 → 16.386). Un enfoque
coarse-to-fine podría testear primero un nivel grueso y, en las regiones totalmente ocluidas, saltar sus
subdivisiones finas. Es real pero complejo, y aproxima, así que hay que decidir cuánta desviación se
acepta. Comparado con eso, `-skylevel` da 1.65× con una línea de configuración.

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

**Fusiones residuales en el BSP final** (`scripts/bspcheck.py`). Aquí estuvo el hallazgo importante.
Un primer conteo naive dio **697** pares "fusionables" en ba_coliseum, lo cual habrea sido enorme.
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
- `WriteFace()` hace `fprintf` directo a los architú `.p0`–`.p3` bajo lock: el orden de finalización
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

## zhlt_embedlightmap: por qué pesa tanto

`zhlt_embedlightmap` existe porque el motor no aplica lightmaps a las entidades
con render mode especial (aditivo, texturizado, etc.). RAD resuelve eso
**horneando la luz dentro de la textura**, y para eso genera **una textura nueva
por cara**, no por entidad ni por textura original.

Medido sobre `ba_dust_island`, poniendo la clave en sus 6 `func_breakable`
(190 caras), con `-fast -bounce 1 -skylevel 4`:

| `zhlt_embedlightmapresolution` | .bsp | texturas | bytes de textura |
|---|---|---|---|
| sin la clave | 673 KB | 0 | 0 |
| 1 (default) | 12.540 KB | 190 | 11.859 KB |
| 2 | 3.759 KB | 190 | 3.078 KB |
| 4 | 1.570 KB | 190 | 889 KB |
| 8 | 1.255 KB | 190 | 573 KB |
| 16 | 994 KB | 190 | 312 KB |

**El mapa pasó de 673 KB a 12,5 MB: 18 veces más grande.** Con `resolution 1`
cada cara se guarda a la resolución nativa de su textura (16 píxeles por luxel),
más 4 niveles de mipmap y una paleta de 256 colores propia (768 bytes) por
textura.

La resolución es lo único que mueve la aguja de verdad: cada duplicación divide
por cuatro. Baja el detalle de la textura base, no el de la luz, que de por sí
sólo tiene una muestra cada 16 unidades.

### Lo que cambió este fork

Dos cosas, ninguna toca la calidad de la luz:

| | res 1 | res 2 | res 4 | res 8 |
|---|---|---|---|---|
| antes | 11.859 KB | 3.078 KB | 889 KB | 573 KB |
| ahora | 9.048 KB | 2.210 KB | 687 KB | 380 KB |
| | **-24%** | **-28%** | **-23%** | **-34%** |

1. **Sin redondeo a potencia de dos.** Cada textura se redondeaba hacia arriba a
   la potencia de dos siguiente, y una cara de 272x144 terminaba ocupando
   512x256: 2,4 veces los píxeles necesarios. GoldSrc sólo exige múltiplos de
   16, que el código ya garantizaba. `"zhlt_embedlightmappoweroftwo" "1"`
   devuelve el comportamiento anterior si hiciera falta.
2. **Texturas idénticas compartidas.** Las caras repetidas (cajones, columnas,
   rejas) horneaban los mismos bytes una y otra vez. Ahora se comparan y se
   reutiliza la misma textura: 12 caras compartidas en este mapa a resolución 1,
   más a resoluciones altas. Sólo se comparte entre caras que vengan del mismo
   texinfo original, porque el nombre de la textura generada codifica cuál era y
   las tools lo leen de vuelta.

Verificado con `scripts/bspcheck.py` (geometría sana) y revisando el lump de
texturas: dimensiones múltiplo de 16, mips dentro del lump, todos los
`texinfo->miptex` en rango. Un mapa sin la clave compila byte por byte igual que
antes.

### Agua: por qué dejaba de moverse

`zhlt_embedlightmap` sobre un `func_water` rompía el agua, y la causa estaba en
dos bloques comentados en `loadtextures.cpp`.

GoldSrc decide que una superficie es agua **por el `!` al principio del nombre
de la textura**, y lee el color y la densidad de la niebla de las entradas 3 y 4
de la paleta. RAD hacía las dos cosas mal:

```
antes:  !leanwater_w5   paleta[3],[4] = [25,69,155, 31,65,132]
        __rad00026MLe00 paleta[3],[4] = [9,71,117, 14,70,117]   <- ya no es agua

ahora:  !_rad00026oI400 paleta[3],[4] = [25,69,155, 31,65,132]  <- sigue siendo agua
```

Sin el `!` el motor la trata como una superficie común: se acabaron las olas, la
niebla y el comportamiento de agua al mirarla desde abajo. Y la paleta
requantizada se llevaba puesta la niebla aunque el nombre se hubiera arreglado.

Ahora el nombre conserva el `!` y las primeras 16 entradas de la paleta se
copian tal cual desde la textura original; los colores horneados usan las 240
restantes.

**Queda por comprobar en juego:** el motor dibuja el agua con una distorsión
senoidal de hasta 8 téxeles, y la textura horneada no repite como la original,
así que en los bordes de cada cara puede verse algo de bleeding. El horneado
deja un margen de `(texturasize * resolution - extent) / 2` píxeles a cada lado,
que en la mayoría de los casos alcanza. Si ves costuras en el borde del agua,
subí `zhlt_embedlightmapresolution` o sacá la clave de esa entidad.

Un aviso de tamaño: un solo brush de agua grande se subdivide en muchas caras
(23 en el mapa de prueba), y cada una hornea su propia textura. El agua es de lo
más caro donde poner esta clave.
