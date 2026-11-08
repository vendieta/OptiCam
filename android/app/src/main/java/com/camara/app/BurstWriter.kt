package com.camara.app

import android.content.ContentResolver
import android.content.ContentValues
import android.media.Image
import android.net.Uri
import android.os.Environment
import android.provider.MediaStore
import java.io.BufferedOutputStream
import java.io.OutputStream
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/**
 * Escribe un paquete de ráfaga .camb (formato v2) directamente en Descargas/Camara,
 * foto por foto, para no acumular memoria.
 *
 * Formato (little-endian):
 *   "CAMB" | u32 versión=2 | u32 nº de fotos
 *   por foto: u32 ancho | u32 alto | u32 blanco | u32 ISO | u64 exposición(ns)
 *             | u32 rotación | u32 cw | u32 ch | Y (u8) | Cb (u8) | Cr (u8)
 */
class BurstWriter(
    private val resolver: ContentResolver,
    private val frameCount: Int,
    private val rotation: Int,
) {
    val uri: Uri
    val displayName: String
    private val out: OutputStream

    init {
        val stamp = SimpleDateFormat("yyyyMMdd_HHmmss", Locale.US).format(Date())
        displayName = "rafaga_$stamp.camb"
        val values = ContentValues().apply {
            put(MediaStore.Downloads.DISPLAY_NAME, displayName)
            put(MediaStore.Downloads.MIME_TYPE, "application/octet-stream")
            put(MediaStore.Downloads.RELATIVE_PATH, Environment.DIRECTORY_DOWNLOADS + "/Camara")
            put(MediaStore.Downloads.IS_PENDING, 1)
        }
        uri = resolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, values)
            ?: throw IllegalStateException("No se pudo crear el archivo en Descargas")
        out = BufferedOutputStream(
            resolver.openOutputStream(uri) ?: throw IllegalStateException("No se pudo abrir el archivo"),
            1 shl 20,
        )
        val head = ByteBuffer.allocate(12).order(ByteOrder.LITTLE_ENDIAN)
        head.put('C'.code.toByte()).put('A'.code.toByte()).put('M'.code.toByte()).put('B'.code.toByte())
        head.putInt(2).putInt(frameCount)
        out.write(head.array())
    }

    fun writeFrame(img: Image, iso: Int, exposureNs: Long) {
        val w = img.width
        val h = img.height
        val cw = (w + 1) / 2
        val ch = (h + 1) / 2
        val hdr = ByteBuffer.allocate(36).order(ByteOrder.LITTLE_ENDIAN)
        hdr.putInt(w).putInt(h).putInt(255).putInt(iso).putLong(exposureNs)
        hdr.putInt(rotation).putInt(cw).putInt(ch)
        out.write(hdr.array())
        writePlane(img.planes[0], w, h)
        writePlane(img.planes[1], cw, ch)
        writePlane(img.planes[2], cw, ch)
    }

    private fun writePlane(p: Image.Plane, pw: Int, ph: Int) {
        val buf = p.buffer
        val rowStride = p.rowStride
        val pixelStride = p.pixelStride
        val row = ByteArray(pw)
        val tmp = if (pixelStride == 1) ByteArray(0) else ByteArray(rowStride)
        for (y in 0 until ph) {
            val start = y * rowStride
            if (pixelStride == 1) {
                buf.position(start)
                buf.get(row, 0, pw)
            } else {
                val len = minOf(rowStride, buf.limit() - start)
                buf.position(start)
                buf.get(tmp, 0, len)
                for (x in 0 until pw) row[x] = tmp[x * pixelStride]
            }
            out.write(row, 0, pw)
        }
    }

    fun finish(): Uri {
        out.flush()
        out.close()
        val values = ContentValues().apply { put(MediaStore.Downloads.IS_PENDING, 0) }
        resolver.update(uri, values, null, null)
        return uri
    }

    fun abort() {
        try { out.close() } catch (_: Exception) {}
        try { resolver.delete(uri, null, null) } catch (_: Exception) {}
    }
}
