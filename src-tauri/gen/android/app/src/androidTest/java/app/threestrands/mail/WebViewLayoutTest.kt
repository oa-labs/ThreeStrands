package app.threestrands.mail

import android.graphics.Rect
import android.view.View
import androidx.core.graphics.Insets
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.filters.SdkSuppress
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.assertEquals
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
@SdkSuppress(minSdkVersion = 30)
class WebViewLayoutTest {
    @Test
    fun laysOutContentInsideSystemBarsAndResizesForTheKeyboard() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        instrumentation.runOnMainSync {
            val content = View(instrumentation.targetContext)
            val layout = WebViewLayout(content)
            val bars = WindowInsetsCompat.Type.statusBars() or WindowInsetsCompat.Type.navigationBars()
            val cutout = WindowInsetsCompat.Type.displayCutout()
            val ime = WindowInsetsCompat.Type.ime()
            var forwarded: Insets? = null
            ViewCompat.setOnApplyWindowInsetsListener(content) { _, insets ->
                forwarded = insets.getInsets(bars or cutout or ime)
                insets
            }

            fun applyInsets(keyboard: Int) {
                val insets = WindowInsetsCompat.Builder()
                    .setInsets(WindowInsetsCompat.Type.statusBars(), Insets.of(0, 28, 0, 0))
                    .setInsets(WindowInsetsCompat.Type.navigationBars(), Insets.of(0, 0, 0, 24))
                    .setInsets(cutout, Insets.of(12, 0, 8, 0))
                    .setInsets(ime, Insets.of(0, 0, 0, keyboard))
                    .setVisible(bars or cutout, true)
                    .setVisible(ime, keyboard > 0)
                    .build()
                ViewCompat.dispatchApplyWindowInsets(layout, insets)
                val exactly = View.MeasureSpec.EXACTLY
                layout.measure(View.MeasureSpec.makeMeasureSpec(400, exactly), View.MeasureSpec.makeMeasureSpec(900, exactly))
                layout.layout(0, 0, 400, 900)
                assertEquals(Insets.NONE, forwarded)
            }

            fun contentBounds() = Rect(content.left, content.top, content.right, content.bottom)
            applyInsets(0)
            assertEquals(Rect(12, 28, 392, 876), contentBounds())
            applyInsets(300)
            assertEquals(Rect(12, 28, 392, 600), contentBounds())
            applyInsets(0)
            assertEquals(Rect(12, 28, 392, 876), contentBounds())
        }
    }
}
