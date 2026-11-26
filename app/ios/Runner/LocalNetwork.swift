import Foundation
import Network
import UIKit

/// Whether Claktory may reach devices on the local network: iOS's Local
/// Network privacy, which the tunnel's direct path to a Claktory on the
/// same Wi-Fi needs (lib/mobile/local_network.dart). iOS has no API that
/// says; a Bonjour listener and a browser for it, on a service type of
/// Claktory's own (Info.plist's NSBonjourServices), find out: the browser
/// finds the listener once it's allowed, and waits with PolicyDenied when
/// it isn't. Not answered yet, iOS asks the person first, and the check
/// waits for their answer, so it runs only where that prompt is meant to
/// come.
final class LocalNetworkCheck {
  static let serviceType = "_claktory-check._tcp"

  /// How long a check waits for an answer before it says it couldn't
  /// tell: the person may leave iOS's prompt up a while.
  static let patience: TimeInterval = 120

  private let queue = DispatchQueue(label: "claktory.local-network-check")
  private let name = UUID().uuidString
  private var listener: NWListener?
  private var browser: NWBrowser?
  private var completion: ((String) -> Void)?
  private var timeout: DispatchWorkItem?

  /// The checks running, kept until each answers.
  private static var running: [ObjectIdentifier: LocalNetworkCheck] = [:]

  /// "granted", "denied" or "unknown" (it couldn't tell: no network to
  /// browse on, or no answer in time), on the main queue.
  static func check(_ completion: @escaping (String) -> Void) {
    if let pretend = Pretend.current() {
      Pretend.answer(pretend, completion)
      return
    }
    let check = LocalNetworkCheck()
    running[ObjectIdentifier(check)] = check
    check.start { answer in
      DispatchQueue.main.async {
        running[ObjectIdentifier(check)] = nil
        completion(answer)
      }
    }
  }

  /// The app's own page in Settings, where its Local Network switch is.
  static func openSettings() {
    if let url = URL(string: UIApplication.openSettingsURLString) {
      UIApplication.shared.open(url)
    }
  }

  private func start(_ completion: @escaping (String) -> Void) {
    queue.async { [self] in
      self.completion = completion
      do {
        let listener = try NWListener(using: .tcp)
        listener.service = NWListener.Service(name: name, type: Self.serviceType)
        // Without one, the listener fails (EINVAL); nothing connects.
        listener.newConnectionHandler = { $0.cancel() }
        listener.stateUpdateHandler = { [weak self] state in
          switch state {
          case .failed(let error), .waiting(let error):
            if Self.denied(error) { self?.finish("denied") }
          default: break
          }
        }
        self.listener = listener
        listener.start(queue: queue)
      } catch {
        finish("unknown")
        return
      }
      let parameters = NWParameters()
      parameters.includePeerToPeer = true
      let browser = NWBrowser(for: .bonjour(type: Self.serviceType, domain: nil), using: parameters)
      browser.stateUpdateHandler = { [weak self] state in
        switch state {
        case .waiting(let error):
          if Self.denied(error) { self?.finish("denied") }
        case .failed(let error):
          self?.finish(Self.denied(error) ? "denied" : "unknown")
        default: break
        }
      }
      browser.browseResultsChangedHandler = { [weak self] results, _ in
        guard let self else { return }
        let found = results.contains { result in
          if case .service(let name, _, _, _) = result.endpoint { return name == self.name }
          return false
        }
        if found { self.finish("granted") }
      }
      self.browser = browser
      browser.start(queue: queue)
      let timeout = DispatchWorkItem { [weak self] in self?.finish("unknown") }
      self.timeout = timeout
      queue.asyncAfter(deadline: .now() + Self.patience, execute: timeout)
    }
  }

  private static func denied(_ error: NWError) -> Bool {
    if case .dns(let code) = error { return code == DNSServiceErrorType(kDNSServiceErr_PolicyDenied) }
    return false
  }

  /// Once: the answer, and everything stopped.
  private func finish(_ answer: String) {
    guard let completion else { return }
    self.completion = nil
    timeout?.cancel()
    listener?.cancel()
    browser?.cancel()
    listener = nil
    browser = nil
    completion(answer)
  }

  /// The Simulator has no Local Network switch: in any build but the App
  /// Store's, QA pretends one with a file in the app's data container,
  /// read at every check, so the answer can change while the app runs
  /// (back from Settings, say):
  ///
  ///   data=$(xcrun simctl get_app_container <udid> com.claktory.app.dev data)
  ///   echo denied > "$data/Library/ClaktoryLocalNetwork"  # granted, unanswered
  ///   rm "$data/Library/ClaktoryLocalNetwork"             # the real check
  ///
  /// "unanswered" waits as iOS's prompt does, until the file says granted
  /// or denied. Never in the App Store's build (com.claktory.app,
  /// testflight.sh's Release): the Runner target's Debug sets no DEBUG
  /// flag, so a build is told by its bundle id.
  enum Pretend {
    static func current() -> String? {
      guard Bundle.main.bundleIdentifier != AppStoreBuild.bundleId,
        let library = FileManager.default.urls(for: .libraryDirectory, in: .userDomainMask).first,
        let said = try? String(contentsOf: library.appendingPathComponent("ClaktoryLocalNetwork"))
      else { return nil }
      let value = said.trimmingCharacters(in: .whitespacesAndNewlines)
      return ["granted", "denied", "unanswered"].contains(value) ? value : nil
    }

    static func answer(_ pretend: String, _ completion: @escaping (String) -> Void) {
      if pretend != "unanswered" {
        DispatchQueue.main.async { completion(pretend) }
        return
      }
      DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
        switch current() {
        case "unanswered": answer("unanswered", completion)
        case let value?: completion(value)
        case nil: check(completion)
        }
      }
    }
  }
}

/// The App Store's build, the only one with this bundle id (#501).
enum AppStoreBuild {
  static let bundleId = "com.claktory.app"
}
