package dev.empathic.clax_app

import android.app.Activity
import android.content.Intent
import android.os.Bundle

/**
 * Takes the `.clax` files other apps open or share with Clax, and the Get
 * in Clax links (the `VIEW` and `SEND` filters in `AndroidManifest.xml`),
 * and forwards each to the one `MainActivity`, in Clax's own task: a file
 * as a [BundleIntake.ACTION_OPEN_BUNDLE] intent carrying the URI and its
 * read grant, a link as a [BundleIntake.ACTION_OPEN_LINK] intent carrying
 * the link. It shows nothing and keeps no history, so the sender's task
 * never holds a second `MainActivity` or engine.
 */
class BundleReceiverActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (savedInstanceState == null) {
            val link = BundleIntake.linkOf(intent)
            val uri = if (link == null) BundleIntake.acceptable(this, BundleIntake.uriOf(intent)) else null
            val forward = when {
                link != null -> Intent(this, MainActivity::class.java)
                    .setAction(BundleIntake.ACTION_OPEN_LINK)
                    .setData(link)
                uri != null -> Intent(this, MainActivity::class.java)
                    .setAction(BundleIntake.ACTION_OPEN_BUNDLE)
                    .setData(uri)
                    .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                else -> null
            }
            forward?.let {
                startActivity(
                    it.addFlags(
                        Intent.FLAG_ACTIVITY_NEW_TASK or
                            Intent.FLAG_ACTIVITY_SINGLE_TOP or
                            Intent.FLAG_ACTIVITY_CLEAR_TOP
                    )
                )
            }
        }
        finish()
    }
}
