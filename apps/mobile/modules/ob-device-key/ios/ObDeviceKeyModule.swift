import ExpoModulesCore
import LocalAuthentication
import Security

final class DeviceKeyException: GenericException<String>, @unchecked Sendable {
  override var reason: String { param }
}

/// P-256 keys for signing home node requests. The device key is in the Secure
/// Enclave and needs the owner present; one Face ID check covers five minutes
/// of requests. The watch key needs no prompt and signs only background syncs.
public final class ObDeviceKeyModule: Module {
  private static let reuseSeconds: TimeInterval = 300
  private var context: LAContext?
  private var contextAt = Date.distantPast

  private static func tag(_ alias: String) -> Data {
    Data("app.openbazaar.devicekey.\(alias)".utf8)
  }

  private func freshContext() async throws -> LAContext {
    if let context, Date().timeIntervalSince(contextAt) < Self.reuseSeconds {
      return context
    }
    let next = LAContext()
    next.touchIDAuthenticationAllowableReuseDuration = Self.reuseSeconds
    next.localizedReason = "Unlock your home node"
    try await next.evaluatePolicy(.deviceOwnerAuthentication, localizedReason: "Unlock your home node")
    context = next
    contextAt = Date()
    return next
  }

  private static func find(_ alias: String, context: LAContext?) throws -> SecKey? {
    var query: [String: Any] = [
      kSecClass as String: kSecClassKey,
      kSecAttrApplicationTag as String: tag(alias),
      kSecAttrKeyType as String: kSecAttrKeyTypeECSECPrimeRandom,
      kSecReturnRef as String: true,
    ]
    if let context { query[kSecUseAuthenticationContext as String] = context }
    var item: CFTypeRef?
    let status = SecItemCopyMatching(query as CFDictionary, &item)
    if status == errSecItemNotFound { return nil }
    guard status == errSecSuccess, let item else { throw DeviceKeyException("Keychain error \(status)") }
    return (item as! SecKey)
  }

  private static func compressed(_ key: SecKey) throws -> String {
    guard let publicKey = SecKeyCopyPublicKey(key),
          let raw = SecKeyCopyExternalRepresentation(publicKey, nil) as Data?, raw.count == 65 else {
      throw DeviceKeyException("Could not read the public key")
    }
    let prefix: UInt8 = raw[64] & 1 == 0 ? 0x02 : 0x03
    return ([prefix] + raw[1...32]).map { String(format: "%02x", $0) }.joined()
  }

  private static func create(_ alias: String, userPresence: Bool, secureEnclave: Bool) throws -> SecKey {
    var flags: SecAccessControlCreateFlags = secureEnclave ? [.privateKeyUsage] : []
    if userPresence { flags.insert(.userPresence) }
    let protection = userPresence ? kSecAttrAccessibleWhenUnlockedThisDeviceOnly : kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
    var error: Unmanaged<CFError>?
    guard let access = SecAccessControlCreateWithFlags(kCFAllocatorDefault, protection, flags, &error) else {
      throw DeviceKeyException("Could not set the key's access rules")
    }
    var attributes: [String: Any] = [
      kSecAttrKeyType as String: kSecAttrKeyTypeECSECPrimeRandom,
      kSecAttrKeySizeInBits as String: 256,
      kSecPrivateKeyAttrs as String: [
        kSecAttrIsPermanent as String: true,
        kSecAttrApplicationTag as String: tag(alias),
        kSecAttrAccessControl as String: access,
      ] as [String: Any],
    ]
    if secureEnclave { attributes[kSecAttrTokenID as String] = kSecAttrTokenIDSecureEnclave }
    guard let key = SecKeyCreateRandomKey(attributes as CFDictionary, &error) else {
      let reason = error?.takeRetainedValue().localizedDescription ?? "unknown"
      throw DeviceKeyException("Could not create the key: \(reason)")
    }
    return key
  }

  private static func delete(_ alias: String) {
    let query: [String: Any] = [kSecClass as String: kSecClassKey, kSecAttrApplicationTag as String: tag(alias)]
    SecItemDelete(query as CFDictionary)
  }

  public func definition() -> ModuleDefinition {
    Name("ObDeviceKey")

    AsyncFunction("create") { (alias: String, userPresence: Bool) throws -> [String: Any] in
      Self.delete(alias)
      do {
        return ["publicKey": try Self.compressed(Self.create(alias, userPresence: userPresence, secureEnclave: true)), "hardware": true]
      } catch {
        #if targetEnvironment(simulator)
        // The simulator has no Secure Enclave.
        let key = try Self.create(alias, userPresence: userPresence, secureEnclave: false)
        return ["publicKey": try Self.compressed(key), "hardware": false]
        #else
        throw error
        #endif
      }
    }

    AsyncFunction("publicKey") { (alias: String) throws -> String? in
      guard let key = try Self.find(alias, context: nil) else { return nil }
      return try Self.compressed(key)
    }

    AsyncFunction("sign") { (alias: String, preimage: String, userPresence: Bool) async throws -> String in
      let context = userPresence ? try await self.freshContext() : nil
      guard let key = try Self.find(alias, context: context) else { throw DeviceKeyException("No key named \(alias)") }
      var error: Unmanaged<CFError>?
      guard let signature = SecKeyCreateSignature(key, .ecdsaSignatureMessageX962SHA256, Data(preimage.utf8) as CFData, &error) as Data? else {
        self.context = nil
        let reason = error?.takeRetainedValue().localizedDescription ?? "unknown"
        throw DeviceKeyException("Could not sign: \(reason)")
      }
      return signature.map { String(format: "%02x", $0) }.joined()
    }

    AsyncFunction("delete") { (alias: String) in
      Self.delete(alias)
    }
  }
}
