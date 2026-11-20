package app.threestrands.mail

import android.content.res.Configuration
import android.os.Bundle
import android.webkit.WebView
import androidx.activity.enableEdgeToEdge
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

    override fun onWebViewCreate(webView: WebView) {
        page = WeakReference(webView)
        webView.settings.textZoom = (resources.configuration.fontScale * 100).roundToInt()
        ViewCompat.setOnApplyWindowInsetsListener(webView) { view, windowInsets ->
            val insets = windowInsets.getInsets(WindowInsetsCompat.Type.systemBars()
                or WindowInsetsCompat.Type.displayCutout() or WindowInsetsCompat.Type.ime())
            view.setPadding(insets.left, insets.top, insets.right, insets.bottom)
            WindowInsetsCompat.CONSUMED
        }
        ViewCompat.requestApplyInsets(webView)
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
