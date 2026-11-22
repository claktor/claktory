// The Mac's render cache for documents (ipad.md §4), as the iPad's
// (app/ios/Runner/Ink/InkCache.swift): the copy of a PDF a viewer opens,
// written as the app reads it from the backend, kept per backend and
// version. A cache and nothing more: never the only copy of anything.
// It lives in this app's own Caches folder (Claktory Dev's apart from
// Claktory's), readable by this user only, and keeps at most 1 GB: the
// copies used longest ago go first. Foundation only, so
// RunnerTests/PaperCheck.swift checks it without the app; PaperViewer.swift
// puts it on the "claktory/ink" channel:
//   cacheBegin {backend, sha256, name} → {token}
//   cacheAppend {token, bytes}         → {written}
//   cacheFinish {token}                → {file}, once the whole file's
//                                         SHA-256 is the one named; else
//                                         error "hash_mismatch" and gone
//   cacheCancel {token}
//   cacheLookup {backend, sha256, name} → {file} or null
//   cacheClear {backend?}              (sign-out, a backend forgotten)

import Foundation

final class PaperCache {
  static let shared = PaperCache()

  private struct Pending {
    let handle: FileHandle
    let partial: URL
    let final: URL
    let sha256: String
  }

  private var pending: [String: Pending] = [:]
  private let lock = NSLock()
  let root: URL
  let limit: Int64

  init(root: URL? = nil, limit: Int64 = 1 << 30) {
    self.limit = limit
    self.root =
      root
      ?? FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]
      .appendingPathComponent(Bundle.main.bundleIdentifier ?? "Claktory", isDirectory: true)
      .appendingPathComponent("ink-render", isDirectory: true)
  }

  /// <root>/<backend>/<sha256>/<name>, every part a plain file name.
  func file(backend: String?, sha256: String?, name: String?) -> URL? {
    guard let backend = Self.safe(backend), let sha = sha256, sha.count == 64,
      sha.allSatisfy(\.isHexDigit), let name = Self.safe(name)
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

  /// The copy named, if it's here (and it counts as used).
  func lookup(_ url: URL) -> URL? {
    guard FileManager.default.fileExists(atPath: url.path) else { return nil }
    touch(url)
    return url
  }

  /// Every folder from the root down to [dir], this user's only.
  private func makeFolders(to dir: URL) throws {
    try FileManager.default.createDirectory(
      at: dir, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
    var folder = dir
    while folder.path.hasPrefix(root.path) {
      try? FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: folder.path)
      if folder.path == root.path { break }
      folder = folder.deletingLastPathComponent()
    }
  }

  func begin(_ url: URL, sha256: String) throws -> String {
    let dir = url.deletingLastPathComponent()
    try makeFolders(to: dir)
    var top = root
    var values = URLResourceValues()
    values.isExcludedFromBackup = true
    try? top.setResourceValues(values)  // `var`: setResourceValues is mutating
    let partial = dir.appendingPathComponent(".\(UUID().uuidString).partial")
    FileManager.default.createFile(
      atPath: partial.path, contents: nil, attributes: [.posixPermissions: 0o600])
    let handle = try FileHandle(forWritingTo: partial)
    let token = UUID().uuidString
    lock.lock()
    pending[token] = Pending(handle: handle, partial: partial, final: url, sha256: sha256.lowercased())
    lock.unlock()
    return token
  }

  /// Bytes for a copy being written; false for an unknown token.
  @discardableResult
  func append(_ token: String, _ data: Data) -> Bool {
    lock.lock()
    let p = pending[token]
    lock.unlock()
    guard let p else { return false }
    p.handle.write(data)
    return true
  }

  /// Keeps the copy once its whole SHA-256 is the one named, off the
  /// calling thread; [done] on the main thread.
  func finish(_ token: String, done: @escaping (Result<URL, PaperCacheError>) -> Void) {
    lock.lock()
    let p = pending.removeValue(forKey: token)
    lock.unlock()
    guard let p else { return done(.failure(PaperCacheError(code: "bad_request", message: "token"))) }
    try? p.handle.close()
    DispatchQueue.global(qos: .userInitiated).async {
      let actual = PaperHash.sha256(file: p.partial)
      let outcome: Result<URL, PaperCacheError>
      if actual == p.sha256 {
        try? FileManager.default.removeItem(at: p.final)
        do {
          try FileManager.default.moveItem(at: p.partial, to: p.final)
          self.touch(p.final)
          self.trim(keeping: p.final)
          outcome = .success(p.final)
        } catch {
          outcome = .failure(PaperCacheError(code: "io", message: error.localizedDescription))
        }
      } else {
        try? FileManager.default.removeItem(at: p.partial)
        outcome = .failure(
          PaperCacheError(
            code: "hash_mismatch", message: nil,
            details: ["expected_sha256": p.sha256, "actual_sha256": actual ?? NSNull()]))
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
    lock.lock()
    let p = pending.removeValue(forKey: token)
    lock.unlock()
    guard let p else { return }
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
}

/// Why a copy wasn't kept.
struct PaperCacheError: Error {
  var code: String
  var message: String?
  var details: [String: Any]?
}
