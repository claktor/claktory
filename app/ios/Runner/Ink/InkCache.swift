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
  /// The copies kept, at most, in bytes: past it, those used longest ago
  /// go when a new one is kept (ipad.md §4: a cache, never the truth).
  let limit: Int64

  init(root: URL? = nil, limit: Int64 = 1 << 30) {
    self.limit = limit
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
      guard FileManager.default.fileExists(atPath: url.path) else { return result(nil) }
      touch(url)
      result(["file": url.path])
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
          self.touch(p.final)
          self.trim(keeping: p.final)
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

  /// A copy used: the last to go.
  func touch(_ url: URL) {
    try? FileManager.default.setAttributes([.modificationDate: Date()], ofItemAtPath: url.path)
  }

  /// Removes the copies used longest ago until what's kept fits the limit,
  /// never [keeping] (just written) nor a copy still being written.
  func trim(keeping: URL?) {
    let keys: [URLResourceKey] = [.isRegularFileKey, .fileSizeKey, .contentModificationDateKey]
    guard
      let walk = FileManager.default.enumerator(
        at: root, includingPropertiesForKeys: keys, options: [.skipsHiddenFiles])
    else { return }
    var files: [(url: URL, size: Int64, used: Date)] = []
    for case let url as URL in walk {
      guard let v = try? url.resourceValues(forKeys: Set(keys)), v.isRegularFile == true else {
        continue
      }
      files.append((url, Int64(v.fileSize ?? 0), v.contentModificationDate ?? .distantPast))
    }
    var total = files.reduce(0) { $0 + $1.size }
    for file in files.sorted(by: { $0.used < $1.used }) where total > limit {
      if file.url.standardizedFileURL == keeping?.standardizedFileURL { continue }
      try? FileManager.default.removeItem(at: file.url)
      // Its version's folder, only once nothing is left in it.
      let folder = file.url.deletingLastPathComponent()
      if (try? FileManager.default.contentsOfDirectory(atPath: folder.path))?.isEmpty == true {
        try? FileManager.default.removeItem(at: folder)
      }
      total -= file.size
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

// MARK: The journal of drawings not yet saved

// Strokes drawn while the backend can't take them, kept on the iPad until
// it has them (Matt's decision: the fourth thing kept on a device). Only a
// page's drawing whose latest change isn't saved yet, by Claktory and
// document (both by hash: no file name is written), removed as soon as its
// save is confirmed. Never backed up, never in a cache iOS may clear,
// readable only while the iPad is unlocked.
//
// On "claktory/ink":
//   journalPut {claktory, document, page_1based?, seq, source_sha256,
//               ink_layer_id?, base_revision?, drawing: {format,
//               content_version, bytes_base64, sha256}}
//   journalRemove {claktory, document, page_1based?, upto_seq?}
//                         the page's entry, if its seq is at most upto_seq
//   journalList {claktory, document} → {entries: [the put's fields]}
//   journalClear {claktory?}
final class InkJournal {
  static let shared = InkJournal()

  let root: URL

  init(root: URL? = nil) {
    self.root =
      root
      ?? FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
      .appendingPathComponent("ink-journal", isDirectory: true)
  }

  func handle(_ call: FlutterMethodCall, _ result: @escaping FlutterResult) {
    let args = call.arguments as? [String: Any] ?? [:]
    switch call.method {
    case "journalPut":
      guard let folder = folder(args), let seq = args["seq"] as? Int,
        let drawing = args["drawing"] as? [String: Any],
        let base64 = drawing["bytes_base64"] as? String, Data(base64Encoded: base64) != nil
      else { return result(bad("claktory, document, seq and drawing")) }
      do {
        try put(folder: folder, page: args["page_1based"] as? Int, seq: seq, fields: args, drawing: drawing)
        result(nil)
      } catch {
        result(FlutterError(code: "io", message: error.localizedDescription, details: nil))
      }
    case "journalRemove":
      guard let folder = folder(args) else { return result(bad("claktory and document")) }
      remove(folder: folder, page: args["page_1based"] as? Int, upto: args["upto_seq"] as? Int)
      result(nil)
    case "journalList":
      guard let folder = folder(args) else { return result(bad("claktory and document")) }
      result(["entries": list(folder: folder)])
    case "journalClear":
      if let claktory = args["claktory"] as? String {
        guard let name = Self.safe(claktory) else { return result(bad("claktory")) }
        try? FileManager.default.removeItem(at: root.appendingPathComponent(name))
      } else {
        try? FileManager.default.removeItem(at: root)
      }
      result(nil)
    default:
      result(FlutterMethodNotImplemented)
    }
  }

  /// <root>/<claktory>/<document>: both hex names the app made.
  func folder(_ args: [String: Any]) -> URL? {
    guard let claktory = Self.safe(args["claktory"] as? String),
      let document = Self.safe(args["document"] as? String)
    else { return nil }
    return root.appendingPathComponent(claktory, isDirectory: true)
      .appendingPathComponent(document, isDirectory: true)
  }

  /// Hex, as the app names them: nothing else reaches the disk as a name.
  private static func safe(_ part: String?) -> String? {
    guard let part, (1...128).contains(part.count),
      part.allSatisfy({ $0.isHexDigit })
    else { return nil }
    return part.lowercased()
  }

  private static let fields = ["page_1based", "seq", "source_sha256", "ink_layer_id", "base_revision"]
  private static let drawingFields = ["format", "content_version", "bytes_base64", "sha256"]

  private func name(_ page: Int?) -> String { page.map { "page-\($0)" } ?? "only" }

  func put(folder: URL, page: Int?, seq: Int, fields: [String: Any], drawing: [String: Any]) throws {
    let fm = FileManager.default
    if !fm.fileExists(atPath: root.path) {
      try fm.createDirectory(at: root, withIntermediateDirectories: true)
      var top = root
      var values = URLResourceValues()
      values.isExcludedFromBackup = true
      try? top.setResourceValues(values)
    }
    try fm.createDirectory(at: folder, withIntermediateDirectories: true)
    var record: [String: Any] = [:]
    for key in Self.fields { if let v = fields[key], !(v is NSNull) { record[key] = v } }
    record["seq"] = seq
    var kept: [String: Any] = [:]
    for key in Self.drawingFields { if let v = drawing[key], !(v is NSNull) { kept[key] = v } }
    record["drawing"] = kept
    let data = try JSONSerialization.data(withJSONObject: record)
    // Written whole, then moved over the last: never half an entry.
    try data.write(to: folder.appendingPathComponent(name(page) + ".json"), options: [.atomic, .completeFileProtection])
  }

  func remove(folder: URL, page: Int?, upto: Int?) {
    let file = folder.appendingPathComponent(name(page) + ".json")
    if let upto {
      guard let data = try? Data(contentsOf: file),
        let record = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
        let seq = record["seq"] as? Int, seq <= upto
      else { return }
    }
    try? FileManager.default.removeItem(at: file)
    let fm = FileManager.default
    if (try? fm.contentsOfDirectory(atPath: folder.path))?.isEmpty == true {
      try? fm.removeItem(at: folder)
    }
  }

  func list(folder: URL) -> [[String: Any]] {
    let files = (try? FileManager.default.contentsOfDirectory(at: folder, includingPropertiesForKeys: nil)) ?? []
    var entries: [[String: Any]] = []
    for file in files where file.pathExtension == "json" {
      guard let data = try? Data(contentsOf: file),
        let record = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
        record["drawing"] is [String: Any]
      else { continue }
      entries.append(record)
    }
    return entries.sorted { ($0["page_1based"] as? Int ?? 0) < ($1["page_1based"] as? Int ?? 0) }
  }

  private func bad(_ what: String) -> FlutterError {
    FlutterError(code: "bad_request", message: what, details: nil)
  }
}
