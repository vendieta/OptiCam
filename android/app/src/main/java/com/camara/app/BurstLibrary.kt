package com.camara.app

import android.content.ContentUris
import android.content.Context
import android.graphics.Bitmap
import android.graphics.Matrix
import android.net.Uri
import android.provider.MediaStore
import android.provider.MediaStore.Downloads as D
import java.io.BufferedInputStream
import java.io.EOFException
import java.io.InputStream
import java.nio.ByteBuffer
import java.nio.ByteOrder

data class BurstItem(val uri: Uri, val name: String, val dateMs: Long, val size: Long)

/** Datos de la primera foto del paquete (la normal, también en HDR). */
data class BurstInfo(val frames: Int, val width: Int, val height: Int, val iso: Int, val exposureNs: Long, val rotation: Int)

class Decoded(val bitmap: Bitmap, val info: BurstInfo)

/** Lista, miniaturas y borrado de los paquetes .camb guardados en Descargas/Camara. */
object BurstLibrary {

    fun list(ctx: Context): List<BurstItem> {
        val out = ArrayList<BurstItem>()
        val proj = arrayOf(D._ID, D.DISPLAY_NAME, D.DATE_ADDED, D.SIZE)
        val sel = "${D.RELATIVE_PATH} LIKE ? AND ${D.DISPLAY_NAME} LIKE ? AND ${D.IS_PENDING} = 0"
        val args = arrayOf("%Download/Camara%", "%.camb")
        try {
            ctx.contentResolver.query(D.EXTERNAL_CONTENT_URI, proj, sel, args, "${D.DATE_ADDED} DESC")?.use { c ->
                while (c.moveToNext()) {
                    out.add(BurstItem(ContentUris.withAppendedId(D.EXTERNAL_CONTENT_URI, c.getLong(0)), c.getString(1), c.getLong(2) * 1000L, c.getLong(3)))
                }
            }
        } catch (_: Exception) {
        }
        return out
    }

    fun delete(ctx: Context, item: BurstItem): Boolean =
        try { ctx.contentResolver.delete(item.uri, null, null) > 0 } catch (_: Exception) { false }

    /**
     * Decodifica la primera foto del paquete a un bitmap de como mucho `maxSide` píxeles por lado
     * (leyendo solo las filas necesarias, así una miniatura es rápida incluso con 45 MB de archivo).
     */
    fun decodeFirstFrame(ctx: Context, uri: Uri, maxSide: Int): Decoded? {
        return try {
            val raw = ctx.contentResolver.openInputStream(uri) ?: return null
            raw.use { decode(BufferedInputStream(it, 1 shl 16), maxSide) }
        } catch (_: Exception) {
            null
        }
    }

    private fun decode(inp: InputStream, maxSide: Int): Decoded? {
        val head = ByteArray(12)
        readFully(inp, head, 12)
        if (head[0] != 'C'.code.toByte() || head[1] != 'A'.code.toByte() || head[2] != 'M'.code.toByte() || head[3] != 'B'.code.toByte()) return null
        val frames = ByteBuffer.wrap(head).order(ByteOrder.LITTLE_ENDIAN).getInt(8)
        val fh = ByteArray(36)
        readFully(inp, fh, 36)
        val b = ByteBuffer.wrap(fh).order(ByteOrder.LITTLE_ENDIAN)
        val w = b.getInt(0)
        val h = b.getInt(4)
        val iso = b.getInt(12)
        val exp = b.getLong(16)
        val rot = b.getInt(24)
        val cw = b.getInt(28)
        val ch = b.getInt(32)
        if (w !in 1..20000 || h !in 1..20000 || cw < 1 || ch < 1) return null

        val step = maxOf(1, (maxOf(w, h) + maxSide - 1) / maxSide)
        val ow = maxOf(1, w / step)
        val oh = maxOf(1, h / step)
        val yy = readPlane(inp, w, h, ow, oh, { it * step }, { it * step })
        val cb = readPlane(inp, cw, ch, ow, oh, { it * step / 2 }, { it * step / 2 })
        val cr = readPlane(inp, cw, ch, ow, oh, { it * step / 2 }, { it * step / 2 })

        val px = IntArray(ow * oh)
        for (i in px.indices) {
            val y = yy[i].toInt() and 0xFF
            val u = (cb[i].toInt() and 0xFF) - 128
            val v = (cr[i].toInt() and 0xFF) - 128
            val r = (y + 1.402f * v).toInt().coerceIn(0, 255)
            val g = (y - 0.344136f * u - 0.714136f * v).toInt().coerceIn(0, 255)
            val bl = (y + 1.772f * u).toInt().coerceIn(0, 255)
            px[i] = (0xFF shl 24) or (r shl 16) or (g shl 8) or bl
        }
        var bmp = Bitmap.createBitmap(px, ow, oh, Bitmap.Config.ARGB_8888)
        if (rot % 360 != 0) {
            val m = Matrix().apply { postRotate(rot.toFloat()) }
            bmp = Bitmap.createBitmap(bmp, 0, 0, ow, oh, m, true)
        }
        return Decoded(bmp, BurstInfo(frames, w, h, iso, exp, rot))
    }

    /** Lee un plano entero del flujo, quedándose solo con las muestras (ox,oy) → (fx(ox), fy(oy)). */
    private fun readPlane(inp: InputStream, pw: Int, ph: Int, ow: Int, oh: Int, fx: (Int) -> Int, fy: (Int) -> Int): ByteArray {
        val out = ByteArray(ow * oh)
        val row = ByteArray(pw)
        var cur = -1
        for (oy in 0 until oh) {
            val ty = fy(oy).coerceIn(0, ph - 1)
            if (ty != cur) {
                skipFully(inp, (ty - cur - 1).toLong() * pw)
                readFully(inp, row, pw)
                cur = ty
            }
            for (ox in 0 until ow) out[oy * ow + ox] = row[fx(ox).coerceIn(0, pw - 1)]
        }
        skipFully(inp, (ph - 1 - cur).toLong() * pw)
        return out
    }

    private fun readFully(i: InputStream, b: ByteArray, len: Int) {
        var o = 0
        while (o < len) {
            val n = i.read(b, o, len - o)
            if (n < 0) throw EOFException()
            o += n
        }
    }

    private fun skipFully(i: InputStream, n0: Long) {
        var n = n0
        while (n > 0) {
            val s = i.skip(n)
            if (s > 0) n -= s else {
                if (i.read() < 0) throw EOFException()
                n--
            }
        }
    }
}
