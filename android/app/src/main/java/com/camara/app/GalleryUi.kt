package com.camara.app

import android.app.Activity
import android.app.AlertDialog
import android.graphics.Bitmap
import android.graphics.Color
import android.graphics.Typeface
import android.graphics.drawable.ColorDrawable
import android.os.Handler
import android.os.Looper
import android.util.LruCache
import android.view.Gravity
import android.view.View
import android.view.ViewGroup
import android.widget.BaseAdapter
import android.widget.FrameLayout
import android.widget.GridView
import android.widget.ImageView
import android.widget.LinearLayout
import android.widget.ProgressBar
import android.widget.TextView
import android.widget.Toast
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import java.util.concurrent.Executors
import kotlin.math.roundToInt

/**
 * Pantallas superpuestas a la cámara: Historial (cuadrícula de ráfagas) y Vista previa de una ráfaga.
 * Se construyen en código, como el resto de la interfaz.
 */
class GalleryUi(
    private val act: Activity,
    private val onShare: (BurstItem) -> Unit,
    private val onChanged: () -> Unit,
) {
    val root = FrameLayout(act).apply { visibility = View.GONE }

    private val ui = Handler(Looper.getMainLooper())
    private val pool = Executors.newFixedThreadPool(2)
    private val cache = LruCache<String, Decoded>(80)
    private val items = ArrayList<BurstItem>()
    private val dateFmt = SimpleDateFormat("d MMM yyyy · HH:mm", Locale("es"))

    var lastItem: BurstItem? = null
        private set

    // historial
    private lateinit var history: LinearLayout
    private lateinit var subtitle: TextView
    private lateinit var empty: TextView
    private lateinit var grid: GridView

    // vista previa
    private lateinit var viewer: FrameLayout
    private lateinit var image: ImageView
    private lateinit var spinner: ProgressBar
    private lateinit var vTitle: TextView
    private lateinit var vInfo: TextView
    private lateinit var topBar: View
    private lateinit var card: View
    private var viewerItem: BurstItem? = null
    private var viewerFromHistory = false

    private fun dp(v: Int) = Ui.dp(act, v)
    private fun mb(b: Long) = "%.1f MB".format(b / 1048576.0)
    private fun stamp(ms: Long) = dateFmt.format(Date(ms))

    init {
        buildHistory()
        buildViewer()
        root.addView(history, ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT)
        root.addView(viewer, ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT)
    }

    // =====================================================================================
    //  API
    // =====================================================================================

    val isOpen get() = root.visibility == View.VISIBLE

    /** Atrás: cierra la vista previa o el historial. Devuelve false si no había nada abierto. */
    fun onBack(): Boolean {
        if (!isOpen) return false
        if (viewer.visibility == View.VISIBLE) {
            viewer.visibility = View.GONE
            viewerItem = null
            if (!viewerFromHistory) root.visibility = View.GONE
        } else {
            root.visibility = View.GONE
        }
        return true
    }

    fun showHistory() {
        root.visibility = View.VISIBLE
        history.visibility = View.VISIBLE
        viewer.visibility = View.GONE
        reloadList()
    }

    fun showViewer(item: BurstItem, fromHistory: Boolean) {
        viewerFromHistory = fromHistory
        root.visibility = View.VISIBLE
        history.visibility = if (fromHistory) View.VISIBLE else View.GONE
        viewer.visibility = View.VISIBLE
        topBar.visibility = View.VISIBLE
        card.visibility = View.VISIBLE
        viewerItem = item
        image.setImageBitmap(null)
        spinner.visibility = View.VISIBLE
        vTitle.text = stamp(item.dateMs)
        vInfo.text = mb(item.size)
        pool.execute {
            val d = BurstLibrary.decodeFirstFrame(act, item.uri, 1600)
            ui.post {
                if (viewerItem !== item) return@post
                spinner.visibility = View.GONE
                if (d == null) {
                    vInfo.text = "No se pudo leer este paquete."
                    return@post
                }
                image.setImageBitmap(d.bitmap)
                val i = d.info
                vInfo.text = "${i.frames} fotos · ${i.width}×${i.height} · ${mb(item.size)}\nISO ${i.iso} · ${exposureLabel(i.exposureNs)}"
            }
        }
    }

    /** Carga el resumen para la pantalla de la cámara: última ráfaga, cuántas hay y su miniatura. */
    fun loadSummary(cb: (BurstItem?, Int, Bitmap?) -> Unit) {
        pool.execute {
            val l = BurstLibrary.list(act)
            val first = l.firstOrNull()
            val bmp = first?.let { decodeCached(it, THUMB)?.bitmap }
            ui.post {
                lastItem = first
                cb(first, l.size, bmp)
            }
        }
    }

    // =====================================================================================
    //  Historial
    // =====================================================================================

    private fun reloadList() {
        subtitle.text = "Cargando…"
        pool.execute {
            val l = BurstLibrary.list(act)
            ui.post {
                items.clear()
                items.addAll(l)
                lastItem = l.firstOrNull()
                (grid.adapter as BaseAdapter).notifyDataSetChanged()
                empty.visibility = if (l.isEmpty()) View.VISIBLE else View.GONE
                subtitle.text = if (l.isEmpty()) "Sin ráfagas" else "${l.size} ráfagas · ${mb(l.sumOf { it.size })}"
            }
        }
    }

    private fun iconButton(glyph: String, onClick: () -> Unit) = TextView(act).apply {
        text = glyph
        textSize = 20f
        setTextColor(Color.WHITE)
        gravity = Gravity.CENTER
        background = Ui.pressableOval(act, Ui.SURFACE2)
        setOnClickListener { onClick() }
    }

    private class Holder(val img: ImageView, val title: TextView, val sub: TextView)

    private fun buildHistory() {
        history = LinearLayout(act).apply {
            orientation = LinearLayout.VERTICAL
            setBackgroundColor(Ui.BG)
            isClickable = true
        }
        val bar = LinearLayout(act).apply {
            gravity = Gravity.CENTER_VERTICAL
            setPadding(dp(12), dp(10), dp(16), dp(10))
        }
        val titles = LinearLayout(act).apply { orientation = LinearLayout.VERTICAL }
        titles.addView(TextView(act).apply {
            text = "Historial"
            textSize = 22f
            typeface = Typeface.DEFAULT_BOLD
            setTextColor(Color.WHITE)
        })
        subtitle = TextView(act).apply { textSize = 12f; setTextColor(Ui.MUTED) }
        titles.addView(subtitle)
        bar.addView(iconButton("←") { onBack() }, LinearLayout.LayoutParams(dp(42), dp(42)))
        bar.addView(titles, LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f).apply { leftMargin = dp(12) })
        history.addView(bar)

        val gap = dp(10)
        val cellW = (act.resources.displayMetrics.widthPixels - dp(24) - gap) / 2
        grid = GridView(act).apply {
            numColumns = 2
            horizontalSpacing = gap
            verticalSpacing = gap
            stretchMode = GridView.STRETCH_COLUMN_WIDTH
            setPadding(dp(12), dp(4), dp(12), dp(16))
            clipToPadding = false
            selector = ColorDrawable(Color.TRANSPARENT)
            adapter = object : BaseAdapter() {
                override fun getCount() = items.size
                override fun getItem(p: Int) = items[p]
                override fun getItemId(p: Int) = items[p].uri.hashCode().toLong()
                override fun getView(p: Int, convert: View?, parent: ViewGroup?): View {
                    val h: Holder
                    val v: View
                    if (convert == null) {
                        val img = ImageView(act).apply { scaleType = ImageView.ScaleType.CENTER_CROP; setBackgroundColor(Ui.SURFACE2) }
                        val t = TextView(act).apply { textSize = 13f; typeface = Typeface.DEFAULT_BOLD; setTextColor(Color.WHITE); maxLines = 1 }
                        val s = TextView(act).apply { textSize = 11f; setTextColor(Ui.MUTED); maxLines = 1 }
                        val box = LinearLayout(act).apply {
                            orientation = LinearLayout.VERTICAL
                            background = Ui.shape(act, Ui.SURFACE, 16)
                            clipToOutline = true
                            addView(img, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, cellW))
                            val txt = LinearLayout(act).apply { orientation = LinearLayout.VERTICAL; setPadding(dp(10), dp(8), dp(10), dp(10)) }
                            txt.addView(t); txt.addView(s)
                            addView(txt)
                        }
                        h = Holder(img, t, s)
                        box.tag = h
                        v = box
                    } else {
                        v = convert
                        h = v.tag as Holder
                    }
                    val item = items[p]
                    val key = item.uri.toString()
                    h.title.text = stamp(item.dateMs)
                    h.sub.text = mb(item.size)
                    h.img.setImageBitmap(null)
                    h.img.tag = key
                    loadAsync(item, THUMB) { d ->
                        if (d != null && h.img.tag == key) {
                            h.img.setImageBitmap(d.bitmap)
                            h.sub.text = "${d.info.frames} fotos · ${d.info.width}×${d.info.height} · ${mb(item.size)}"
                        }
                    }
                    return v
                }
            }
            setOnItemClickListener { _, _, p, _ -> showViewer(items[p], true) }
            setOnItemLongClickListener { _, _, p, _ ->
                val item = items[p]
                dialog().setTitle(stamp(item.dateMs))
                    .setItems(arrayOf("Compartir", "Eliminar")) { _, which ->
                        if (which == 0) onShare(item) else confirmDelete(item)
                    }.show()
                true
            }
        }
        empty = TextView(act).apply {
            text = "Aún no hay ráfagas.\nToma una y aparecerá aquí."
            textSize = 15f
            setTextColor(Ui.MUTED)
            gravity = Gravity.CENTER
            visibility = View.GONE
        }
        val hint = TextView(act).apply {
            text = "Toca para ver · mantén pulsado para compartir o eliminar"
            textSize = 11f
            setTextColor(Ui.MUTED)
            gravity = Gravity.CENTER
            setPadding(dp(12), dp(6), dp(12), dp(10))
        }
        val body = FrameLayout(act)
        body.addView(grid, FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT))
        body.addView(empty, FrameLayout.LayoutParams(ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT, Gravity.CENTER))
        history.addView(body, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 0, 1f))
        history.addView(hint)
    }

    // =====================================================================================
    //  Vista previa
    // =====================================================================================

    private fun buildViewer() {
        viewer = FrameLayout(act).apply {
            setBackgroundColor(Color.BLACK)
            isClickable = true
            visibility = View.GONE
        }
        image = ImageView(act).apply {
            scaleType = ImageView.ScaleType.FIT_CENTER
            setOnClickListener {
                // Toque sobre la foto: oculta o muestra los controles.
                val show = card.visibility != View.VISIBLE
                card.visibility = if (show) View.VISIBLE else View.GONE
                topBar.visibility = if (show) View.VISIBLE else View.GONE
            }
        }
        viewer.addView(image, FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT))
        spinner = ProgressBar(act)
        viewer.addView(spinner, FrameLayout.LayoutParams(dp(40), dp(40), Gravity.CENTER))

        val bar = LinearLayout(act).apply {
            gravity = Gravity.CENTER_VERTICAL
            setBackgroundColor(Ui.SCRIM)
            setPadding(dp(12), dp(8), dp(16), dp(8))
        }
        vTitle = TextView(act).apply { textSize = 15f; typeface = Typeface.DEFAULT_BOLD; setTextColor(Color.WHITE) }
        bar.addView(iconButton("←") { onBack() }, LinearLayout.LayoutParams(dp(42), dp(42)))
        bar.addView(vTitle, LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f).apply { leftMargin = dp(12) })
        topBar = bar
        viewer.addView(bar, FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT, Gravity.TOP))

        vInfo = TextView(act).apply { textSize = 13f; setTextColor(Color.WHITE); setLineSpacing(0f, 1.2f) }
        val tip = TextView(act).apply {
            text = "Vista de la primera foto de la ráfaga. La foto fusionada y mejorada se genera en la PC."
            textSize = 11f
            setTextColor(Ui.MUTED)
            setPadding(0, dp(6), 0, dp(12))
        }
        val share = TextView(act).apply {
            text = "Compartir"
            textSize = 14f
            typeface = Typeface.DEFAULT_BOLD
            setTextColor(Color.BLACK)
            gravity = Gravity.CENTER
            background = Ui.pressable(act, Ui.ACCENT, 24)
            setOnClickListener { viewerItem?.let(onShare) }
        }
        val del = TextView(act).apply {
            text = "Eliminar"
            textSize = 14f
            typeface = Typeface.DEFAULT_BOLD
            setTextColor(Ui.DANGER)
            gravity = Gravity.CENTER
            background = Ui.shape(act, Color.TRANSPARENT, 24, Ui.DANGER, 1)
            setOnClickListener { viewerItem?.let { confirmDelete(it) } }
        }
        val row = LinearLayout(act)
        row.addView(share, LinearLayout.LayoutParams(0, dp(48), 1f).apply { rightMargin = dp(6) })
        row.addView(del, LinearLayout.LayoutParams(0, dp(48), 1f).apply { leftMargin = dp(6) })
        val c = LinearLayout(act).apply {
            orientation = LinearLayout.VERTICAL
            background = Ui.topRounded(act, Ui.PANEL, 24)
            setPadding(dp(20), dp(16), dp(20), dp(16))
            addView(vInfo)
            addView(tip)
            addView(row)
        }
        card = c
        viewer.addView(c, FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT, Gravity.BOTTOM))
    }

    private fun exposureLabel(ns: Long): String {
        if (ns <= 0) return "—"
        val s = ns / 1e9
        return if (s >= 0.5) "%.1f s".format(s) else "1/${(1.0 / s).roundToInt()} s"
    }

    // =====================================================================================
    //  Utilidades
    // =====================================================================================

    private fun dialog() = AlertDialog.Builder(act, android.R.style.Theme_Material_Dialog_Alert)

    private fun confirmDelete(item: BurstItem) {
        dialog()
            .setTitle("¿Eliminar esta ráfaga?")
            .setMessage("${stamp(item.dateMs)} · ${mb(item.size)}\nSe borrará el archivo .camb del teléfono y no se puede deshacer.")
            .setPositiveButton("Eliminar") { _, _ ->
                if (BurstLibrary.delete(act, item)) {
                    cache.remove("${item.uri}#$THUMB")
                    Toast.makeText(act, "Ráfaga eliminada", Toast.LENGTH_SHORT).show()
                    if (viewer.visibility == View.VISIBLE) onBack()
                    if (viewerFromHistory || history.visibility == View.VISIBLE) reloadList()
                    onChanged()
                } else {
                    Toast.makeText(act, "No se pudo eliminar (bórrala desde Archivos > Descargas > Camara)", Toast.LENGTH_LONG).show()
                }
            }
            .setNegativeButton("Cancelar", null)
            .show()
    }

    private fun decodeCached(item: BurstItem, side: Int): Decoded? {
        val key = "${item.uri}#$side"
        cache.get(key)?.let { return it }
        val d = BurstLibrary.decodeFirstFrame(act, item.uri, side) ?: return null
        cache.put(key, d)
        return d
    }

    private fun loadAsync(item: BurstItem, side: Int, cb: (Decoded?) -> Unit) {
        cache.get("${item.uri}#$side")?.let { cb(it); return }
        pool.execute {
            val d = decodeCached(item, side)
            ui.post { cb(d) }
        }
    }

    companion object {
        private const val THUMB = 320
    }
}
