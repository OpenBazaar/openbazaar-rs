package expo.modules.obdevicekey

import android.os.Build
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyInfo
import android.security.keystore.KeyProperties
import android.security.keystore.StrongBoxUnavailableException
import android.security.keystore.UserNotAuthenticatedException
import expo.modules.kotlin.exception.CodedException
import expo.modules.kotlin.modules.Module
import expo.modules.kotlin.modules.ModuleDefinition
import java.security.KeyFactory
import java.security.KeyPairGenerator
import java.security.KeyStore
import java.security.PrivateKey
import java.security.Signature
import java.security.interfaces.ECPublicKey
import java.security.spec.ECGenParameterSpec

class AuthRequiredException : CodedException("ERR_AUTH_REQUIRED", "Unlock with your fingerprint, face, or screen lock first.", null)

class DeviceKeyException(message: String) : CodedException("ERR_DEVICE_KEY", message, null)

/**
 * P-256 keys in the Android Keystore (StrongBox when the phone has it, else
 * the TEE). The device key needs the owner to have authenticated within the
 * last five minutes; the watch key signs background syncs without a prompt.
 */
class ObDeviceKeyModule : Module() {
  private val validitySeconds = 300

  private fun keyStore(): KeyStore = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }

  private fun spec(alias: String, userPresence: Boolean, strongBox: Boolean): KeyGenParameterSpec =
    KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_SIGN)
      .setAlgorithmParameterSpec(ECGenParameterSpec("secp256r1"))
      .setDigests(KeyProperties.DIGEST_SHA256)
      .apply {
        if (userPresence) {
          setUserAuthenticationRequired(true)
          if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            setUserAuthenticationParameters(validitySeconds, KeyProperties.AUTH_BIOMETRIC_STRONG or KeyProperties.AUTH_DEVICE_CREDENTIAL)
          } else {
            @Suppress("DEPRECATION")
            setUserAuthenticationValidityDurationSeconds(validitySeconds)
          }
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) setIsStrongBoxBacked(strongBox)
      }
      .build()

  private fun generate(alias: String, userPresence: Boolean) {
    val generator = KeyPairGenerator.getInstance(KeyProperties.KEY_ALGORITHM_EC, "AndroidKeyStore")
    try {
      generator.initialize(spec(alias, userPresence, strongBox = Build.VERSION.SDK_INT >= Build.VERSION_CODES.P))
      generator.generateKeyPair()
    } catch (err: StrongBoxUnavailableException) {
      generator.initialize(spec(alias, userPresence, strongBox = false))
      generator.generateKeyPair()
    } catch (err: IllegalStateException) {
      throw DeviceKeyException("Set a screen lock on this phone first.")
    }
  }

  private fun hardware(alias: String): Boolean {
    val key = keyStore().getKey(alias, null) as PrivateKey
    val info = KeyFactory.getInstance(key.algorithm, "AndroidKeyStore").getKeySpec(key, KeyInfo::class.java)
    return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
      info.securityLevel != KeyProperties.SECURITY_LEVEL_SOFTWARE
    } else {
      @Suppress("DEPRECATION")
      info.isInsideSecureHardware
    }
  }

  private fun compressed(alias: String): String? {
    val certificate = keyStore().getCertificate(alias) ?: return null
    val point = (certificate.publicKey as ECPublicKey).w
    val x = point.affineX.toByteArray().takeLast(32).toByteArray().let { ByteArray(32 - it.size) + it }
    val prefix = if (point.affineY.testBit(0)) 0x03 else 0x02
    return (byteArrayOf(prefix.toByte()) + x).joinToString("") { "%02x".format(it) }
  }

  override fun definition() = ModuleDefinition {
    Name("ObDeviceKey")

    AsyncFunction("create") { alias: String, userPresence: Boolean ->
      keyStore().deleteEntry(alias)
      generate(alias, userPresence)
      mapOf("publicKey" to compressed(alias), "hardware" to hardware(alias))
    }

    AsyncFunction("publicKey") { alias: String -> compressed(alias) }

    AsyncFunction("sign") { alias: String, preimage: String, _: Boolean ->
      val key = keyStore().getKey(alias, null) as? PrivateKey ?: throw DeviceKeyException("No key named $alias")
      val signature = Signature.getInstance("SHA256withECDSA")
      try {
        signature.initSign(key)
      } catch (err: UserNotAuthenticatedException) {
        throw AuthRequiredException()
      }
      signature.update(preimage.toByteArray(Charsets.UTF_8))
      signature.sign().joinToString("") { "%02x".format(it) }
    }

    AsyncFunction("delete") { alias: String -> keyStore().deleteEntry(alias) }
  }
}
