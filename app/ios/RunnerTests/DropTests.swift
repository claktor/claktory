import Foundation
import Testing
import UniformTypeIdentifiers

@testable import Runner

/// What an iPad takes from a drop (Drop.swift, #512): files attach as the
/// paperclip's do; text alone goes in as text.
@MainActor struct DropTests {
  @Test func picturesVideosAndNamedFilesAreFilesTextAloneIsnt() {
    // A photo from Photos, a picture from Safari.
    #expect(Drop.isFile(types: [UTType.heic.identifier, UTType.jpeg.identifier], named: true))
    #expect(Drop.isFile(types: [UTType.png.identifier, UTType.url.identifier], named: false))
    #expect(Drop.isFile(types: [UTType.quickTimeMovie.identifier], named: true))
    // A PDF from Files, and a text file from Files: named.
    #expect(Drop.isFile(types: [UTType.pdf.identifier], named: true))
    #expect(Drop.isFile(types: [UTType.plainText.identifier], named: true))
    // Data with no name of its own is still a file.
    #expect(Drop.isFile(types: [UTType.data.identifier], named: false))
    // A selection's words, a link: text.
    #expect(!Drop.isFile(types: [UTType.utf8PlainText.identifier, UTType.rtf.identifier], named: false))
    #expect(!Drop.isFile(types: [UTType.url.identifier, UTType.plainText.identifier], named: false))
  }

  @Test func aPictureIsCopiedAsJPEGThenPNGElseAsItIs() {
    #expect(
      Drop.copyType(of: [UTType.heic.identifier, UTType.jpeg.identifier]) == UTType.jpeg.identifier)
    #expect(
      Drop.copyType(of: [UTType.heic.identifier, UTType.png.identifier]) == UTType.png.identifier)
    #expect(Drop.copyType(of: [UTType.pdf.identifier]) == UTType.pdf.identifier)
    #expect(Drop.copyType(of: []) == nil)
  }

  @Test func aNameKeepsItsOwnExtensionOrGetsTheTypes() {
    #expect(Drop.fileName(suggested: "Plan.pdf", type: UTType.pdf.identifier, photo: false) == "Plan.pdf")
    #expect(Drop.fileName(suggested: "Plan", type: UTType.pdf.identifier, photo: false) == "Plan.pdf")
    // A HEIC photo copied as a JPEG says so.
    #expect(
      Drop.fileName(suggested: "IMG_0001", type: UTType.jpeg.identifier, photo: true)
        == "IMG_0001.jpeg")
    #expect(Drop.fileName(suggested: nil, type: UTType.jpeg.identifier, photo: true) == "Photo.jpeg")
    #expect(Drop.fileName(suggested: "", type: UTType.pdf.identifier, photo: false) == "File.pdf")
    // Never a path.
    #expect(
      Drop.fileName(suggested: "../../etc/notes.txt", type: UTType.plainText.identifier, photo: false)
        == "notes.txt")
  }

  @Test func aSavedTextIsNamedOnlyByItsLastPart() {
    #expect(
      AppDelegate.savedTextName("Claktory recovery key (Studio).txt")
        == "Claktory recovery key (Studio).txt")
    #expect(AppDelegate.savedTextName("../../Library/key.txt") == "key.txt")
    #expect(AppDelegate.savedTextName("") == nil)
    #expect(AppDelegate.savedTextName(".") == nil)
    #expect(AppDelegate.savedTextName("..") == nil)
    #expect(AppDelegate.savedTextName("/") == nil)
  }

  @Test func copiesGoInAFolderOfTheirOwn() throws {
    let source = FileManager.default.temporaryDirectory.appendingPathComponent("drop-source.txt")
    try "hello".write(to: source, atomically: true, encoding: .utf8)
    let one = try #require(Drop.copy(source, as: "notes.txt"))
    let two = try #require(Drop.copy(source, as: "notes.txt"))
    #expect(one != two)
    #expect(URL(fileURLWithPath: one).lastPathComponent == "notes.txt")
    #expect(one.contains("/Attachments/"))
    #expect(try String(contentsOfFile: one, encoding: .utf8) == "hello")
  }

  @Test func providersCopyInOrderLeavingOutWhatCantBe() async throws {
    let text = NSItemProvider(item: "first" as NSString, typeIdentifier: UTType.plainText.identifier)
    text.suggestedName = "a.txt"
    let empty = NSItemProvider()
    let data = NSItemProvider(item: Data([1, 2, 3]) as NSData, typeIdentifier: UTType.data.identifier)
    data.suggestedName = "b"
    let paths = await withCheckedContinuation { done in
      Drop.shared.copy([text, empty, data]) { done.resume(returning: $0) }
    }
    #expect(paths.count == 2)
    #expect(paths.first.map { URL(fileURLWithPath: $0).lastPathComponent } == "a.txt")
    #expect(paths.last.map { URL(fileURLWithPath: $0).lastPathComponent }?.hasPrefix("b") == true)
  }
}
