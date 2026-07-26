# Perfilar RAD en tu máquina

RAD es **más del 95%** del tiempo de compilación, así que es el único lugar donde queda tiempo real por
ganar. Esta guía es para que midas en tu CPU, que tiene más núcleos y menos ruido que el entorno donde
hice el análisis (2 núcleos, ±8% de varianza).

No necesitás instalar nada: el profiler está dentro de RAD.

---

## 1. Perfil rápido (sin recompilar)

Con los binarios normales ya tenés el nivel de fases:

```powershell
sdHLRAD -profile -threads 1 mimapa
```

Te va a dar algo así:

```
---- profile ----
site                              calls       Mticks    share   ticks/call
BuildFacelights                     653      45282.5    51.6%   69345405.7
GatherSampleLight                146152      42399.9    47.9%     290108.3
FinalLightFace                      653         15.8     0.0%      24239.2
```

`GatherSampleLight` está anidada dentro de `BuildFacelights`, así que ese 47.9% es ~94% del tiempo de
`BuildFacelights`. Traducción: **casi todo el tiempo de RAD se va en juntar luz por muestra**.

> Usá `-threads 1` para perfilar. Con varios hilos los tiempos se solapan y las cifras se vuelven
> difíciles de interpretar.

## 2. Perfil completo (con contadores internos)

Los contadores de las funciones internas del ray casting están apagados en compilación, a propósito:
`TestLine_r` se entra ~1.800 millones de veces por mapa, así que incluso un chequeo apagado costaría
casi un segundo por compilación. Para activarlos:

```powershell
cmake -B build-profile -S . -DSDHLT_PROFILE=ON
cmake --build build-profile -j
```

Y después:

```powershell
tools\sdHLRAD -profile -threads 1 mimapa
```

Ahora agrega:

```
TestLine                       62245301 (count only)
TestLine_r                   1849616672 (count only)
TestSegmentAgainstOpaqueList   38965499 (count only)
CheckVisBit                    43864129 (count only)
```

**Ojo:** este build es más lento por la instrumentación misma. Sirve para *entender* dónde va el trabajo,
no para medir tiempos. Para medir tiempos usá el build normal.

## 3. Lo que ya está descartado

Para que no gastes tiempo dos veces:

- **Convertir la recursión de `TestLine_r` en loop.** Tres de sus cuatro caminos son tail calls, así que
  parece la optimización obvia. La hice: la iluminación salió byte-idéntica (correcta) pero **más lenta**,
  tanto copiando endpoints (2.69s vs 2.59s) como con punteros (2.76s). GCC ya emite menos llamadas
  recursivas de lo que sugiere el código, y las llamadas son baratas por el *return stack buffer*.
  Revertido.
- **`-O3` y LTO.** Sin mejora medible; la varianza interna supera la diferencia. Detalle en
  `BENCHMARKS.md` §3.
- **Fusión de caras / `TryMerge`.** 0.7% de margen. Detalle en `BENCHMARKS.md` §5.

## 4. Dónde sí buscar

El dato importante del perfil:

| | ba_dust_island | koth_sandy |
|---|---|---|
| rayos `TestLine` por `GatherSampleLight` | **426** | 374 |
| descensos de árbol por rayo | 30 | 20 |

El costo está en **cuántos rayos se lanzan**, no en lo que cuesta cada uno. Así que las líneas con
sentido son algorítmicas:

1. **Lanzar menos rayos.** ¿Se pueden descartar antes las fuentes de luz que no pueden llegar a una
   muestra? Un test de PVS o de distancia previo al `TestLine` evitaría el rayo completo.
2. **Cachear visibilidad.** Muestras vecinas prueban las mismas luces contra la misma geometría. Si hay
   correlación explotable, se ahorran rayos enteros.
3. **Early-out más agresivo.** Si la contribución de una luz va a quedar por debajo del umbral de
   `-limiter`, el rayo no hace falta.

Cualquiera de esas **cambia la iluminación**, así que hay que decidir explícitamente cuánta desviación es
aceptable. Y ahí la validación cambia: ya no sirve exigir el lump `lighting` byte-idéntico.

## 5. Cómo medir un cambio, sin engañarse

Tres cosas que aprendí midiendo acá y te ahorran errores:

**Intercalá A/B.** Una medición no intercalada me dio una "regresión del 20%" que era otra compilación
corriendo en la misma máquina. Alterná binario viejo / binario nuevo en la misma tanda:

```powershell
python scripts\compilebench.py --tools tools_viejo --map m.map --threads 1 --runs 4 --json a.json
python scripts\compilebench.py --tools tools_nuevo --map m.map --threads 1 --runs 4 --json b.json
python scripts\compilebench.py --compare a.json b.json
```

**Mirá todas las corridas, no el promedio.** `compilebench.py` imprime `all_totals`. Si la dispersión
dentro de una variante supera la diferencia entre variantes, no mediste nada. Eso es exactamente lo que
pasó con `-O3` y LTO.

**Verificá correctitud aparte del tiempo.** Una optimización de ray casting que no pretende cambiar el
resultado debe dar el lump `lighting` idéntico:

```powershell
python scripts\compilebench.py --compare a.json b.json   # incluye hash por lump
python scripts\bspcheck.py mimapa.bsp                    # geometria
```

Si el hash de `lighting` cambia y no era la intención, la optimización está mal — sin importar cuánto
haya acelerado.
