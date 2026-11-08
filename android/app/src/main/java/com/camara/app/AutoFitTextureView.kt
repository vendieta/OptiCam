package com.camara.app

import android.content.Context
import android.view.TextureView

/** TextureView que mantiene una relación de aspecto fija (ancho:alto mostrados). */
class AutoFitTextureView(context: Context) : TextureView(context) {
    private var ratioW = 0
    private var ratioH = 0

    fun setAspectRatio(w: Int, h: Int) {
        ratioW = w
        ratioH = h
        requestLayout()
    }

    override fun onMeasure(widthMeasureSpec: Int, heightMeasureSpec: Int) {
        val width = MeasureSpec.getSize(widthMeasureSpec)
        if (ratioW == 0 || ratioH == 0) {
            setMeasuredDimension(width, MeasureSpec.getSize(heightMeasureSpec))
        } else {
            setMeasuredDimension(width, width * ratioH / ratioW)
        }
    }
}
