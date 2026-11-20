import CoreImage
import Testing

@testable import Runner

struct RunnerTests {
  /// The camera's reading of QR codes, on an image of a pairing code drawn
  /// as the Mac draws it: the Simulator has no camera.
  @Test func readsAPairingCodeFromAQRImage() throws {
    let code =
      "factory://?t=0000000000000000000000000000000000000000000000000000000000000000"
      + "&name=standin&e=4fb375e4e5877d51fe09e40635833e932b26eb4eed4dc9cdacd1a64f678e3255"
      + "&r=https://euc1-1.relay.n0.iroh.link./"
    let filter = try #require(CIFilter(name: "CIQRCodeGenerator"))
    filter.setValue(Data(code.utf8), forKey: "inputMessage")
    filter.setValue("M", forKey: "inputCorrectionLevel")
    let modules = try #require(filter.outputImage)
    let scaled = modules.transformed(by: CGAffineTransform(scaleX: 8, y: 8))
    // Dark modules on white with a quiet zone around them.
    let white = CIImage(color: .white).cropped(to: scaled.extent.insetBy(dx: -64, dy: -64))
    let image = scaled.composited(over: white)
    let picture = try #require(CIContext().createCGImage(image, from: image.extent))
    #expect(CodeReader.read(picture) == [code])
  }

  @Test func readsNothingWithoutACode() throws {
    let blank = CIImage(color: .white).cropped(to: CGRect(x: 0, y: 0, width: 400, height: 400))
    let picture = try #require(CIContext().createCGImage(blank, from: blank.extent))
    #expect(CodeReader.read(picture).isEmpty)
  }
}
