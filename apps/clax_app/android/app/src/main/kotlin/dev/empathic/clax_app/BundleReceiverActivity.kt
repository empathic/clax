package dev.empathic.clax_app

import android.app.Activity
import android.content.Intent
import android.os.Bundle

/**
 * Takes the `.clax` files other apps open or share with Clax (the `VIEW`
 * and `SEND` filters in `AndroidManifest.xml`) and forwards each to the one
 * `MainActivity`, in Clax's own task, as a
 * [BundleIntake.ACTION_OPEN_BUNDLE] intent carrying the URI and its read
 * grant. It shows nothing and keeps no history, so the sender's task never
 * holds a second `MainActivity` or engine.
 */
class BundleReceiverActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val uri = BundleIntake.acceptable(this, BundleIntake.uriOf(intent))
        if (savedInstanceState == null && uri != null) {
            startActivity(
                Intent(this, MainActivity::class.java)
                    .setAction(BundleIntake.ACTION_OPEN_BUNDLE)
                    .setData(uri)
                    .addFlags(
                        Intent.FLAG_ACTIVITY_NEW_TASK or
                            Intent.FLAG_ACTIVITY_SINGLE_TOP or
                            Intent.FLAG_ACTIVITY_CLEAR_TOP or
                            Intent.FLAG_GRANT_READ_URI_PERMISSION
                    )
            )
        }
        finish()
    }
}
