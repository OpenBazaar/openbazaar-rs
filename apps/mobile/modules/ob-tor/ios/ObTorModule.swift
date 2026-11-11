import ExpoModulesCore
import ObMobileTor

final class TorException: GenericException<String>, @unchecked Sendable {
  override var reason: String { param }
}

public final class ObTorModule: Module {
  private static func dataDir() throws -> String {
    let base = try FileManager.default.url(for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: true)
    var dir = base.appendingPathComponent("ob-tor", isDirectory: true)
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    var values = URLResourceValues()
    values.isExcludedFromBackup = true
    try? dir.setResourceValues(values)
    return dir.path
  }

  private static func check(_ error: UnsafeMutablePointer<CChar>?) throws {
    guard let error else { return }
    defer { ob_tor_string_free(error) }
    throw TorException(String(cString: error))
  }

  public func definition() -> ModuleDefinition {
    Name("ObTor")

    // The Tor client's lines (crates/mobile-tor/src/log.rs) and the app's own go to the device log, in Release builds too.
    OnCreate {
      ob_tor_set_log { line in
        if let line { NSLog("%@", String(cString: line)) }
      }
    }

    Function("log") { (line: String) in
      NSLog("%@", line)
    }

    AsyncFunction("start") { () throws in
      let dir = try Self.dataDir()
      try Self.check(dir.withCString { ob_tor_start($0) })
    }

    Function("status") { () -> String in
      guard let raw = ob_tor_status() else { return "{\"state\":\"stopped\",\"progress\":0,\"summary\":\"\"}" }
      defer { ob_tor_string_free(raw) }
      return String(cString: raw)
    }

    AsyncFunction("insertDiscoveryKey") { (onion: String, secret: String) throws in
      try Self.check(onion.withCString { o in secret.withCString { s in ob_tor_insert_discovery_key(o, s) } })
    }

    AsyncFunction("exchange") { (onion: String, port: Int, requestBase64: String, maxBytes: Int, timeoutMs: Double) throws -> String in
      guard let request = Data(base64Encoded: requestBase64) else { throw TorException("the request is not base64") }
      let answer = onion.withCString { o in
        request.withUnsafeBytes { bytes in
          ob_tor_exchange(o, UInt16(port), bytes.bindMemory(to: UInt8.self).baseAddress, request.count, maxBytes, UInt64(timeoutMs))
        }
      }
      defer { ob_tor_bytes_free(answer) }
      if let error = answer.error { throw TorException(String(cString: error)) }
      guard let data = answer.data else { return "" }
      return Data(bytes: data, count: answer.len).base64EncodedString()
    }.runOnQueue(DispatchQueue.global(qos: .userInitiated))
  }
}
