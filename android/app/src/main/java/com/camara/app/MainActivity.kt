package com.camara.app

import android.Manifest
import android.app.Activity
import android.app.AlertDialog
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Color
import android.graphics.ImageFormat
import android.graphics.SurfaceTexture
import android.graphics.Typeface
import android.hardware.camera2.CameraCaptureSession
import android.hardware.camera2.CameraCharacteristics
import android.hardware.camera2.CameraDevice
import android.hardware.camera2.CameraManager
import android.hardware.camera2.CameraMetadata
import android.hardware.camera2.CaptureRequest
import android.hardware.camera2.CaptureResult
import android.hardware.camera2.TotalCaptureResult
import android.media.ImageReader
import android.net.Uri
import android.os.Bundle
import android.os.Handler
import android.os.HandlerThread
import android.provider.MediaStore
import android.util.Size
import android.view.Gravity
import android.view.Surface
import android.view.TextureView
import android.view.View
import android.view.ViewGroup
import android.widget.Button
import android.widget.FrameLayout
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import android.widget.Toast
import kotlin.math.abs

class MainActivity : Activity() {

    // ---- interfaz ----
    private lateinit var texture: AutoFitTextureView
    private lateinit var statusView: TextView
    private lateinit var btnShoot: Button
    private lateinit var btnShare: Button
    private lateinit var btnInfo: Button
    private lateinit var btnCount: Button
    private lateinit var btnRes: Button
    private lateinit var btnNr: Button

    // ---- cámara ----
    private val bgThread = HandlerThread("camara-bg").also { it.start() }
    private val handler = Handler(bgThread.looper)
    private lateinit var manager: CameraManager
    private var cameraId: String? = null
    private lateinit var chars: CameraCharacteristics
    private var device: CameraDevice? = null
    private var session: CameraCaptureSession? = null
    private var reader: ImageReader? = null
    private var previewBuilder: CaptureRequest.Builder? = null
    private var yuvSizes: List<Size> = emptyList()
    private var captureSize: Size = Size(640, 480)
    private var previewSize: Size = Size(640, 480)
    private var sensorOrientation = 90
    private var afSupported = false
    private var noiseModes: List<Int> = emptyList()
    private var edgeModes: List<Int> = emptyList()

    // ---- opciones ----
    private val counts = listOf(4, 6, 8, 10, 12, 16)
    private var countIdx = 1
    private val resNames = listOf("Baja", "Media", "Alta")
    private var resLevel = 1
    private var nrMinimal = true
    private var sizeOverride: Size? = null

    // ---- estado de la ráfaga ----
    @Volatile private var busy = false
    @Volatile private var waitingAf = false
    @Volatile private var fired = false
    @Volatile private var lastIso = 100
    @Volatile private var lastExposureNs = 10_000_000L
    private var writer: BurstWriter? = null
    private var framesWritten = 0
    private var tFirst = 0L
    private var tLast = 0L
    private var lastUri: Uri? = null
    private var lastReport: String = ""

    private val count get() = counts[countIdx]

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        manager = getSystemService(CAMERA_SERVICE) as CameraManager
        buildUi()
        pickCamera()
        if (checkSelfPermission(Manifest.permission.CAMERA) != PackageManager.PERMISSION_GRANTED) {
            requestPermissions(arrayOf(Manifest.permission.CAMERA), 1)
        }
    }

    override fun onRequestPermissionsResult(code: Int, perms: Array<out String>, results: IntArray) {
        if (results.isNotEmpty() && results[0] == PackageManager.PERMISSION_GRANTED) {
            if (texture.isAvailable) openCamera()
        } else {
            setStatus("Sin permiso de cámara: ábrelo en Ajustes > Apps > Cámara Fusión > Permisos")
        }
    }

    override fun onResume() {
        super.onResume()
        if (texture.isAvailable && checkSelfPermission(Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED) openCamera()
    }

    override fun onPause() {
        closeCamera()
        super.onPause()
    }

    override fun onDestroy() {
        bgThread.quitSafely()
        super.onDestroy()
    }

    // =====================================================================================
    //  Interfaz (sin XML: se construye en código)
    // =====================================================================================

    private fun dp(v: Int) = (v * resources.displayMetrics.density).toInt()

    private fun styledButton(text: String): Button = Button(this).apply {
        this.text = text
        isAllCaps = false
        setTextColor(Color.WHITE)
        setBackgroundColor(Color.parseColor("#33FFFFFF"))
        textSize = 13f
        setPadding(dp(6), dp(6), dp(6), dp(6))
    }

    private fun buildUi() {
        val root = FrameLayout(this).apply { setBackgroundColor(Color.BLACK) }

        texture = AutoFitTextureView(this)
        root.addView(texture, FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT, Gravity.TOP))

        statusView = TextView(this).apply {
            setTextColor(Color.WHITE)
            setBackgroundColor(Color.parseColor("#99000000"))
            textSize = 13f
            setPadding(dp(12), dp(8), dp(12), dp(8))
            text = "Iniciando cámara…"
        }
        root.addView(statusView, FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT, Gravity.TOP))

        val panel = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setBackgroundColor(Color.parseColor("#CC000000"))
            setPadding(dp(8), dp(8), dp(8), dp(10))
        }
        val opts = LinearLayout(this).apply { orientation = LinearLayout.HORIZONTAL }
        btnCount = styledButton("")
        btnRes = styledButton("")
        btnNr = styledButton("")
        for (b in listOf(btnCount, btnRes, btnNr)) {
            opts.addView(b, LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f).apply { setMargins(dp(3), 0, dp(3), dp(6)) })
        }
        panel.addView(opts)

        val main = LinearLayout(this).apply { orientation = LinearLayout.HORIZONTAL; gravity = Gravity.CENTER_VERTICAL }
        btnInfo = styledButton("Info de\ncámara")
        btnShare = styledButton("Compartir\núltimo")
        btnShoot = Button(this).apply {
            text = "●"
            textSize = 30f
            setTextColor(Color.parseColor("#E53935"))
            setBackgroundColor(Color.WHITE)
        }
        main.addView(btnInfo, LinearLayout.LayoutParams(0, dp(56), 1f).apply { setMargins(dp(3), 0, dp(3), 0) })
        main.addView(btnShoot, LinearLayout.LayoutParams(dp(84), dp(64)).apply { setMargins(dp(8), 0, dp(8), 0) })
        main.addView(btnShare, LinearLayout.LayoutParams(0, dp(56), 1f).apply { setMargins(dp(3), 0, dp(3), 0) })
        panel.addView(main)
        root.addView(panel, FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT, Gravity.BOTTOM))

        root.setOnApplyWindowInsetsListener { v, insets ->
            @Suppress("DEPRECATION")
            v.setPadding(insets.systemWindowInsetLeft, insets.systemWindowInsetTop, insets.systemWindowInsetRight, insets.systemWindowInsetBottom)
            insets
        }
        setContentView(root)

        btnCount.setOnClickListener { countIdx = (countIdx + 1) % counts.size; refreshLabels(); restartPreview() }
        btnRes.setOnClickListener { resLevel = (resLevel + 1) % 3; sizeOverride = null; refreshLabels(); restartPreview() }
        btnNr.setOnClickListener { nrMinimal = !nrMinimal; refreshLabels() }
        btnShoot.setOnClickListener { shoot() }
        btnInfo.setOnClickListener { showInfo() }
        btnShare.setOnClickListener { shareLast() }
        refreshLabels()

        texture.surfaceTextureListener = object : TextureView.SurfaceTextureListener {
            override fun onSurfaceTextureAvailable(st: SurfaceTexture, w: Int, h: Int) {
                if (checkSelfPermission(Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED) openCamera()
            }
            override fun onSurfaceTextureSizeChanged(st: SurfaceTexture, w: Int, h: Int) {}
            override fun onSurfaceTextureDestroyed(st: SurfaceTexture): Boolean = true
            override fun onSurfaceTextureUpdated(st: SurfaceTexture) {}
        }
    }

    private fun refreshLabels() {
        btnCount.text = "Fotos: $count"
        btnRes.text = "Calidad: ${resNames[resLevel]}\n" + sizeLabel(chooseCaptureSize())
        btnNr.text = "Ruido cámara:\n" + if (nrMinimal) "Apagado" else "Normal"
    }

    private fun sizeLabel(s: Size) = "${s.width}×${s.height}"

    private fun setStatus(msg: String) = runOnUiThread { statusView.text = msg }

    private fun setBusyUi(b: Boolean) = runOnUiThread {
        for (v in listOf<View>(btnShoot, btnCount, btnRes, btnNr, btnInfo, btnShare)) v.isEnabled = !b
    }

    // =====================================================================================
    //  Elección de cámara y tamaños
    // =====================================================================================

    private fun pickCamera() {
        var best: String? = null
        var bestPx = -1L
        for (id in manager.cameraIdList) {
            val c = manager.getCameraCharacteristics(id)
            if (c.get(CameraCharacteristics.LENS_FACING) != CameraCharacteristics.LENS_FACING_BACK) continue
            val map = c.get(CameraCharacteristics.SCALER_STREAM_CONFIGURATION_MAP) ?: continue
            val top = map.getOutputSizes(ImageFormat.YUV_420_888)?.maxOfOrNull { it.width.toLong() * it.height } ?: continue
            if (top > bestPx) { bestPx = top; best = id }
        }
        cameraId = best ?: manager.cameraIdList.firstOrNull()
        val id = cameraId ?: run { setStatus("Este teléfono no tiene cámara disponible"); return }
        chars = manager.getCameraCharacteristics(id)
        sensorOrientation = chars.get(CameraCharacteristics.SENSOR_ORIENTATION) ?: 90
        afSupported = chars.get(CameraCharacteristics.CONTROL_AF_AVAILABLE_MODES)?.contains(CameraMetadata.CONTROL_AF_MODE_AUTO) == true
        noiseModes = chars.get(CameraCharacteristics.NOISE_REDUCTION_AVAILABLE_NOISE_REDUCTION_MODES)?.toList() ?: emptyList()
        edgeModes = chars.get(CameraCharacteristics.EDGE_AVAILABLE_EDGE_MODES)?.toList() ?: emptyList()
        val map = chars.get(CameraCharacteristics.SCALER_STREAM_CONFIGURATION_MAP)!!
        yuvSizes = (map.getOutputSizes(ImageFormat.YUV_420_888) ?: emptyArray()).sortedByDescending { it.width.toLong() * it.height }
    }

    private fun chooseCaptureSize(): Size {
        sizeOverride?.let { return it }
        if (yuvSizes.isEmpty()) return Size(640, 480)
        val top = yuvSizes.first()
        val aspect = top.width.toFloat() / top.height
        val cands = yuvSizes.filter { abs(it.width.toFloat() / it.height - aspect) < 0.02f }.ifEmpty { yuvSizes }
        val maxPx = top.width.toLong() * top.height
        val target = when (resLevel) { 0 -> 2_000_000L; 1 -> 5_000_000L; else -> maxPx }.coerceAtMost(maxPx)
        return cands.minByOrNull { abs(it.width.toLong() * it.height - target) } ?: top
    }

    private fun choosePreviewSize(cap: Size): Size {
        val map = chars.get(CameraCharacteristics.SCALER_STREAM_CONFIGURATION_MAP)!!
        val sizes = map.getOutputSizes(SurfaceTexture::class.java)?.toList() ?: listOf(Size(640, 480))
        val aspect = cap.width.toFloat() / cap.height
        val ok = sizes.filter { abs(it.width.toFloat() / it.height - aspect) < 0.02f && it.width <= 1280 }
        return ok.maxByOrNull { it.width.toLong() * it.height }
            ?: sizes.filter { it.width <= 1280 }.maxByOrNull { it.width.toLong() * it.height }
            ?: sizes.first()
    }

    // =====================================================================================
    //  Apertura y vista previa
    // =====================================================================================

    private fun openCamera() {
        if (device != null) return
        val id = cameraId ?: return
        try {
            @Suppress("MissingPermission")
            manager.openCamera(id, object : CameraDevice.StateCallback() {
                override fun onOpened(camera: CameraDevice) { device = camera; startPreview() }
                override fun onDisconnected(camera: CameraDevice) { camera.close(); device = null }
                override fun onError(camera: CameraDevice, error: Int) { camera.close(); device = null; setStatus("Error de cámara ($error)") }
            }, handler)
        } catch (e: Exception) {
            setStatus("No se pudo abrir la cámara: ${e.message}")
        }
    }

    private fun closeCamera() {
        try { session?.close() } catch (_: Exception) {}
        session = null
        try { device?.close() } catch (_: Exception) {}
        device = null
        try { reader?.close() } catch (_: Exception) {}
        reader = null
        busy = false
    }

    private fun restartPreview() {
        if (device == null) return
        try { session?.close() } catch (_: Exception) {}
        session = null
        startPreview()
    }

    private fun startPreview() {
        val dev = device ?: return
        val st = texture.surfaceTexture ?: return
        captureSize = chooseCaptureSize()
        previewSize = choosePreviewSize(captureSize)
        runOnUiThread { texture.setAspectRatio(previewSize.height, previewSize.width); refreshLabels() }
        st.setDefaultBufferSize(previewSize.width, previewSize.height)
        val previewSurface = Surface(st)

        try { reader?.close() } catch (_: Exception) {}
        val rd = ImageReader.newInstance(captureSize.width, captureSize.height, ImageFormat.YUV_420_888, minOf(count + 2, 10))
        reader = rd

        @Suppress("DEPRECATION")
        dev.createCaptureSession(listOf(previewSurface, rd.surface), object : CameraCaptureSession.StateCallback() {
            override fun onConfigured(s: CameraCaptureSession) {
                session = s
                try {
                    val b = dev.createCaptureRequest(CameraDevice.TEMPLATE_PREVIEW)
                    b.addTarget(previewSurface)
                    b.set(CaptureRequest.CONTROL_MODE, CameraMetadata.CONTROL_MODE_AUTO)
                    if (afSupported) b.set(CaptureRequest.CONTROL_AF_MODE, CameraMetadata.CONTROL_AF_MODE_CONTINUOUS_PICTURE)
                    previewBuilder = b
                    s.setRepeatingRequest(b.build(), metaCallback, handler)
                    setStatus("Lista · ${sizeLabel(captureSize)} · $count fotos. Mantén el celular quieto al disparar.")
                } catch (e: Exception) {
                    setStatus("No se pudo iniciar la vista previa: ${e.message}")
                }
            }

            override fun onConfigureFailed(s: CameraCaptureSession) {
                // Este tamaño no lo admite junto a la vista previa: probar uno más pequeño.
                val smaller = yuvSizes.firstOrNull { it.width.toLong() * it.height < captureSize.width.toLong() * captureSize.height &&
                    abs(it.width.toFloat() / it.height - captureSize.width.toFloat() / captureSize.height) < 0.02f }
                if (smaller != null) {
                    sizeOverride = smaller
                    setStatus("Tamaño no admitido, probando ${sizeLabel(smaller)}…")
                    handler.post { restartPreview() }
                } else {
                    setStatus("No se pudo configurar la cámara")
                }
            }
        }, handler)
    }

    /** Guarda ISO y exposición reales de la vista previa para anotarlos en el paquete. */
    private val metaCallback = object : CameraCaptureSession.CaptureCallback() {
        override fun onCaptureCompleted(s: CameraCaptureSession, r: CaptureRequest, result: TotalCaptureResult) {
            result.get(CaptureResult.SENSOR_SENSITIVITY)?.let { lastIso = it }
            result.get(CaptureResult.SENSOR_EXPOSURE_TIME)?.let { lastExposureNs = it }
            if (waitingAf) {
                val st = result.get(CaptureResult.CONTROL_AF_STATE)
                if (st == null || st == CaptureResult.CONTROL_AF_STATE_FOCUSED_LOCKED || st == CaptureResult.CONTROL_AF_STATE_NOT_FOCUSED_LOCKED) {
                    fire()
                }
            }
        }
    }

    // =====================================================================================
    //  Ráfaga
    // =====================================================================================

    private fun shoot() {
        if (busy) return
        val s = session ?: return
        val b = previewBuilder ?: return
        busy = true
        fired = false
        setBusyUi(true)
        if (!afSupported) { setStatus("Capturando…"); handler.post { fire() }; return }
        setStatus("Enfocando…")
        try {
            waitingAf = true
            b.set(CaptureRequest.CONTROL_AF_MODE, CameraMetadata.CONTROL_AF_MODE_AUTO)
            b.set(CaptureRequest.CONTROL_AF_TRIGGER, CameraMetadata.CONTROL_AF_TRIGGER_START)
            s.capture(b.build(), metaCallback, handler)
            b.set(CaptureRequest.CONTROL_AF_TRIGGER, CameraMetadata.CONTROL_AF_TRIGGER_IDLE)
            s.setRepeatingRequest(b.build(), metaCallback, handler)
            handler.postDelayed({ if (waitingAf) fire() }, 2000)
        } catch (e: Exception) {
            failBurst("Error al enfocar: ${e.message}")
        }
    }

    @Synchronized
    private fun fire() {
        if (fired) return
        fired = true
        waitingAf = false
        val dev = device ?: return failBurst("Cámara cerrada")
        val s = session ?: return failBurst("Sin sesión de cámara")
        val rd = reader ?: return failBurst("Sin lector de imágenes")
        val n = count
        val w = try {
            BurstWriter(contentResolver, n, sensorOrientation)
        } catch (e: Exception) {
            return failBurst("No se pudo crear el archivo: ${e.message}")
        }
        writer = w
        framesWritten = 0
        tFirst = 0L
        tLast = 0L
        rd.setOnImageAvailableListener({ r -> onImage(r, n) }, handler)
        try {
            val req = dev.createCaptureRequest(CameraDevice.TEMPLATE_STILL_CAPTURE)
            req.addTarget(rd.surface)
            req.set(CaptureRequest.CONTROL_AE_LOCK, true)
            req.set(CaptureRequest.CONTROL_AWB_LOCK, true)
            if (afSupported) req.set(CaptureRequest.CONTROL_AF_MODE, CameraMetadata.CONTROL_AF_MODE_AUTO)
            // "Apagado": el filtro del teléfono borra detalle antes de que el motor lo vea.
            // Se prefiere OFF; si no existe, MINIMAL; y como último recurso FAST.
            val nrMode = when {
                !nrMinimal -> null
                noiseModes.contains(CameraMetadata.NOISE_REDUCTION_MODE_OFF) -> CameraMetadata.NOISE_REDUCTION_MODE_OFF
                noiseModes.contains(CameraMetadata.NOISE_REDUCTION_MODE_MINIMAL) -> CameraMetadata.NOISE_REDUCTION_MODE_MINIMAL
                noiseModes.contains(CameraMetadata.NOISE_REDUCTION_MODE_FAST) -> CameraMetadata.NOISE_REDUCTION_MODE_FAST
                else -> null
            }
            if (nrMode != null) req.set(CaptureRequest.NOISE_REDUCTION_MODE, nrMode)
            // Sin realce de bordes del teléfono (el motor aplica su propia nitidez controlada).
            if (nrMinimal && edgeModes.contains(CameraMetadata.EDGE_MODE_OFF)) req.set(CaptureRequest.EDGE_MODE, CameraMetadata.EDGE_MODE_OFF)
            setStatus("Capturando $n fotos… no muevas el celular")
            s.stopRepeating()
            s.captureBurst(List(n) { req.build() }, null, handler)
            // Vigilante: si no llegan todas las fotos, se descarta.
            handler.postDelayed({ if (busy && writer === w) failBurst("Tiempo agotado: llegaron $framesWritten de $n fotos") }, 20_000)
        } catch (e: Exception) {
            failBurst("Error al capturar: ${e.message}")
        }
    }

    private fun onImage(r: ImageReader, n: Int) {
        val w = writer ?: return
        val img = r.acquireNextImage() ?: return
        try {
            w.writeFrame(img, lastIso, lastExposureNs)
        } catch (e: Exception) {
            img.close()
            return failBurst("Error guardando: ${e.message}")
        }
        img.close()
        val now = System.nanoTime()
        if (framesWritten == 0) tFirst = now
        tLast = now
        framesWritten++
        setStatus("Guardando foto $framesWritten de $n…")
        if (framesWritten >= n) finishBurst(w, n)
    }

    private fun finishBurst(w: BurstWriter, n: Int) {
        writer = null
        reader?.setOnImageAvailableListener(null, null)
        try {
            lastUri = w.finish()
        } catch (e: Exception) {
            return failBurst("No se pudo cerrar el archivo: ${e.message}")
        }
        val secs = (tLast - tFirst) / 1e9
        val fps = if (n > 1 && secs > 0) (n - 1) / secs else 0.0
        lastReport = "Ráfaga: $n fotos de ${sizeLabel(captureSize)} en %.2f s (%.1f fotos/s). ISO %d, exposición %.1f ms.".format(secs, fps, lastIso, lastExposureNs / 1e6)
        resumePreview()
        busy = false
        setBusyUi(false)
        setStatus("Guardado en Descargas/Camara/${w.displayName}\n$lastReport\nToca «Compartir último» para enviarlo a la PC.")
    }

    private fun failBurst(msg: String) {
        writer?.abort()
        writer = null
        reader?.setOnImageAvailableListener(null, null)
        waitingAf = false
        resumePreview()
        busy = false
        setBusyUi(false)
        setStatus(msg)
    }

    /** Vuelve a la vista previa continua tras una ráfaga. */
    private fun resumePreview() {
        val s = session ?: return
        val b = previewBuilder ?: return
        try {
            if (afSupported) {
                b.set(CaptureRequest.CONTROL_AF_TRIGGER, CameraMetadata.CONTROL_AF_TRIGGER_CANCEL)
                s.capture(b.build(), metaCallback, handler)
                b.set(CaptureRequest.CONTROL_AF_TRIGGER, CameraMetadata.CONTROL_AF_TRIGGER_IDLE)
                b.set(CaptureRequest.CONTROL_AF_MODE, CameraMetadata.CONTROL_AF_MODE_CONTINUOUS_PICTURE)
            }
            s.setRepeatingRequest(b.build(), metaCallback, handler)
        } catch (_: Exception) {
            restartPreview()
        }
    }

    // =====================================================================================
    //  Info y compartir
    // =====================================================================================

    private fun showInfo() {
        val report = CameraProbe.describe(this) + if (lastReport.isNotEmpty()) "\nÚltima medición:\n$lastReport\n" else ""
        val tv = TextView(this).apply {
            text = report
            typeface = Typeface.MONOSPACE
            textSize = 11f
            setPadding(dp(14), dp(10), dp(14), dp(10))
        }
        val sv = ScrollView(this).apply { addView(tv) }
        AlertDialog.Builder(this)
            .setTitle("Información de la cámara")
            .setView(sv)
            .setPositiveButton("Compartir informe") { _, _ ->
                val i = Intent(Intent.ACTION_SEND).apply {
                    type = "text/plain"
                    putExtra(Intent.EXTRA_SUBJECT, "Informe de cámara")
                    putExtra(Intent.EXTRA_TEXT, report)
                }
                startActivity(Intent.createChooser(i, "Enviar informe"))
            }
            .setNegativeButton("Cerrar", null)
            .show()
    }

    /** Último .camb de Descargas/Camara (también funciona tras reiniciar la app). */
    private fun findLatest(): Uri? {
        lastUri?.let { return it }
        val proj = arrayOf(MediaStore.Downloads._ID)
        val sel = "${MediaStore.Downloads.RELATIVE_PATH} LIKE ? AND ${MediaStore.Downloads.DISPLAY_NAME} LIKE ?"
        val args = arrayOf("%Download/Camara%", "%.camb%")
        contentResolver.query(MediaStore.Downloads.EXTERNAL_CONTENT_URI, proj, sel, args, "${MediaStore.Downloads.DATE_ADDED} DESC")?.use { c ->
            if (c.moveToFirst()) return android.content.ContentUris.withAppendedId(MediaStore.Downloads.EXTERNAL_CONTENT_URI, c.getLong(0))
        }
        return null
    }

    private fun shareLast() {
        val uri = findLatest()
        if (uri == null) {
            Toast.makeText(this, "Aún no hay ninguna ráfaga guardada", Toast.LENGTH_SHORT).show()
            return
        }
        val i = Intent(Intent.ACTION_SEND).apply {
            type = "application/octet-stream"
            putExtra(Intent.EXTRA_STREAM, uri)
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        }
        startActivity(Intent.createChooser(i, "Enviar paquete a la PC (Bluetooth, Nearby Share, etc.)"))
    }
}
