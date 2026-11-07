package expo.modules.obtor

/** JNI functions in crates/mobile-tor/src/jni.rs. Errors are IOExceptions. */
object ObTorNative {
  init {
    System.loadLibrary("ob_mobile_tor")
  }

  @JvmStatic external fun start(dataDir: String)

  @JvmStatic external fun status(): String

  @JvmStatic external fun insertDiscoveryKey(onion: String, secret: String)

  @JvmStatic external fun exchange(onion: String, port: Int, request: ByteArray, maxBytes: Int, timeoutMs: Long): ByteArray
}
