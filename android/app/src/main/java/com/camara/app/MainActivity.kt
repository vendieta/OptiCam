package com.camara.app

import android.Manifest
import android.app.Activity
import android.app.AlertDialog
import android.content.Intent
import android.content.pm.PackageManager
import android.content.res.ColorStateList
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
import android.widget.FrameLayout
import android.widget.ImageView
import android.widget.LinearLayout
import android.widget.ProgressBar
import android.widget.ScrollView
import android.widget.TextView
import android.widget.Toast
import kotlin.math.abs

class MainActivity : Activity() {

    // ---- interfaz ----
    private lateinit var texture: AutoFitTextureView
    private lateinit var statusView: TextView
    private lateinit var btnShoot: FrameLayout
    private lateinit var shutterInner: View
    private lateinit var btnInfo: TextView
    private lateinit var btnHistory: LinearLayout
    private lateinit var historyCount: TextView
    private lateinit var thumbView: ImageView
    private lateinit var progress: ProgressBar
    private lateinit var gallery: GalleryUi
    private lateinit var chipMode: Chip
    private lateinit var chipCount: Chip
    private lateinit var chipRes: Chip
    private lateinit var chipNr: Chip

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

    // ---- modos de captura ----
    private val modeNames = listOf("Normal", "HDR", "Noche")
    private var mode = 0
    private var expRange: android.util.Range<Long>? = null
    private var isoRange: android.util.Range<Int>? = null
    private var manualOk = false
    private var framePlan: List<Pair<Int, Long>> = emptyList()

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

    private fun dp(v: Int) = Ui.dp(this, v)

    private class Chip(val root: LinearLayout, val label: TextView, val value: TextView)

    private fun chip(): Chip {
        val label = TextView(this).apply { textSize = 10f; setTextColor(Ui.MUTED); gravity = Gravity.CENTER; maxLines = 1 }
        val value = TextView(this).apply { textSize = 13f; setTextColor(Color.WHITE); typeface = Typeface.DEFAULT_BOLD; gravity = Gravity.CENTER; maxLines = 1 }
        val box = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            gravity = Gravity.CENTER
            setPadding(dp(4), dp(8), dp(4), dp(8))
            background = Ui.pressable(this@MainActivity, Ui.SURFACE, 14)
            isClickable = true
            addView(label)
            addView(value)
        }
        return Chip(box, label, value)
    }

    private fun buildUi() {
        val root = FrameLayout(this).apply { setBackgroundColor(Ui.BG) }

        texture = AutoFitTextureView(this)
        root.addView(texture, FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT, Gravity.TOP))

        // Barra superior flotante: estado + información de la cámara.
        statusView = TextView(this).apply {
            setTextColor(Color.WHITE)
            background = Ui.shape(this@MainActivity, Ui.SCRIM, 16)
            textSize = 12f
            setPadding(dp(12), dp(8), dp(12), dp(8))
            text = "Iniciando cámara…"
        }
        btnInfo = TextView(this).apply {
            text = "i"
            textSize = 17f
            typeface = Typeface.create(Typeface.SERIF, Typeface.BOLD_ITALIC)
            setTextColor(Color.WHITE)
            gravity = Gravity.CENTER
            background = Ui.pressableOval(this@MainActivity, Ui.SCRIM)
        }
        val top = LinearLayout(this).apply { gravity = Gravity.TOP; setPadding(dp(10), dp(10), dp(10), 0) }
        top.addView(statusView, LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f).apply { rightMargin = dp(8) })
        top.addView(btnInfo, LinearLayout.LayoutParams(dp(40), dp(40)))
        root.addView(top, FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT, Gravity.TOP))

        // Panel inferior: progreso, opciones y fila del obturador.
        val panel = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            background = Ui.topRounded(this@MainActivity, Ui.PANEL, 24)
            setPadding(dp(12), dp(14), dp(12), dp(14))
        }
        progress = ProgressBar(this, null, android.R.attr.progressBarStyleHorizontal).apply {
            max = 100
            progressTintList = ColorStateList.valueOf(Ui.ACCENT)
            progressBackgroundTintList = ColorStateList.valueOf(Ui.SURFACE2)
            visibility = View.INVISIBLE
        }
        panel.addView(progress, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, dp(3)).apply { bottomMargin = dp(10) })

        val opts = LinearLayout(this).apply { orientation = LinearLayout.HORIZONTAL }
        chipMode = chip(); chipCount = chip(); chipRes = chip(); chipNr = chip()
        for (c in listOf(chipMode, chipCount, chipRes, chipNr)) {
            opts.addView(c.root, LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f).apply { setMargins(dp(3), 0, dp(3), 0) })
        }
        panel.addView(opts, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT).apply { bottomMargin = dp(14) })

        // Vista previa de la última ráfaga (izquierda).
        thumbView = ImageView(this).apply {
            scaleType = ImageView.ScaleType.CENTER_CROP
            background = Ui.shape(this@MainActivity, Ui.SURFACE2, 14, Color.WHITE, 2)
            clipToOutline = true
            isClickable = true
        }
        // Obturador (centro).
        shutterInner = View(this).apply { background = Ui.oval(this@MainActivity, Color.WHITE) }
        val ring = View(this).apply { background = Ui.oval(this@MainActivity, Color.TRANSPARENT, Color.WHITE, 4) }
        btnShoot = FrameLayout(this).apply {
            isClickable = true
            addView(ring, FrameLayout.LayoutParams(dp(78), dp(78), Gravity.CENTER))
            addView(shutterInner, FrameLayout.LayoutParams(dp(62), dp(62), Gravity.CENTER))
        }
        // Historial (derecha).
        historyCount = TextView(this).apply { textSize = 16f; typeface = Typeface.DEFAULT_BOLD; setTextColor(Color.WHITE); gravity = Gravity.CENTER; text = "▦" }
        val historyLabel = TextView(this).apply { text = "Historial"; textSize = 10f; setTextColor(Ui.MUTED); gravity = Gravity.CENTER }
        btnHistory = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            gravity = Gravity.CENTER
            background = Ui.pressable(this@MainActivity, Ui.SURFACE, 14)
            isClickable = true
            addView(historyCount)
            addView(historyLabel)
        }
        val main = LinearLayout(this).apply { orientation = LinearLayout.HORIZONTAL; gravity = Gravity.CENTER_VERTICAL }
        main.addView(thumbView, LinearLayout.LayoutParams(dp(62), dp(62)))
        main.addView(View(this), LinearLayout.LayoutParams(0, 1, 1f))
        main.addView(btnShoot, LinearLayout.LayoutParams(dp(80), dp(80)))
        main.addView(View(this), LinearLayout.LayoutParams(0, 1, 1f))
        main.addView(btnHistory, LinearLayout.LayoutParams(dp(62), dp(62)))
        panel.addView(main)
        root.addView(panel, FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT, Gravity.BOTTOM))

        // Historial y vista previa (superpuestos).
        gallery = GalleryUi(this, { shareUri(it.uri) }, { refreshSummary() })
        root.addView(gallery.root, FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT))

        root.setOnApplyWindowInsetsListener { v, insets ->
            @Suppress("DEPRECATION")
            v.setPadding(insets.systemWindowInsetLeft, insets.systemWindowInsetTop, insets.systemWindowInsetRight, insets.systemWindowInsetBottom)
            insets
        }
        setContentView(root)

        chipMode.root.setOnClickListener {
            mode = (mode + 1) % modeNames.size
            refreshLabels()
            setStatus(when (mode) {
                1 -> if (manualOk) "HDR: toma fotos con exposición corta, normal y larga para recuperar luces y sombras. Mantén el celular quieto."
                     else "Este teléfono no permite exposición manual: HDR se hará como Normal."
                2 -> if (manualOk) "Noche: exposición más larga y ISO más bajo (menos ruido). Apoya el celular o usa un trípode."
                     else "Este teléfono no permite exposición manual: Noche se hará como Normal."
                else -> "Normal: exposición automática bloqueada."
            })
        }
        chipCount.root.setOnClickListener { countIdx = (countIdx + 1) % counts.size; refreshLabels(); restartPreview() }
        chipRes.root.setOnClickListener { resLevel = (resLevel + 1) % 3; sizeOverride = null; refreshLabels(); restartPreview() }
        chipNr.root.setOnClickListener { nrMinimal = !nrMinimal; refreshLabels() }
        btnShoot.setOnClickListener { shoot() }
        btnInfo.setOnClickListener { showInfo() }
        btnHistory.setOnClickListener { gallery.showHistory() }
        thumbView.setOnClickListener {
            val last = gallery.lastItem
            if (last == null) Toast.makeText(this, "Aún no hay ninguna ráfaga guardada", Toast.LENGTH_SHORT).show()
            else gallery.showViewer(last, false)
        }
        refreshLabels()
        refreshSummary()

        texture.surfaceTextureListener = object : TextureView.SurfaceTextureListener {
            override fun onSurfaceTextureAvailable(st: SurfaceTexture, w: Int, h: Int) {
                if (checkSelfPermission(Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED) openCamera()
            }
            override fun onSurfaceTextureSizeChanged(st: SurfaceTexture, w: Int, h: Int) {}
            override fun onSurfaceTextureDestroyed(st: SurfaceTexture): Boolean = true
            override fun onSurfaceTextureUpdated(st: SurfaceTexture) {}
        }
    }

    /** Miniatura de la última ráfaga y contador del historial. */
    private fun refreshSummary() = gallery.loadSummary { _, n, bmp ->
        thumbView.setImageBitmap(bmp)
        historyCount.text = if (n > 0) "▦ $n" else "▦"
    }

    @Suppress("OVERRIDE_DEPRECATION", "DEPRECATION")
    override fun onBackPressed() {
        if (!gallery.onBack()) super.onBackPressed()
    }

    private fun refreshLabels() {
        chipMode.label.text = "Modo"; chipMode.value.text = modeNames[mode]
        chipCount.label.text = "Fotos"; chipCount.value.text = "$count"
        chipRes.label.text = "Calidad"; chipRes.value.text = resNames[resLevel]
        chipNr.label.text = "Ruido cám."; chipNr.value.text = if (nrMinimal) "Apagado" else "Normal"
    }

    private fun sizeLabel(s: Size) = "${s.width}×${s.height}"

    private fun setStatus(msg: String) = runOnUiThread { statusView.text = msg }

    private fun setBusyUi(b: Boolean) = runOnUiThread {
        for (v in listOf<View>(chipCount.root, chipRes.root, chipNr.root, chipMode.root, btnInfo, btnHistory, thumbView)) v.isEnabled = !b
        btnShoot.isEnabled = !b
        btnShoot.alpha = if (b) 0.6f else 1f
        shutterInner.background = Ui.oval(this, if (b) Ui.DANGER else Color.WHITE)
        progress.progress = 0
        progress.visibility = if (b) View.VISIBLE else View.INVISIBLE
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
        expRange = chars.get(CameraCharacteristics.SENSOR_INFO_EXPOSURE_TIME_RANGE)
        isoRange = chars.get(CameraCharacteristics.SENSOR_INFO_SENSITIVITY_RANGE)
        manualOk = chars.get(CameraCharacteristics.REQUEST_AVAILABLE_CAPABILITIES)
            ?.contains(CameraMetadata.REQUEST_AVAILABLE_CAPABILITIES_MANUAL_SENSOR) == true && expRange != null && isoRange != null
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

    /**
     * Plan de exposición de cada foto de la ráfaga: (ISO, tiempo en ns).
     * Devuelve también si hay que usar exposición manual.
     *  - Normal: la exposición automática ya calculada, bloqueada.
     *  - HDR: tres exposiciones intercaladas (base, corta = -2 EV, larga = hasta 2x), mismo ISO.
     *  - Noche: exposición más larga (hasta 3x) con ISO más bajo, para el mismo brillo con menos ruido.
     */
    private fun buildPlan(n: Int): Pair<List<Pair<Int, Long>>, Boolean> {
        val t0 = lastExposureNs
        val iso0 = lastIso
        val er = expRange
        val ir = isoRange
        if (mode == 0 || !manualOk || er == null || ir == null) {
            return List(n) { iso0 to t0 } to false
        }
        fun clampT(t: Long) = t.coerceIn(er.lower, er.upper)
        fun clampI(i: Int) = i.coerceIn(ir.lower, ir.upper)
        return if (mode == 1) {
            val base = clampI(iso0) to clampT(t0)
            val short = clampI(iso0) to clampT(t0 / 4)
            val long = clampI(iso0) to clampT(minOf(t0 * 2, maxOf(t0, 100_000_000L)))
            List(n) { i -> when (i % 3) { 0 -> base; 1 -> short; else -> long } } to true
        } else {
            val t = clampT(minOf(t0 * 3, 150_000_000L))
            val iso = clampI((iso0.toLong() * t0 / maxOf(t, 1L)).toInt())
            List(n) { iso to t } to true
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
        val (plan, manual) = buildPlan(n)
        framePlan = plan
        try {
            val req = dev.createCaptureRequest(CameraDevice.TEMPLATE_STILL_CAPTURE)
            req.addTarget(rd.surface)
            if (!manual) req.set(CaptureRequest.CONTROL_AE_LOCK, true)
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
            setStatus("Capturando $n fotos (${modeNames[if (manual) mode else 0]})… no muevas el celular")
            s.stopRepeating()
            val requests = List(n) { i ->
                if (manual) {
                    req.set(CaptureRequest.CONTROL_AE_MODE, CameraMetadata.CONTROL_AE_MODE_OFF)
                    req.set(CaptureRequest.SENSOR_SENSITIVITY, plan[i].first)
                    req.set(CaptureRequest.SENSOR_EXPOSURE_TIME, plan[i].second)
                    req.set(CaptureRequest.SENSOR_FRAME_DURATION, maxOf(plan[i].second, 33_333_333L))
                }
                req.build()
            }
            s.captureBurst(requests, null, handler)
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
            // ISO y exposición REALES de esta foto (en HDR cambian de una a otra).
            val meta = framePlan.getOrNull(framesWritten)
            w.writeFrame(img, meta?.first ?: lastIso, meta?.second ?: lastExposureNs)
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
        val pct = framesWritten * 100 / n
        runOnUiThread { progress.progress = pct }
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
        lastReport = "Ráfaga: $n fotos de ${sizeLabel(captureSize)} en %.2f s (%.1f fotos/s). ISO %d, exposición %.1f ms. Modo %s: exposiciones [%s] ms.".format(
            secs, fps, lastIso, lastExposureNs / 1e6, modeNames[mode],
            framePlan.map { it.second / 1_000_000.0 }.distinct().joinToString(", ") { "%.1f".format(it) })
        resumePreview()
        busy = false
        setBusyUi(false)
        setStatus("Guardado ✓ ${w.displayName}\n$lastReport\nToca la miniatura para verla o ábrela en Historial.")
        runOnUiThread { refreshSummary() }
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

    private fun shareUri(uri: Uri) {
        val i = Intent(Intent.ACTION_SEND).apply {
            type = "application/octet-stream"
            putExtra(Intent.EXTRA_STREAM, uri)
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        }
        startActivity(Intent.createChooser(i, "Enviar paquete a la PC (Bluetooth, Nearby Share, etc.)"))
    }
}
