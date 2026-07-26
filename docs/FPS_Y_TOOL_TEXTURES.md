# Bajar `wpoly` y subir FPS en CS 1.6

Esta es la guía práctica para el que hace el mapa. Está separada del resto de la documentación
porque es, medida contra cualquier otra cosa, **la palanca más grande que existe sobre los FPS** —
y no está en el compilador, está en cómo se construye el mapa.

## Por qué esta guía existe

Las herramientas de compilación pueden hacer poco por los FPS. VIS ya calcula un PVS prácticamente
exacto, y la fusión de caras del BSP ya elimina entre el **19% y el 46%** de las caras según el mapa
(medido en tres mapas reales de este repo, ver §6). No hay un flag mágico que falte.

Lo que sí mueve la aguja es reducir **qué tiene que dibujar el motor en cada frame**. Eso lo decide
la geometría y el bloqueo de visibilidad, o sea el mapper.

## 1. Medir antes de optimizar

En consola:

```
developer 1
r_speeds 1
```

Vas a ver `wpoly` (polígonos de mundo) y `epoly` (entidades/modelos).

Referencias prácticas para CS 1.6:

| `wpoly` | Situación |
|---|---|
| < 600 | Cómodo |
| 600 – 800 | Aceptable |
| 800 – 1000 | Empieza a doler en máquinas modestas |
| > 1000 | Zona de problemas |

Reejecuta el mapa anotando `wpoly` en los mismos puntos antes y después de cada cambio. Sin eso estás
adivinando. El script `scripts/compilebench.py` de este repo te da el conteo total de caras del BSP,
que es útil como número global, pero `r_speeds` es el que te dice qué pasa **en cada lugar**.

## 2. NULL — la herramienta más rentable

`NULL` **borra la cara** en vez de dibujarla. Toda superficie que el jugador no puede ver debería ser
NULL: caras traseras de paredes, la parte de abajo del piso, los lados de bloques contra otros bloques,
las tapas de cajas a las que no se sube.

Es la mejora de FPS más directa y de riesgo cero: si el jugador no la ve, no debería costar nada.

> Atención: NULL elimina la cara pero **no** el bloqueo de visibilidad ni la colisión. El brush sigue siendo
> sólido y sigue bloqueando VIS. Es exactamente lo que quieres.

## 3. HINT y SKIP — controlar los cortes del BSP

- **HINT**: fuerza al BSP a cortar en ese plano, creando un límite de leaf donde ti quieras.
- **SKIP**: la cara no se dibuja ni se tiene en cuenta; se usa en las otras caras del brush de hint.

Un brush de hint típico: la cara que define el corte con `HINT`, las otras cinco con `SKIP`.

**Dónde ponerlos:** en los pasajes entre zonas grandes — puertas, pasillos, codos. La idea es que
cuando estés en la zona A, el PVS no incluya toda la zona B. Un hint bien puesto en una puerta puede
sacar cientos de `wpoly`.

**Dónde NO ponerlos:** por todas partes "por si acaso". Los hints de más fragmentan el BSP, suben el
conteo de caras y hacen VIS más lento, empeorando las dos cosas que querías arreglar.

## 4. SOLIDHINT y BEVELHINT — evitar subdivisión inútil

Aquí está el techo que conviene entender. El BSP subdivide las caras porque el motor no puede tener
lightmaps más grandes que cierto extent:

```c
#define TEXTURE_STEP        16   // luxels por unidad de lightmap
#define MAX_SURFACE_EXTENT  16   // limite del software renderer y del HLDS
#define DEFAULT_SUBDIVIDE_SIZE ((MAX_SURFACE_EXTENT-1)*TEXTURE_STEP)  // = 240
```

**No se puede subir `-subdivide` por encima de 240**: el mapa deja de cargar en software renderer y en
el HLDS. Así que "compilo con subdivide grande y gano FPS" no es una opción real.

Lo que sí se puede es evitar cortes innecesarios:

- **SOLIDHINT**: elimina la subdivisión de caras que provoca ese brush sobre las caras que toca.
  Clásico en terreno, rampas, escaleras y formas complejas, donde el BSP genera un picadillo de
  caras chiquitas sin necesidad.
- **BEVELHINT** (propio de SDHLT): actúa como `SOLIDHINT` **y** `BEVEL` a la vez — evita la
  subdivisión y bisela los clipnodes en la misma pasada. Es lo que quieres en terreno y en clipping de
  escaleras en espiral.

Estas dos suelen dar reducciones grandes de conteo de caras en geometría irregular, que es justo donde
`wpoly` se dispara.

## 5. El resto del kit

| Textura | Para qué |
|---|---|
| `CLIP` | Colisión sin cara visible. Para suavizar escaleras y bordes |
| `CLIPBEVEL` / `CLIPBEVELBRUSH` | Colisión biselada, evita quedarse trabado en esquinas |
| `NOCLIP` | Visible pero sin colisión |
| `BEVEL` | Bisela clipnodes, contra el trabarse en salientes |
| `SPLITFACE` | Subdivide las caras que toca a lo largo de sus bordes (equivalente a `zhlt_chopdown`) |
| `ORIGIN` | Define el punto de rotación de entidades brush |
| `CONTENTWATER` | Contenido de agua |
| `BOUNDINGBOX` | Define el bbox de una entidad |
| `%texname` | Minlight por textura. `%texname` = `_minlight 1.0`; `%#texname` con `#` de 0 a 255 |

## 6. Lo que hace el compilador por sí solo

Medido con este repo sobre tres mapas reales, la fusión de caras coplanares del BSP
(`MergeAll` / `TryMerge`) ya elimina:

| Mapa | Caras finales | Liberadas por fusión | Reducción |
|---|---|---|---|
| ba_coliseum | 1144 | 975 | 46% |
| koth_sandy | 1259 | 303 | 19% |
| ba_dust_island | 583 | 149 | 20% |

O sea: el compilador ya está haciendo un trabajo considerable, y llega a un punto fijo por plano
(cada fusión reintenta contra toda la lista). No esperes ganancias grandes de tocar ese algoritmo;
el margen está en no generar las caras de entrada.

## 7. VIS: `-full` y bloqueo de visibilidad

- Compila el release final con **`-full`** en VIS. Tarda más, da el PVS más ajustado, y eso es menos
  `wpoly` en juego.
- Lo que de verdad baja el PVS es **bloquear la línea de visión**: pasillos con codos, desniveles,
  paredes que cortan sightlines largas. Un mapa abierto donde se ve todo desde todas partes va a tener
  `wpoly` alto y no hay compilador que lo arregle.
- `info_portal` + `info_leaf` (propios de SDHLT) fuerzan visibilidad entre dos leafs concretos. Sirven
  para casos puntuales, no para bajar `wpoly` en general.

## 8. Sobre la iluminación

RAD **no afecta `wpoly`**. Los lightmaps no cambian la geometría que se dibuja. Subir la calidad de luz
te cuesta tiempo de compilación y algo de tamaño del BSP, pero **no FPS en juego**. Es el área donde
puedes ser generoso sin miedo:

- `-extra` (que en SDHLT sube `-bounce` a mínimo 12)
- `-chop` / `-texchop` más bajos para más detalle, a costa de tiempo y tamaño

## 9. Orden de trabajo sugerido

1. `r_speeds 1` y encontrar los puntos donde `wpoly` está alto.
2. `NULL` en todo lo invisible. Es lo más barato y no rompe nada.
3. `SOLIDHINT` / `BEVELHINT` en terreno y formas complejas.
4. `HINT` en los pasajes entre zonas grandes, **de a uno y midiendo**.
5. Si sigue alto: es diseño. Cortar sightlines.
6. Release final con `-full` en VIS.

Los pasos 2 y 3 casi siempre dan más que cualquier flag del compilador.
