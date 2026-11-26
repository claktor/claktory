import Foundation
import Testing

@testable import Runner

/// Only a shared website's own address opens in the page over the app
/// (ShareBrowser.swift, W6), rebuilt from the parts that were checked: as
/// Android's ShareBrowserTest.
struct ShareBrowserTests {
  @Test func aSharesOwnAddressOpensAsItIs() {
    #expect(
      ShareBrowser.shareAddress("http://s-kmccoovm7ul5v6mtcqfs.localhost:21234/")?.absoluteString
        == "http://s-kmccoovm7ul5v6mtcqfs.localhost:21234/")
    #expect(
      ShareBrowser.shareAddress("http://s-abc.localhost:20001/d/x?claktory=ONE#top")?.absoluteString
        == "http://s-abc.localhost:20001/d/x?claktory=ONE#top")
    // Without a path, at its root.
    #expect(
      ShareBrowser.shareAddress("http://s-abc.localhost:20001")?.absoluteString
        == "http://s-abc.localhost:20001/")
    // The host in any case, opened in lowercase.
    #expect(
      ShareBrowser.shareAddress("HTTP://S-ABC.LOCALHOST:20001/")?.absoluteString
        == "http://s-abc.localhost:20001/")
  }

  @Test func nothingElseOpens() {
    for address in [
      nil,
      "",
      "not a url",
      "https://s-abc.localhost:20001/",
      "http://localhost:20001/",
      "http://127.0.0.1:20001/",
      "http://s-abc.localhost/",
      "http://s-abc.localhost:0/",
      "http://s-.localhost:20001/",
      "http://s-ab_c.localhost:20001/",
      "http://s-abc.localhost.example.com:20001/",
      "http://x.s-abc.localhost:20001/",
      "http://user@s-abc.localhost:20001/",
      "http://user:pass@s-abc.localhost:20001/",
      "http://s-\(String(repeating: "a", count: 65)).localhost:20001/",
      "javascript:alert(1)",
      "file:///etc/hosts",
    ] {
      #expect(ShareBrowser.shareAddress(address) == nil, "\(address ?? "nil")")
    }
  }

  /// What Safari will reach, asked of the system first: this device's own
  /// loopback, and nothing else.
  @Test func onlyLoopbackCountsAsReachable() {
    #expect(ShareBrowser.resolvesToLoopback("localhost"))
    #expect(ShareBrowser.resolvesToLoopback("127.0.0.1"))
    #expect(!ShareBrowser.resolvesToLoopback("192.0.2.1"))
    #expect(!ShareBrowser.resolvesToLoopback("name.invalid"))
    #expect(ShareBrowser.resolvesToLoopback("localhost", within: 1))
    // No answer in time counts as none.
    #expect(!ShareBrowser.resolvesToLoopback("localhost", within: 0))
  }

  /// The fallback (Fugo 38931): exactly 127.0.0.1, at the share's own
  /// address's port, rebuilt; nothing else.
  @Test func theFallbackIs127AtTheSamePortOnly() {
    #expect(
      ShareBrowser.fallbackAddress("http://127.0.0.1:21234/__claktory_open?k=once", port: 21234)?
        .absoluteString == "http://127.0.0.1:21234/__claktory_open?k=once")
    for address in [
      nil,
      "http://127.0.0.1:21235/",
      "http://127.0.0.1/",
      "https://127.0.0.1:21234/",
      "http://localhost:21234/",
      "http://[::1]:21234/",
      "http://127.0.0.2:21234/",
      "http://s-abc.localhost:21234/",
      "http://user@127.0.0.1:21234/",
      "http://127.0.0.1.example.com:21234/",
    ] {
      #expect(ShareBrowser.fallbackAddress(address, port: 21234) == nil, "\(address ?? "nil")")
    }
    #expect(ShareBrowser.fallbackAddress("http://127.0.0.1:21234/", port: nil) == nil)
  }
}
