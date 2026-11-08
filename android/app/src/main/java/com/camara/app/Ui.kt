package com.camara.app

import android.content.Context
import android.content.res.ColorStateList
import android.graphics.Color
import android.graphics.drawable.Drawable
import android.graphics.drawable.GradientDrawable
import android.graphics.drawable.RippleDrawable

/** Paleta y utilidades de dibujo compartidas por toda la interfaz (sin XML). */
object Ui {
    val BG = Color.parseColor("#0B0C0F")
    val SURFACE = Color.parseColor("#1A1C22")
    val SURFACE2 = Color.parseColor("#262932")
    val PANEL = Color.parseColor("#F2101114")
    val SCRIM = Color.parseColor("#99000000")
    val ACCENT = Color.parseColor("#FFC107")
    val DANGER = Color.parseColor("#E5484D")
    val MUTED = Color.parseColor("#9AA0AA")

    fun dp(ctx: Context, v: Int) = (v * ctx.resources.displayMetrics.density).toInt()

    fun shape(ctx: Context, color: Int, radiusDp: Int, strokeColor: Int = 0, strokeDp: Int = 0): GradientDrawable =
        GradientDrawable().apply {
            setColor(color)
            cornerRadius = dp(ctx, radiusDp).toFloat()
            if (strokeDp > 0) setStroke(dp(ctx, strokeDp), strokeColor)
        }

    fun oval(ctx: Context, color: Int, strokeColor: Int = 0, strokeDp: Int = 0): GradientDrawable =
        GradientDrawable().apply {
            shape = GradientDrawable.OVAL
            setColor(color)
            if (strokeDp > 0) setStroke(dp(ctx, strokeDp), strokeColor)
        }

    /** Fondo con esquinas redondeadas solo arriba (hojas inferiores). */
    fun topRounded(ctx: Context, color: Int, radiusDp: Int): GradientDrawable {
        val r = dp(ctx, radiusDp).toFloat()
        return GradientDrawable().apply {
            setColor(color)
            cornerRadii = floatArrayOf(r, r, r, r, 0f, 0f, 0f, 0f)
        }
    }

    /** Fondo redondeado con efecto de pulsación. */
    fun pressable(ctx: Context, color: Int, radiusDp: Int): Drawable =
        RippleDrawable(ColorStateList.valueOf(Color.parseColor("#33FFFFFF")), shape(ctx, color, radiusDp), shape(ctx, Color.WHITE, radiusDp))

    fun pressableOval(ctx: Context, color: Int): Drawable =
        RippleDrawable(ColorStateList.valueOf(Color.parseColor("#33FFFFFF")), oval(ctx, color), oval(ctx, Color.WHITE))
}
