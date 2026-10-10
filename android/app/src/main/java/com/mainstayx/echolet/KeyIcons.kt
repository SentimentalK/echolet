package com.mainstayx.echolet

import android.graphics.Canvas
import android.graphics.Color
import android.graphics.ColorFilter
import android.graphics.Paint
import android.graphics.Path
import android.graphics.PixelFormat
import android.graphics.drawable.Drawable

/**
 * Small stroke-drawn keyboard glyphs (chevron, backspace) drawn in a 24-unit
 * view box, so the keyboard needs no font glyphs or XML resources and matches
 * the thin-line look of the desktop panel.
 */
class KeyIconDrawable(
    private val kind: Kind,
    private val sizePx: Int,
    strokePx: Float,
    color: Int,
) : Drawable() {

    enum class Kind { CHEVRON_UP, CHEVRON_DOWN, BACKSPACE }

    private val strokePaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        style = Paint.Style.STROKE
        strokeWidth = strokePx
        strokeCap = Paint.Cap.ROUND
        strokeJoin = Paint.Join.ROUND
        this.color = color
    }

    private val fillPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        style = Paint.Style.FILL
        this.color = color
    }

    override fun getIntrinsicWidth(): Int = sizePx
    override fun getIntrinsicHeight(): Int = sizePx

    override fun draw(canvas: Canvas) {
        val b = bounds
        val side = minOf(b.width(), b.height()).toFloat()
        val s = side / 24f
        val ox = b.left + (b.width() - side) / 2f
        val oy = b.top + (b.height() - side) / 2f
        fun x(v: Float) = ox + v * s
        fun y(v: Float) = oy + v * s

        when (kind) {
            Kind.CHEVRON_UP -> {
                // Matching ic_arrow_up: M7.41,15.41L12,10.83l4.59,4.58L18,14l-6,-6 -6,6z
                val path = Path().apply {
                    moveTo(x(7.41f), y(15.41f))
                    lineTo(x(12f), y(10.83f))
                    lineTo(x(16.59f), y(15.41f))
                    lineTo(x(18f), y(14f))
                    lineTo(x(12f), y(8f))
                    lineTo(x(6f), y(14f))
                    close()
                }
                canvas.drawPath(path, fillPaint)
            }
            Kind.CHEVRON_DOWN -> {
                // Matching ic_arrow_down: M7.41,8.59L12,13.17l4.59,-4.58L18,10l-6,6 -6,-6z
                val path = Path().apply {
                    moveTo(x(7.41f), y(8.59f))
                    lineTo(x(12f), y(13.17f))
                    lineTo(x(16.59f), y(8.59f))
                    lineTo(x(18f), y(10f))
                    lineTo(x(12f), y(16f))
                    lineTo(x(6f), y(10f))
                    close()
                }
                canvas.drawPath(path, fillPaint)
            }
            Kind.BACKSPACE -> {
                // Matching ic_backspace_dark: M22,3H7c-0.69,0 -1.23,0.35 -1.59,0.88L0,12l5.41,8.11c0.36,0.53 0.9,0.89 1.59,0.89h15c1.1,0 2,-0.9 2,-2V5c0,-1.1 -0.9,-2 -2,-2z
                // and inner X cutout
                val outerPath = Path().apply {
                    moveTo(x(22f), y(4f))
                    lineTo(x(7.5f), y(4f))
                    cubicTo(x(6.8f), y(4f), x(6.3f), y(4.4f), x(5.9f), y(4.9f))
                    lineTo(x(0.8f), y(12f))
                    lineTo(x(5.9f), y(19.1f))
                    cubicTo(x(6.3f), y(19.6f), x(6.8f), y(20f), x(7.5f), y(20f))
                    lineTo(x(22f), y(20f))
                    cubicTo(x(23.1f), y(20f), x(24f), y(19.1f), x(24f), y(18f))
                    lineTo(x(24f), y(6f))
                    cubicTo(x(24f), y(4.9f), x(23.1f), y(4f), x(22f), y(4f))
                    close()
                }
                // X mark in path (winding rule creates cutout or draw over)
                val xPath = Path().apply {
                    moveTo(x(18.5f), y(15.5f)); lineTo(x(12.5f), y(9.5f))
                    moveTo(x(12.5f), y(15.5f)); lineTo(x(18.5f), y(9.5f))
                }
                canvas.drawPath(outerPath, fillPaint)
                val whiteStroke = Paint(Paint.ANTI_ALIAS_FLAG).apply {
                    style = Paint.Style.STROKE
                    strokeWidth = 2.2f * s
                    strokeCap = Paint.Cap.ROUND
                    color = Color.WHITE
                }
                canvas.drawPath(xPath, whiteStroke)
            }
        }
    }

    override fun setAlpha(alpha: Int) {
        strokePaint.alpha = alpha
        fillPaint.alpha = alpha
        invalidateSelf()
    }

    override fun setColorFilter(colorFilter: ColorFilter?) {
        strokePaint.colorFilter = colorFilter
        fillPaint.colorFilter = colorFilter
        invalidateSelf()
    }

    @Deprecated("Deprecated in Java")
    override fun getOpacity(): Int = PixelFormat.TRANSLUCENT
}
