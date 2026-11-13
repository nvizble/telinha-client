// Video-only AVFoundation worker. No microphone, recording or image files.
import AVFoundation
import CoreMedia
import CoreVideo
import Foundation
import Darwin

let mode = CommandLine.arguments.dropFirst().first ?? ""
let outputLock = NSLock()
let output = FileHandle.standardOutput

func write(_ data: Data) {
    outputLock.lock()
    defer { outputLock.unlock() }
    do { try output.write(contentsOf: data) }
    catch { Darwin._exit(0) }
}

func header(_ width: UInt32, _ height: UInt32) -> Data {
    var w = width.littleEndian, h = height.littleEndian
    var data = withUnsafeBytes(of: &w) { Data($0) }
    data.append(withUnsafeBytes(of: &h) { Data($0) })
    return data
}

func fail(_ message: String) -> Never {
    if mode == "list" {
        write(try! JSONSerialization.data(withJSONObject: ["Err": message]))
    } else {
        let text = Data(message.utf8)
        var packet = header(0, UInt32(text.count))
        packet.append(text)
        write(packet)
    }
    fputs("telinha webcam: \(message)\n", stderr)
    Darwin._exit(1)
}

func packet(for buffer: CVPixelBuffer) -> Data {
    guard CVPixelBufferLockBaseAddress(buffer, .readOnly) == kCVReturnSuccess else {
        fail("Não deu pra ler a imagem desta webcam.")
    }
    defer { CVPixelBufferUnlockBaseAddress(buffer, .readOnly) }
    let width = CVPixelBufferGetWidth(buffer), height = CVPixelBufferGetHeight(buffer)
    let stride = CVPixelBufferGetBytesPerRow(buffer)
    guard width >= 2, height >= 2, width <= 4096, height <= 2160,
          CVPixelBufferGetPixelFormatType(buffer) == kCVPixelFormatType_32BGRA,
          stride >= width * 4, let base = CVPixelBufferGetBaseAddress(buffer) else {
        fail("A webcam enviou um formato de imagem inesperado.")
    }
    var data = header(UInt32(width), UInt32(height))
    data.reserveCapacity(8 + width * height * 4)
    // USB pixel buffers may have padded rows. The pipe uses tight BGRA.
    for row in 0..<height {
        data.append(base.advanced(by: row * stride).assumingMemoryBound(to: UInt8.self), count: width * 4)
    }
    return data
}

// Closing/force-quitting the app closes stdin. Do not wait for a blocked driver.
DispatchQueue.global(qos: .utility).async {
    var byte: UInt8 = 0
    while Darwin.read(STDIN_FILENO, &byte, 1) > 0 {}
    Darwin._exit(0)
}

if mode == "probe" {
    // Exercise the same pixel-buffer packing as a USB camera, including padding.
    // This branch never enumerates devices or requests camera permission.
    var pixelBuffer: CVPixelBuffer?
    let attributes = [kCVPixelBufferBytesPerRowAlignmentKey: 64] as CFDictionary
    guard CVPixelBufferCreate(kCFAllocatorDefault, 2, 2, kCVPixelFormatType_32BGRA,
                              attributes, &pixelBuffer) == kCVReturnSuccess,
          let buffer = pixelBuffer,
          CVPixelBufferLockBaseAddress(buffer, []) == kCVReturnSuccess,
          let base = CVPixelBufferGetBaseAddress(buffer),
          CVPixelBufferGetBytesPerRow(buffer) >= 64 else { fail("Pixel buffer de teste inválido.") }
    let stride = CVPixelBufferGetBytesPerRow(buffer)
    base.initializeMemory(as: UInt8.self, repeating: 0x7f, count: stride * 2)
    let row: [UInt8] = [0, 0, 255, 255, 0, 0, 255, 255]
    row.withUnsafeBytes { bytes in
        base.copyMemory(from: bytes.baseAddress!, byteCount: 8)
        base.advanced(by: stride).copyMemory(from: bytes.baseAddress!, byteCount: 8)
    }
    CVPixelBufferUnlockBaseAddress(buffer, [])
    write(packet(for: buffer))
    RunLoop.current.run()
    Darwin._exit(0)
}

guard mode == "list" || mode == "capture" else { fail("Modo de webcam inválido.") }
let authorization = AVCaptureDevice.authorizationStatus(for: .video)
if authorization == .notDetermined {
    let done = DispatchSemaphore(value: 0)
    AVCaptureDevice.requestAccess(for: .video) { _ in done.signal() }
    guard done.wait(timeout: .now() + 55) == .success else {
        fail("Confirme a permissão de câmera e tente novamente.")
    }
}
guard AVCaptureDevice.authorizationStatus(for: .video) == .authorized else {
    fail("Permita o acesso à câmera nas configurações de privacidade do macOS e tente novamente.")
}

let devices = AVCaptureDevice.DiscoverySession(
    deviceTypes: [.builtInWideAngleCamera, .external, .continuityCamera],
    mediaType: .video, position: .unspecified
).devices

if mode == "list" {
    let list: [[String: Any]] = devices.prefix(64).map {
        ["name": $0.localizedName, "id": $0.uniqueID, "index": NSNull()]
    }
    write(try JSONSerialization.data(withJSONObject: ["Ok": list]))
    Darwin._exit(0)
}

guard mode == "capture", CommandLine.arguments.count == 3,
      let data = CommandLine.arguments[2].data(using: .utf8),
      let choice = try JSONSerialization.jsonObject(with: data) as? [String: Any],
      let id = choice["id"] as? String,
      let device = devices.first(where: { $0.uniqueID == id }) else {
    fail("A webcam não está mais disponível. Atualize a lista de câmeras.")
}

final class Frames: NSObject, AVCaptureVideoDataOutputSampleBufferDelegate {
    private var lastFrame = -Double.infinity
    private var logged = false

    func captureOutput(_ output: AVCaptureOutput, didOutput sample: CMSampleBuffer,
                       from connection: AVCaptureConnection) {
        let now = ProcessInfo.processInfo.systemUptime
        guard now - lastFrame >= 1.0 / 30.0,
              let buffer = CMSampleBufferGetImageBuffer(sample) else { return }
        lastFrame = now
        let width = CVPixelBufferGetWidth(buffer), height = CVPixelBufferGetHeight(buffer)
        write(packet(for: buffer))
        if !logged {
            logged = true
            fputs("telinha webcam: recebendo \(width)x\(height) BGRA pelo AVFoundation\n", stderr)
        }
    }
}

let session = AVCaptureSession()
session.beginConfiguration()
let input: AVCaptureDeviceInput
do { input = try AVCaptureDeviceInput(device: device) }
catch { fail("Não deu pra abrir a webcam. Confira se outro aplicativo está usando a câmera.") }
guard session.canAddInput(input) else { fail("O macOS não conseguiu conectar esta webcam.") }
session.addInput(input)

// AVFoundation negotiates supported device formats and frame-rate ranges.
if session.canSetSessionPreset(.hd1280x720) { session.sessionPreset = .hd1280x720 }
else if session.canSetSessionPreset(.vga640x480) { session.sessionPreset = .vga640x480 }
else if session.canSetSessionPreset(.high) { session.sessionPreset = .high }

let video = AVCaptureVideoDataOutput()
video.alwaysDiscardsLateVideoFrames = true
video.videoSettings = [kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA]
let frames = Frames()
video.setSampleBufferDelegate(frames, queue: DispatchQueue(label: "tech.telinha.camera.frames"))
guard session.canAddOutput(video) else { fail("Não deu pra receber vídeo desta webcam.") }
session.addOutput(video)
session.commitConfiguration()

let runtimeError = NotificationCenter.default.addObserver(
    forName: AVCaptureSession.runtimeErrorNotification, object: session, queue: nil
) { notification in
    if let error = notification.userInfo?[AVCaptureSessionErrorKey] as? Error {
        fputs("telinha webcam: \(error.localizedDescription)\n", stderr)
    }
    fail("O macOS interrompeu a webcam. Confira a conexão e tente novamente.")
}
let interruption = NotificationCenter.default.addObserver(
    forName: AVCaptureSession.wasInterruptedNotification, object: session, queue: nil
) { _ in fail("A webcam foi interrompida. Confira se outro aplicativo está usando a câmera.") }

session.startRunning()
guard session.isRunning else { fail("A webcam não conseguiu iniciar o vídeo.") }
RunLoop.current.run()
