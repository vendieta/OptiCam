package com.camara.app

import android.content.Context
import android.graphics.ImageFormat
import android.hardware.camera2.CameraCharacteristics
import android.hardware.camera2.CameraManager
import android.hardware.camera2.CameraMetadata
import android.os.Build

/** Sonda de la cámara: dice qué permite realmente este teléfono (RAW, ráfaga, tamaños...). */
object CameraProbe {

    private val capNames = mapOf(
        0 to "BACKWARD_COMPATIBLE", 1 to "MANUAL_SENSOR", 2 to "MANUAL_POST_PROCESSING",
        3 to "RAW", 4 to "PRIVATE_REPROCESSING", 5 to "READ_SENSOR_SETTINGS", 6 to "BURST_CAPTURE",
        7 to "YUV_REPROCESSING", 8 to "DEPTH_OUTPUT", 9 to "CONSTRAINED_HIGH_SPEED_VIDEO",
        10 to "MOTION_TRACKING", 11 to "LOGICAL_MULTI_CAMERA", 12 to "MONOCHROME",
    )

    private fun level(v: Int?) = when (v) {
        CameraMetadata.INFO_SUPPORTED_HARDWARE_LEVEL_LEGACY -> "LEGACY (el más básico)"
        CameraMetadata.INFO_SUPPORTED_HARDWARE_LEVEL_LIMITED -> "LIMITED"
        CameraMetadata.INFO_SUPPORTED_HARDWARE_LEVEL_FULL -> "FULL"
        CameraMetadata.INFO_SUPPORTED_HARDWARE_LEVEL_3 -> "LEVEL_3"
        CameraMetadata.INFO_SUPPORTED_HARDWARE_LEVEL_EXTERNAL -> "EXTERNAL"
        else -> "desconocido"
    }

    private fun mp(w: Int, h: Int) = String.format("%.1f MP", w.toLong() * h / 1e6)

    fun describe(context: Context): String {
        val mgr = context.getSystemService(Context.CAMERA_SERVICE) as CameraManager
        val sb = StringBuilder()
        sb.appendLine("CÁMARA FUSIÓN · informe de la cámara")
        sb.appendLine("Teléfono: ${Build.MANUFACTURER} ${Build.MODEL}  (Android ${Build.VERSION.RELEASE}, API ${Build.VERSION.SDK_INT})")
        sb.appendLine("Chip/placa: ${Build.HARDWARE} / ${Build.BOARD}")
        sb.appendLine()
        for (id in mgr.cameraIdList) {
            val c = mgr.getCameraCharacteristics(id)
            val facing = when (c.get(CameraCharacteristics.LENS_FACING)) {
                CameraCharacteristics.LENS_FACING_BACK -> "TRASERA"
                CameraCharacteristics.LENS_FACING_FRONT -> "FRONTAL"
                else -> "externa"
            }
            sb.appendLine("── Cámara $id ($facing)")
            sb.appendLine("Nivel de hardware: ${level(c.get(CameraCharacteristics.INFO_SUPPORTED_HARDWARE_LEVEL))}")
            val caps = c.get(CameraCharacteristics.REQUEST_AVAILABLE_CAPABILITIES)?.toList() ?: emptyList()
            sb.appendLine("Capacidades: " + caps.joinToString(", ") { capNames[it] ?: "#$it" })
            val raw = caps.contains(CameraMetadata.REQUEST_AVAILABLE_CAPABILITIES_RAW)
            sb.appendLine("RAW (datos en bruto): " + if (raw) "SÍ" else "NO — solo se puede usar YUV/JPEG")
            sb.appendLine("Ráfaga rápida (BURST_CAPTURE): " + if (caps.contains(CameraMetadata.REQUEST_AVAILABLE_CAPABILITIES_BURST_CAPTURE)) "SÍ" else "no declarada")
            sb.appendLine("Control manual del sensor: " + if (caps.contains(CameraMetadata.REQUEST_AVAILABLE_CAPABILITIES_MANUAL_SENSOR)) "SÍ" else "NO")

            c.get(CameraCharacteristics.SENSOR_INFO_PIXEL_ARRAY_SIZE)?.let { sb.appendLine("Sensor: ${it.width}×${it.height} (${mp(it.width, it.height)})") }
            c.get(CameraCharacteristics.SENSOR_INFO_PHYSICAL_SIZE)?.let { sb.appendLine("Tamaño físico del sensor: %.2f × %.2f mm".format(it.width, it.height)) }
            sb.appendLine("Orientación del sensor: ${c.get(CameraCharacteristics.SENSOR_ORIENTATION)}°")
            c.get(CameraCharacteristics.SENSOR_INFO_SENSITIVITY_RANGE)?.let { sb.appendLine("Rango ISO: ${it.lower}–${it.upper}") }
            c.get(CameraCharacteristics.SENSOR_INFO_EXPOSURE_TIME_RANGE)?.let { sb.appendLine("Rango de exposición: ${it.lower / 1000} µs – ${it.upper / 1_000_000} ms") }
            c.get(CameraCharacteristics.SENSOR_INFO_WHITE_LEVEL)?.let { sb.appendLine("Nivel de blanco RAW: $it") }
            c.get(CameraCharacteristics.LENS_INFO_AVAILABLE_FOCAL_LENGTHS)?.let { sb.appendLine("Distancia focal: ${it.joinToString { f -> "%.2f mm".format(f) }}") }
            c.get(CameraCharacteristics.LENS_INFO_AVAILABLE_APERTURES)?.let { sb.appendLine("Apertura: ${it.joinToString { f -> "f/%.1f".format(f) }}") }
            val ois = c.get(CameraCharacteristics.LENS_INFO_AVAILABLE_OPTICAL_STABILIZATION)
            sb.appendLine("Estabilización óptica (OIS): " + if (ois != null && ois.contains(CameraMetadata.LENS_OPTICAL_STABILIZATION_MODE_ON)) "SÍ" else "no")
            val af = c.get(CameraCharacteristics.CONTROL_AF_AVAILABLE_MODES)?.toList() ?: emptyList()
            sb.appendLine("Autoenfoque: " + if (af.contains(CameraMetadata.CONTROL_AF_MODE_AUTO)) "SÍ" else "NO (enfoque fijo)")
            val nr = c.get(CameraCharacteristics.NOISE_REDUCTION_AVAILABLE_NOISE_REDUCTION_MODES)?.toList() ?: emptyList()
            sb.appendLine("Modos de reducción de ruido: " + nr.joinToString { m -> when (m) { 0 -> "OFF"; 1 -> "FAST"; 2 -> "HIGH_QUALITY"; 3 -> "MINIMAL"; 4 -> "ZERO_SHUTTER_LAG"; else -> "#$m" } })

            val map = c.get(CameraCharacteristics.SCALER_STREAM_CONFIGURATION_MAP)
            if (map != null) {
                val yuv = map.getOutputSizes(ImageFormat.YUV_420_888)?.sortedByDescending { it.width.toLong() * it.height }
                if (yuv != null && yuv.isNotEmpty()) {
                    val top = yuv.first()
                    sb.appendLine("YUV máximo: ${top.width}×${top.height} (${mp(top.width, top.height)})")
                    val frame = map.getOutputMinFrameDuration(ImageFormat.YUV_420_888, top)
                    if (frame > 0) sb.appendLine("Velocidad teórica a ese tamaño: %.1f fotos/s".format(1e9 / frame))
                    sb.appendLine("Tamaños YUV: " + yuv.take(8).joinToString { "${it.width}×${it.height}" })
                }
                map.getOutputSizes(ImageFormat.JPEG)?.maxByOrNull { it.width.toLong() * it.height }?.let { sb.appendLine("JPEG máximo: ${it.width}×${it.height} (${mp(it.width, it.height)})") }
                map.getOutputSizes(ImageFormat.RAW_SENSOR)?.maxByOrNull { it.width.toLong() * it.height }?.let { sb.appendLine("RAW_SENSOR máximo: ${it.width}×${it.height}") }
            }
            sb.appendLine()
        }
        return sb.toString()
    }
}
