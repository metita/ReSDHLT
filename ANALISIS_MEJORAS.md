# ReSDHLT — Análisis técnico y estado de las mejoras

Fork de [seedee/SDHLT](https://github.com/seedee/SDHLT) (linaje Vluzacn ZHLT v34).
Herramientas de compilación de mapas GoldSrc (**CSG, BSP, VIS, RAD** + RIPENT) para **CS 1.6**.

Objetivo del fork: mejorar la compilación y, donde sea realmente posible, los FPS de los mapas.

---

## 0. Marco honesto: qué determina los FPS en CS 1.6

| Factor de FPS | Lo fija principalmente | ¿Lo puede mejorar el compilador? |
|---|---|---|
| `wpoly` (polígonos de mundo) | VIS (PVS) + conteo de caras del BSP | Parcialmente: fusión de caras y PVS ajustado |
| `epoly` (entidades/modelos) | El mapa y los modelos | No |
| Overdraw en zonas abiertas | Diseño del mapa + hint brushes | No directamente |
| Carga de lightmaps | RAD (`chop`, `texchop`) | Indirecto |

**Conclusión que conviene tener clara:** en GoldSrc la mayor parte de la ganancia de FPS viene de
*prácticas de mapeo* (hint/skip, VIS blocking, dividir zonas grandes), no de cambiar el algoritmo del
compilador — VIS ya calcula un PVS prácticamente exacto. Además hay un **techo duro**: no se puede subir
`-subdivide` por encima de 240 sin romper el software renderer y el HLDS (§3.1).

Por eso el retorno real de este fork está, por orden: **(1) velocidad de compilación**, **(2) robustez /
correctitud**, **(3) calidad de iluminación** (que no cuesta FPS), y **(4) FPS**, que es el más acotado.

---

## 1. Implementado y verificado ✅

Todo lo de abajo está commiteado, compilado y probado. Los tres primeros son bugs que hacían que las
tools corrieran **monohilo** o **sin optimizar** sin que el usuario se enterara.

### 1.1 Linux compilaba siempre con 1 hilo
`src/sdhlt/common/threads.h` definía, para POSIX:

```c
#define DEFAULT_NUMTHREADS 1     // Win32 usaba -1
```

y `ThreadSetDefault()` autodetecta solo si `g_numthreads == -1`. Con el valor inicial en `1`, **esa rama
era código muerto**: ninguna build de Linux autodetectaba núcleos jamás. Salvo que pasaras `-threads N`
a mano, VIS y RAD (las fases largas) corrían en un solo hilo.

**Fix:** unificar el sentinel a `-1` y autodetectar con `std::thread::hardware_concurrency()`, con
fallback a `sysconf(_SC_NPROCESSORS_ONLN)`.
**Verificado:** `ThreadSetDefault()` ahora devuelve un conteo igual a `nproc` (2 en el entorno de prueba)
en vez de `1`.

### 1.2 Windows con más de 32 procesadores lógicos caía a 1 hilo
```c
if (g_numthreads < 1 || g_numthreads > 32) { g_numthreads = 1; }
```
Cualquier Ryzen 9 / Threadripper / Intel de gama alta compilaba **monohilo**. `GetSystemInfo()` además
solo ve el *processor group* del hilo actual, así que subcuenta en sistemas grandes.

**Fix:** preferir `hardware_concurrency()`, dejar `GetSystemInfo()` como fallback y hacer *clamp* a
`MAX_THREADS` en vez de colapsar a 1.
**Verificado con stubs de la API Win32:** 48 CPUs → 48 hilos (antes: 1); 512 → clamp a 256.

### 1.3 Stack buffer overflow con `-threads` fuera de rango
`RunThreadsOn()` guarda un handle por hilo en arrays **de tamaño fijo en el stack**
(`HANDLE threadhandle[MAX_THREADS]`, `pthread_t work_threads[MAX_THREADS]`), pero **ninguna de las 5
tools valida el límite superior** de `-threads`: solo rechazan valores menores a 1.

`sdHLVIS -threads 5000` escribía miles de entradas más allá del final del array.
**Reproducido:** SIGSEGV con core dump antes de despachar trabajo.

**Fix:** `ClampNumThreads()` al inicio de ambas implementaciones de `RunThreadsOn()` — un solo lugar
cubre las 5 tools, en vez de parchear 5 parsers de argumentos.
**Verificado:** ahora avisa y usa 256 en lugar de crashear.

### 1.4 Release de CMake se degradaba de `-O3` a `-O2`
El branch de Linux pasaba `-O2` por `add_compile_options()`. CMake los agrega **después** de
`CMAKE_CXX_FLAGS_RELEASE`, produciendo:

```
-O3 -DNDEBUG -Wall -O2 -fno-strict-aliasing -pthread -pipe
```

y **GCC respeta el último `-O`**, así que Release quedaba en `-O2`.
**Verificado con `g++ -Q --help=optimizers`:** `-O3` → 157 pases, `-O2` → 142, `-O3 -O2` → **142**.

**Fix:** quitar el `-O2` hardcodeado y dejar que el build type decida.
**Verificado:** Release da `-O3 -DNDEBUG ...`; Debug sigue en `-g` sin `-O`.

### 1.5 `CMAKE_BUILD_TYPE` sin default → binarios sin optimizar
Con generadores mono-config (Make/Ninja), un `CMAKE_BUILD_TYPE` vacío significa **cero flags de
optimización**. Un `cmake -B build && cmake --build build` producía tools mucho más lentas de lo previsto.
**Fix:** default a `Release` solo en generadores mono-config. Además opción opt-in `SDHLT_LTO`
protegida con `check_ipo_supported()`.

### 1.6 Dispatch de trabajo con atomic en lugar del lock global
`GetThreadWork()` tomaba el lock global **en cada unidad de trabajo**, básicamente para hacer
`dispatch++`. Con muchos hilos y millones de unidades (RAD, VIS) el dispatcher se volvía punto de
serialización.

**Fix:** reclamar el índice con `dispatch.fetch_add()`, dejando el lock solo para el pacifier (que sí
comparte estado y consola).

Medido en 2 núcleos, 2M unidades triviales, solo tiempo de dispatcher:

| hilos | con lock | con atomic |
|---|---|---|
| 2 | 0.064 s | 0.059 s |
| 16 | 0.060 s | 0.054 s |
| 64 | 0.059 s | 0.046 s |

La ganancia acá es **modesta** — el dispatch es chico al lado del trabajo real de lighting/visibilidad, y
2 núcleos no generan mucha contención; debería ampliarse en CPUs de muchos núcleos. Lo importante es la
correctitud: **verificado sobre 200k unidades con 1/2/4/16/64 hilos que cada unidad se despacha
exactamente una vez**, sin faltantes ni duplicados.

### 1.7 CI corría `ctest` sin tests
El workflow ejecutaba `ctest` sin ninguna suite registrada → fallaba con "No tests were found" en cada
run. Reemplazado por un smoke test que verifica que las 5 tools se generaron y arrancan.
**Verificado localmente: 5/5.**

> *Corrección respecto del análisis inicial:* la subida de artefactos en CI **ya existía** upstream; lo
> que estaba roto era el paso de test. Lo dejo anotado para no atribuirme algo que ya estaba.

---

## 2. Impacto esperado

El impacto real depende de tu hardware, y conviene medirlo (§5) en vez de asumirlo:

- **Si compilás en Linux** sin pasar `-threads`: la mejora es directamente proporcional a tus núcleos
  (antes usabas 1). En 8 núcleos, esperá algo cercano a **varias veces más rápido** en VIS y RAD.
- **Si compilás en Windows con >32 hilos lógicos**: misma situación.
- **Si compilás en Windows con ≤32 hilos y pasabas `-threads`**: ya estabas usando bien la CPU; acá la
  ganancia es la de `-O3` real (§1.4) más el atomic (§1.6), o sea **unos pocos puntos porcentuales**.
- **FPS en el juego:** **cero cambio por ahora**. Nada de lo implementado altera la geometría, el PVS ni
  los lightmaps del BSP resultante. Los binarios son más rápidos y más robustos, pero el `.bsp` que
  producen es equivalente. Las mejoras de FPS están en §3, todavía sin implementar.

---

## 3. Pendiente — FPS y calidad

### 3.1 El techo de subdivisión (contexto necesario)
`bsp5.h` / `bspfile.h`:
```c
#define TEXTURE_STEP        16
#define MAX_SURFACE_EXTENT  16   // límite del software renderer / HLDS
#define DEFAULT_SUBDIVIDE_SIZE ((MAX_SURFACE_EXTENT-1)*TEXTURE_STEP)  // = 240
```
Menos caras ⇒ menos `wpoly` ⇒ más FPS, pero **no se puede pasar de 240** sin que el mapa no cargue en
software renderer ni en HLDS. La ganancia hay que buscarla en **fusión**, no en subdividir menos.

### 3.2 Fusión de caras coplanares (`sdHLBSP/merge.cpp`)
`TryMerge` / `MergePlaneFaces` unen caras coplanares tras el BSP. A revisar: si la fusión se frena por
diferencias de textura/lightmap que podrían tolerarse. **Riesgo medio** — tocar BSP puede introducir
grietas; requiere tests de regresión visual.

### 3.3 Documentar las tool-textures
`SOLIDHINT` / `BEVELHINT` / `SPLITFACE` ya existen y eliminan subdivisión innecesaria en terreno y
escaleras. Documentarlas bien es la vía más barata a FPS reales, porque el que baja `wpoly` es el mapper.

### 3.4 RAD: calidad sin costo de FPS
`qrad.h`: `DEFAULT_BOUNCE 8`, `DEFAULT_CHOP 64`, `DEFAULT_TEXCHOP 32`, `eMethodSparseVismatrix`.
RAD **no afecta `wpoly`**, así que más calidad de luz no cuesta FPS en runtime: solo tiempo de
compilación y algo de tamaño de BSP. Área segura para mejorar.

### 3.5 Perfilar `BuildFacelights` y `LeafThread`
El README upstream ya los lista como objetivos. Son el hot-path de RAD y VIS. Perfilar antes de tocar.

---

## 4. Roadmap restante

| # | Mejora | Área | Impacto | Riesgo |
|---|--------|------|---------|--------|
| 1 | Unificar threading en `std::thread` (elimina ~250 líneas y el `#ifdef`) | Compilación | ⭐⭐ | Medio |
| 2 | Perfilar y optimizar `BuildFacelights` | RAD | ⭐⭐⭐ | Medio |
| 3 | Mejorar fusión de caras coplanares | **FPS** | ⭐⭐ | Medio |
| 4 | Documentar tool-textures para mappers | **FPS** | ⭐⭐ | Bajo |
| 5 | Evaluar `-O3` vs `-O2` y LTO con medición real | Build | ⭐ | Bajo |
| 6 | Target `install` + empaquetado de release | Build | ⭐ | Bajo |

---

## 5. Cómo medir (importante)

- **Compilación:** cronometrar CSG+BSP+VIS+RAD sobre un mapa de referencia, antes y después.
  Guardar el `-chart` para comparar límites.
- **FPS / wpoly:** en CS 1.6, `developer 1` + `r_speeds 1`, recorrido fijo, anotar `wpoly` en los mismos
  puntos. Comparar mismo recorrido antes/después.
- **Regresión:** un cambio en BSP/VIS/RAD no debe introducir leaks ni caras faltantes. Diff del `.bsp` y
  revisión visual son obligatorios antes de mergear los ítems 2 y 3 del roadmap.

---

## 6. Limitaciones de la verificación hecha

Para ser transparente sobre qué está probado y qué no:

- Todo se compiló y probó en **Linux/GCC 11**, 2 núcleos. El branch de Windows se verificó por
  **sintaxis y lógica con stubs**, no compilando con MSVC — eso lo cubre el CI de GitHub Actions.
- **No se compiló ningún mapa real** (falta un `.map` de referencia y los WADs). Las mejoras de threading
  y build están verificadas a nivel unitario, no end-to-end sobre un mapa de CS.
- Los números de §1.6 son de un microbenchmark del dispatcher, no de una compilación real.

*Código citado verificado contra el árbol del fork.*
