package app.mirrorleaf

import android.app.Activity
import app.tauri.annotation.Command
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import com.google.mlkit.vision.barcode.common.Barcode
import com.google.mlkit.vision.codescanner.GmsBarcodeScannerOptions
import com.google.mlkit.vision.codescanner.GmsBarcodeScanning

/**
 * Scans the QR code another device shows when adding this one (E1). Google's code scanner runs in
 * Google Play services and hands back only the text, so the app needs no camera permission (owner,
 * 8 Nov 2026). Without Play services it fails, and the screen offers the word code instead.
 */
@TauriPlugin
class QrScannerPlugin(private val activity: Activity) : Plugin(activity) {
    @Command
    fun scan(invoke: Invoke) {
        val options = GmsBarcodeScannerOptions.Builder()
            .setBarcodeFormats(Barcode.FORMAT_QR_CODE)
            .enableAutoZoom()
            .build()
        GmsBarcodeScanning.getClient(activity, options).startScan()
            .addOnSuccessListener { code -> invoke.resolve(JSObject().put("text", code.rawValue ?: "")) }
            // Cancelled: no text, no error.
            .addOnCanceledListener { invoke.resolve(JSObject().put("text", "")) }
            .addOnFailureListener { e ->
                invoke.reject("The scanner couldn't start (${e.message}). Type the words instead.")
            }
    }
}
