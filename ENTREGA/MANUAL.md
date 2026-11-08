# Cámara Fusión — Manual

Toma una ráfaga de fotos con el celular, la pasa a la PC y el motor la fusiona en una sola foto con menos ruido,
mejor tono (HDR local), más nitidez y sin "fantasmas" por objetos en movimiento.

## Qué hay en esta carpeta

| Archivo | Para qué sirve |
|---|---|
| `CamaraFusion.apk` | La app para el celular (Redmi A5). |
| `camara_v2.exe` | El motor y el dashboard para la PC (no necesita instalar nada). |
| `INICIAR_DASHBOARD.bat` | Doble clic: abre el dashboard en el navegador. |

## 1. Instalar la app en el celular

**Sin cable:** pasa `CamaraFusion.apk` al celular (Bluetooth, Drive, WhatsApp a ti mismo, etc.), ábrelo desde
Archivos y acepta "Instalar apps de esta fuente". Al abrirla, permite el acceso a la cámara.

**Con cable:** activa *Opciones de desarrollador > Depuración USB* y ejecuta

```
adb install -r CamaraFusion.apk
```

## 2. Primera vez: revisa qué permite tu cámara

Toca **Info de cámara**. Verás, entre otras cosas, si el teléfono ofrece **RAW** y cuántas fotos por segundo
alcanza. Toca *Compartir informe* para enviarlo.

## 3. Tomar una ráfaga

1. Apunta y mantén el celular **lo más quieto que puedas** (apóyalo en algo si hay poca luz).
2. Opciones de abajo (tócalas para cambiarlas):
   - **Modo:**
     - *Normal*: exposición automática bloqueada (lo de siempre).
     - *HDR*: la ráfaga mezcla tres exposiciones (normal, corta y larga). El motor las une para recuperar
       los detalles de las **luces** (una ventana, una pantalla encendida) y de las **sombras** a la vez.
       Úsalo en escenas con mucho contraste. Mantén el celular muy quieto: las fotos largas se
       emborronan con el temblor.
     - *Noche*: exposición más larga con ISO más bajo (mismo brillo, menos ruido). **Apoya el celular**
       o usa un trípode.
   - **Fotos:** 4, 6, 8, 10, 12 o 16. Más fotos = menos ruido (baja con la raíz del número de fotos),
     pero el archivo pesa más. Para escenas con poca luz usa 12 o 16.
   - **Calidad:** Baja (~2 MP), Media (~5 MP), Alta (máximo del sensor).
   - **Ruido cámara:** *Apagado* desactiva el filtro del propio teléfono (que borra detalle) y deja que el motor
     de la PC limpie el ruido (recomendado); *Normal* usa el filtro del teléfono.
3. Toca el botón blanco ●. La app enfoca, bloquea exposición y color, y guarda la ráfaga.
4. El archivo queda en **Descargas/Camara/rafaga_AAAAMMDD_HHMMSS.camb** y la pantalla indica cuántas fotos
   por segundo logró.

## 4. Pasar el archivo a la PC (elige una vía)

- **Bluetooth, Nearby Share, WhatsApp, Drive…:** en la app toca **Compartir último** y elige el destino.
  (Un paquete de 6 fotos "Media" pesa unos 45 MB; por Bluetooth tarda varios minutos. Para rapidez usa
  cable, Nearby Share o Drive.)
- **Cable USB:** conecta el celular en modo *Transferencia de archivos*, abre la carpeta
  `Descargas\Camara` y copia el `.camb` a la PC.

## 5. Procesar en la PC

1. Doble clic en `INICIAR_DASHBOARD.bat` (se abre el navegador; deja la ventana negra abierta).
2. Arrastra el archivo `.camb` al recuadro.
3. Espera unos segundos. Verás la **foto única** frente a la **fusionada** con un deslizador y podrás
   descargar ambas en PNG.

**Control «Detalle» (arriba del recuadro):** elige entre suavidad y textura fina.
- *Suave*: máxima limpieza de ruido, aspecto más liso (puede borrar texturas finas).
- *Equilibrado* (recomendado): buena limpieza conservando la textura.
- *Máximo detalle*: conserva la textura fina (tela, piel, follaje, plástico mate) a costa de algo más de grano.

Si ves que se pierden detalles finos, usa *Máximo detalle*.

**Qué se puede y qué no se puede recuperar.** El motor deshace el emborronado del lente y conserva la
textura real, pero **no inventa detalle**. Si el procesador de imagen del teléfono ya alisó una zona
(por ejemplo, flores pequeñas o follaje, que salen como manchas "pintadas" incluso en la foto única),
esa información ya no existe en los datos. Un programa de IA generativa puede "rellenarla" y parecer más
nítido, pero inventa estructura y puede alterar textos y objetos reales. Por línea de comandos: `--detail 0`, `--detail 1` o `--detail 2`.

Por línea de comandos también se puede:

```
camara_v2.exe process rafaga.camb resultado.png
```

Opciones del revelado (todas opcionales):

| Opción | Qué hace | Valor por defecto |
|---|---|---|
| `--strength` | HDR local: sube las sombras (0 a 1) | 0.70 |
| `--sat` | Saturación del color (1.0 = neutro) | 1.25 |
| `--sharpen` | Nitidez (0 = ninguna) | 0.40 |
| `--clarity` | Claridad: contraste local de medios tonos (0 a 0.5) | 0.30 |
| `--denoise` | Limpieza de ruido multi-escala (0 = ninguna, 3 = fuerte) | 2.0 |
| `--nlm` | Limpieza previa por promedios no locales (0 = ninguna). Valores altos borran textura fina | 0.3 |
| `--sharpw` | Peso por nitidez en la fusión: una foto más borrosa que la referencia pesa menos (0 = apagado) | 3 |
| `--detail` | Preajuste 0 (Suave), 1 (Equilibrado) o 2 (Máximo detalle) | 1 |
| `--deconv` | Deconvolución: deshace el emborronado óptico del lente y del sensor, midiéndolo en los bordes de cada foto (0 = apagada, 1 = completa) | 0.7 |
| `--dsigma` | Emborronado a deshacer en píxeles (0 = medirlo automáticamente). **No lo subas de más:** sobrestimarlo empeora la foto | 0 (auto) |
| `--no-wb` | Desactiva el balance de blancos automático | activo |
| `--wiener 1` | **Experimental:** fusión por frecuencias (estilo HDR+) en vez de la fusión por píxel. En mis pruebas con tus ráfagas dio **más ruido y bordes algo peores** que la fusión por defecto, así que no se recomienda | desactivado |
| `--c` | Tolerancia al ruido de la fusión Wiener (solo con `--wiener 1`) | 8 |
| `--raw` | Desactiva todo el revelado y deja solo la fusión | — |

El balance de blancos busca zonas claras y casi neutras (blancos, grises luminosos) y quita el tinte de la
luz. Si la foto no tiene nada blanco o gris, no toca el color.

## Consejos

- Si la imagen sale con "fantasmas" o borrosa, usa menos fotos o mantén el celular más quieto.
- La luz baja es donde más se nota la mejora: prueba interiores y atardeceres.
- Los paquetes `.camb` conservan los datos originales: cuando mejore el motor puedes reprocesarlos.
