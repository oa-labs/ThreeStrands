package app.threestrands.mail

import android.content.res.Configuration
import android.os.Bundle
import android.view.View
import android.webkit.WebView
import android.widget.FrameLayout
import androidx.activity.enableEdgeToEdge
import androidx.core.graphics.Insets
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import java.lang.ref.WeakReference
import kotlin.math.roundToInt

class MainActivity : TauriActivity() {
    override val handleBackNavigation = true
    private var page = WeakReference<WebView>(null)

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
    }

    override fun setContentView(view: View?) {
        // Wry sets the WebView as the activity content. Inset its actual
        // bounds by putting it inside a native layout that owns the padding.
        if (view is WebView) {
            val layout = WebViewLayout(view)
            super.setContentView(layout)
            ViewCompat.requestApplyInsets(layout)
        } else {
            super.setContentView(view)
        }
    }

    override fun onWebViewCreate(webView: WebView) {
        page = WeakReference(webView)
        webView.settings.textZoom = (resources.configuration.fontScale * 100).roundToInt()
    }

    override fun onConfigurationChanged(newConfig: Configuration) {
        super.onConfigurationChanged(newConfig)
        page.get()?.settings?.textZoom = (newConfig.fontScale * 100).roundToInt()
    }

    override fun onResume() {
        super.onResume()
        page.get()?.evaluateJavascript("window.dispatchEvent(new Event('threestrands-resume'))", null)
    }
}

internal class WebViewLayout(content: View) : FrameLayout(content.context) {
    init {
        setBackgroundResource(R.color.mobile_paper)
        addView(content, LayoutParams(LayoutParams.MATCH_PARENT, LayoutParams.MATCH_PARENT))
        ViewCompat.setOnApplyWindowInsetsListener(this) { view, windowInsets ->
            val types = WindowInsetsCompat.Type.systemBars() or
                WindowInsetsCompat.Type.displayCutout() or WindowInsetsCompat.Type.ime()
            val insets = windowInsets.getInsets(types)
            view.setPadding(insets.left, insets.top, insets.right, insets.bottom)
            // Forward each update, including keyboard dismissal, with the
            // types already applied by the native layout cleared.
            WindowInsetsCompat.Builder(windowInsets).setInsets(types, Insets.NONE).build()
        }
    }
}
