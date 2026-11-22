// The documents' render cache (ipad.md §4): the copy of a PDF or image a
// surface opens, written as the app reads it from the backend, kept per
// backend and version. It's a cache and nothing more: never the only copy
// of anything, never a drawing, excluded from backup, protected until the
// device is first unlocked, and iOS may clear it.
//
// On "claktory/ink":
//   cacheBegin {backend, sha256, name} → {token}
//   cacheAppend {token, bytes}         → {written}
//   cacheFinish {token}                → {file}, once the whole file's
//                                         SHA-256 is the one named; else
//                                         error "hash_mismatch" and gone
//   cacheCancel {token}
//   cacheLookup {backend, sha256, name} → {file} or null
//   cacheClear {backend?}              (sign-out, a backend forgotten)

import Flutter
import UIKit

final class InkRenderCache {
  static let shared = InkRenderCache()

  private struct Pending {
    let handle: FileHandle
    let partial: URL
    let final: URL
    let sha256: String
  }

  private var pending: [String: Pending] = [:]
  let root: URL

  init(root: URL? = nil) {
    self.root =
      root
      ?? FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]
      .appendingPathComponent("ink-render", isDirectory: true)
  }

  func handle(_ call: FlutterMethodCall, _ result: @escaping FlutterResult) {
    let args = call.arguments as? [String: Any] ?? [:]
    switch call.method {
    case "cacheBegin":
      guard let url = file(args) else { return result(bad("backend, sha256 and name")) }
      do { result(["token": try begin(url, sha256: args["sha256"] as! String)]) } catch {
        result(FlutterError(code: "io", message: error.localizedDescription, details: nil))
      }
    case "cacheAppend":
      guard let token = args["token"] as? String, let p = pending[token],
        let bytes = args["bytes"] as? FlutterStandardTypedData
      else { return result(bad("token and bytes")) }
      p.handle.write(bytes.data)
      result(["written": bytes.data.count])
    case "cacheFinish":
      guard let token = args["token"] as? String else { return result(bad("token")) }
      finish(token) { outcome in
        switch outcome {
        case .success(let url): result(["file": url.path])
        case .failure(let error): result(error.flutter)
        }
      }
    case "cacheCancel":
      if let token = args["token"] as? String { cancel(token) }
      result(nil)
    case "cacheLookup":
      guard let url = file(args) else { return result(bad("backend, sha256 and name")) }
      result(FileManager.default.fileExists(atPath: url.path) ? ["file": url.path] : nil)
    case "cacheClear":
      clear(backend: args["backend"] as? String)
      result(nil)
    default:
      result(FlutterMethodNotImplemented)
    }
  }

  /// <root>/<backend>/<sha256>/<name>, every part a plain file name.
  func file(_ args: [String: Any]) -> URL? {
    guard let backend = Self.safe(args["backend"] as? String),
      let sha = args["sha256"] as? String, sha.count == 64, sha.allSatisfy(\.isHexDigit),
      let name = Self.safe(args["name"] as? String)
    else { return nil }
    return root.appendingPathComponent(backend, isDirectory: true)
      .appendingPathComponent(sha.lowercased(), isDirectory: true).appendingPathComponent(name)
  }

  private static func safe(_ part: String?) -> String? {
    guard let part, !part.isEmpty, part != ".", part != "..", !part.contains("/"),
      !part.contains("\0")
    else { return nil }
    return part
  }

  func begin(_ url: URL, sha256: String) throws -> String {
    let dir = url.deletingLastPathComponent()
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    var top = root
    var values = URLResourceValues()
    values.isExcludedFromBackup = true
    try? top.setResourceValues(values)  // `var`: setResourceValues is mutating
    let partial = dir.appendingPathComponent(".\(UUID().uuidString).partial")
    FileManager.default.createFile(
      atPath: partial.path, contents: nil,
      attributes: [.protectionKey: FileProtectionType.completeUntilFirstUserAuthentication])
    let handle = try FileHandle(forWritingTo: partial)
    let token = UUID().uuidString
    pending[token] = Pending(
      handle: handle, partial: partial, final: url, sha256: sha256.lowercased())
    return token
  }

  func append(_ token: String, _ data: Data) {
    pending[token]?.handle.write(data)
  }

  func finish(_ token: String, done: @escaping (Result<URL, InkCacheError>) -> Void) {
    guard let p = pending.removeValue(forKey: token) else {
      return done(.failure(InkCacheError(code: "bad_request", message: "token")))
    }
    try? p.handle.close()
    DispatchQueue.global(qos: .userInitiated).async {
      let actual = InkHash.sha256(file: p.partial)
      let outcome: Result<URL, InkCacheError>
      if actual == p.sha256 {
        try? FileManager.default.removeItem(at: p.final)
        do {
          try FileManager.default.moveItem(at: p.partial, to: p.final)
          outcome = .success(p.final)
        } catch {
          outcome = .failure(
            InkCacheError(code: "io", message: error.localizedDescription))
        }
      } else {
        try? FileManager.default.removeItem(at: p.partial)
        outcome = .failure(
          InkCacheError(
            code: "hash_mismatch", message: nil,
            details: ["expected_sha256": p.sha256, "actual_sha256": actual.inkJSON]))
      }
      DispatchQueue.main.async { done(outcome) }
    }
  }

  func cancel(_ token: String) {
    guard let p = pending.removeValue(forKey: token) else { return }
    try? p.handle.close()
    try? FileManager.default.removeItem(at: p.partial)
  }

  func clear(backend: String?) {
    if let backend = Self.safe(backend) {
      try? FileManager.default.removeItem(at: root.appendingPathComponent(backend))
    } else if backend == nil {
      try? FileManager.default.removeItem(at: root)
    }
  }

  private func bad(_ what: String) -> FlutterError {
    FlutterError(code: "bad_request", message: what, details: nil)
  }
}

/// Why a copy wasn't kept; FlutterError itself isn't a Swift Error.
struct InkCacheError: Error {
  var code: String
  var message: String?
  var details: [String: Any]?

  var flutter: FlutterError { FlutterError(code: code, message: message, details: details) }
}
