package app.mirrorleaf

import android.app.Activity
import android.app.KeyguardManager
import android.hardware.biometrics.BiometricManager
import android.hardware.biometrics.BiometricPrompt
import android.os.Build
import android.os.CancellationSignal
import androidx.activity.result.ActivityResult
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin

/**
 * The app lock's unlock (M11, E4): the phone's own passcode, fingerprint or face, through Android's
 * own prompt, so there is no Mirrorleaf PIN (owner, 9 Nov 2026). Android 10 and later show one
 * prompt offering both; older versions ask for the passcode.
 */
@TauriPlugin
class LockPlugin(private val activity: Activity) : Plugin(activity) {
    private val keyguard get() = activity.getSystemService(KeyguardManager::class.java)

    /** Whether the phone has a screen lock the app lock can use. */
    @Command
    fun available(invoke: Invoke) {
        invoke.resolve(JSObject().put("available", keyguard.isDeviceSecure))
    }

    @Command
    fun authenticate(invoke: Invoke) {
        if (!keyguard.isDeviceSecure) {
            invoke.reject("Set a screen lock on this phone first.")
            return
        }
        if (Build.VERSION.SDK_INT >= 29) {
            activity.runOnUiThread { prompt(invoke) }
        } else {
            @Suppress("DEPRECATION")
            val intent = keyguard.createConfirmDeviceCredentialIntent("Unlock Mirrorleaf", null)
            if (intent == null) {
                invoke.resolve(JSObject().put("unlocked", true))
            } else {
                startActivityForResult(invoke, intent, "confirmed")
            }
        }
    }

    private fun prompt(invoke: Invoke) {
        val builder = BiometricPrompt.Builder(activity).setTitle("Unlock Mirrorleaf")
        if (Build.VERSION.SDK_INT >= 30) {
            builder.setAllowedAuthenticators(
                BiometricManager.Authenticators.BIOMETRIC_WEAK or
                    BiometricManager.Authenticators.DEVICE_CREDENTIAL,
            )
        } else {
            @Suppress("DEPRECATION")
            builder.setDeviceCredentialAllowed(true)
        }
        val callback = object : BiometricPrompt.AuthenticationCallback() {
            override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) {
                invoke.resolve(JSObject().put("unlocked", true))
            }

            override fun onAuthenticationError(errorCode: Int, errString: CharSequence) {
                invoke.resolve(JSObject().put("unlocked", false))
            }
        }
        builder.build().authenticate(CancellationSignal(), activity.mainExecutor, callback)
    }

    @ActivityCallback
    private fun confirmed(invoke: Invoke, result: ActivityResult) {
        invoke.resolve(JSObject().put("unlocked", result.resultCode == Activity.RESULT_OK))
    }
}
