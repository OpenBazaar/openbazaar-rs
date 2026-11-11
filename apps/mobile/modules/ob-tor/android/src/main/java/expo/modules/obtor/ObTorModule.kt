package expo.modules.obtor

import android.util.Base64
import android.util.Log
import expo.modules.kotlin.exception.CodedException
import expo.modules.kotlin.functions.Coroutine
import expo.modules.kotlin.modules.Module
import expo.modules.kotlin.modules.ModuleDefinition
import java.io.File
import java.io.IOException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

class TorException(message: String?) : CodedException("ERR_TOR", message ?: "Tor failed", null)

class ObTorModule : Module() {
  private fun dataDir(): String {
    val context = appContext.reactContext ?: throw TorException("no context")
    val dir = File(context.noBackupFilesDir, "ob-tor")
    dir.mkdirs()
    return dir.absolutePath
  }

  private inline fun <T> tor(block: () -> T): T =
    try {
      block()
    } catch (err: IOException) {
      throw TorException(err.message)
    }

  override fun definition() = ModuleDefinition {
    Name("ObTor")

    AsyncFunction("start") { tor { ObTorNative.start(dataDir()) } }

    Function("status") { ObTorNative.status() }

    Function("log") { line: String ->
      Log.i("ObTor", line)
      Unit
    }

    AsyncFunction("insertDiscoveryKey") { onion: String, secret: String ->
      tor { ObTorNative.insertDiscoveryKey(onion, secret) }
    }

    AsyncFunction("exchange") Coroutine { onion: String, port: Int, requestBase64: String, maxBytes: Int, timeoutMs: Double ->
      withContext(Dispatchers.IO) {
        val request = Base64.decode(requestBase64, Base64.NO_WRAP)
        val answer = tor { ObTorNative.exchange(onion, port, request, maxBytes, timeoutMs.toLong()) }
        Base64.encodeToString(answer, Base64.NO_WRAP)
      }
    }
  }
}
