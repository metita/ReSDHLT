# Merge de entidades estáticas (`-mergeentities`)

Guía para el que hace el mapa. Explica qué problema resuelve la opción, cuándo
usarla y en qué casos el compilador se va a negar a fusionar.

## El problema

Cada entidad brush del mapa cuesta **un modelo BSP**. El límite del formato son
512 modelos (`MAX_MAP_MODELS`), pero el techo que de verdad duele está en el
motor: la tabla de precache de modelos es una sola y la comparten los `*N` del
BSP con los modelos de jugadores, armas, props y sprites. Un mapa que gasta 300
slots en decoración deja al servidor sin lugar para lo demás.

Eso se paga por entidad, no por brush. Un `func_illusionary` de 1 brush y otro
de 80 brushes cuestan exactamente lo mismo: un slot.

El caso típico es un campo de arbustos. Se hacen con `func_illusionary` porque
es la única forma de tener transparencia — el `rendermode` es una propiedad de
entidad en tiempo de ejecución, así que un brush de `worldspawn` (o un
`func_detail`, que termina fusionado dentro de `worldspawn` en CSG) no puede
tenerlo. Si son 200 arbustos, son 200 entidades y 200 slots.

## Qué hace la opción

En CSG, antes de que nada empiece a repartir modelos, se buscan entidades brush
que sean **intercambiables** y se las fusiona en una sola. Los brushes no se
tocan: siguen siendo los mismos, con la misma geometría, las mismas texturas y
la misma iluminación. Lo único que cambia es de qué entidad cuelgan.

200 arbustos idénticos repartidos por el mapa pasan a ser un puñado de
entidades, una por zona.

## Uso

```
sdHLCSG mapa.map -mergeentities
```

Opciones:

| Opción | Efecto |
|---|---|
| `-mergeentities` | activa la fusión (por defecto está apagada) |
| `-mergesize #` | tamaño máximo del grupo fusionado, en unidades. Por defecto 1024. `0` = sin límite |
| `-mergeblend` | permite fusionar también los render modes que el motor mezcla (ver más abajo) |

`-mergesize` y `-mergeblend` activan la fusión por sí solos, no hace falta pasar
también `-mergeentities`.

Al terminar, CSG informa lo que hizo:

```
Merged 23 static brush entities into 4 (19 models freed)
```

Con `-verbose` además lista cada grupo, con su clase y el tamaño de la caja que
quedó:

```
merge: 8 func_illusionary entities -> entity 1, box 368 x 32 x 96
```

## Por qué existe `-mergesize`

Una entidad brush se descarta con el bounding box de su modelo: si la caja no
está en el PVS del jugador, el motor ni la manda ni la dibuja.

Si se fusionaran todos los arbustos del mapa en una sola entidad, esa caja sería
del tamaño del mapa y estaría en el PVS desde casi cualquier lado. El resultado
sería peor que no fusionar nada: se ahorran slots pero se dibuja decoración que
no se ve.

Por eso los candidatos se agrupan por cercanía y un grupo deja de crecer cuando
su caja pasaría de `-mergesize` unidades en cualquier eje. El default de 1024
mantiene los grupos dentro de algo parecido a una habitación o un sector. Si el
mapa es muy abierto se puede subir; si es de pasillos, bajarlo.

## Qué se fusiona y qué no

Se fusionan solo entidades que son demostrablemente equivalentes. Para entrar,
una entidad tiene que cumplir **todo** esto:

- Clase de la lista blanca: `func_illusionary` o `func_wall`. Son las dos clases
  que no se mueven, no piensan y no se activan.
- **Todos** los keyvalues idénticos a los de las otras del grupo. Dos arbustos
  con distinto `renderamt` van a grupos distintos.
- Sin `targetname`, `target`, `killtarget`, `globalname`, `parentname`,
  `master`, `netname` ni `message`. Cualquiera de esas claves ata la entidad al
  resto del mapa.
- Sin brush de origin (`origin` en 0 0 0).
- Sin `zhlt_usemodel` ni `zhlt_minsmaxs`.
- Sin `zhlt_nomerge` `1`.
- `rendermode` **Normal (0)** o **Solid (4)**, salvo que se pase `-mergeblend`.

Todo lo demás queda como estaba.

### Lo del `rendermode`

Las entidades transparentes que el motor **mezcla** (Texture, Additive, Glow,
Color) se ordenan por profundidad usando **un solo punto por entidad**. Si se
fusionan varias en una, todas pasan a compartir ese punto y se pueden dibujar en
el orden equivocado entre ellas.

Los dos modos que no tienen ese problema son:

- **Normal (0)**: opaco, no se ordena.
- **Solid (4)**: el alpha test que se usa con las texturas `{`. Recorta el píxel,
  no lo mezcla, así que tampoco se ordena.

Y **Solid es justamente el que usan los arbustos**, así que el caso que motiva
todo esto entra sin restricciones. Los otros modos quedan afuera salvo que se
pida `-mergeblend` a propósito.

### Opt-out manual

Para dejar una entidad concreta fuera de la fusión, agregarle:

```
zhlt_nomerge  1
```

Sirve cuando se quiere conservar una entidad separada por alguna razón que el
compilador no puede ver.

## Recomendaciones

- Los arbustos y la vegetación: `func_illusionary`, textura `{`, **rendermode
  Solid, renderamt 255**, sin nombre. Se fusionan solos.
- No hace falta agrupar a mano en el editor. Da igual, y a mano es más fácil
  equivocarse y meter en el grupo algo que tenga `targetname`.
- Si el mapa es muy abierto, probar `-mergesize 2048` y mirar `r_speeds`. Si
  aparece decoración dibujándose desde lejos, bajarlo.
- Con `-onlyents` la opción se ignora: ahí CSG solo reescribe el lump de
  entidades de un BSP que ya tiene los modelos armados, y cambiar la lista de
  entidades los desalinearía.

## Lo que esto no es

No da transparencia a `func_detail` ni a los brushes del mundo. Eso no se puede:
el `rendermode` sólo existe para entidades, y `func_detail` deja de ser una
entidad en CSG. Lo que hace esta opción es quitarle al `func_illusionary` casi
todo su costo, que es el motivo real por el que uno querría evitarlo.
