@preconcurrency import AVFoundation
import CoreGraphics
import Speech

/// On-device transcription with macOS 26's SpeechAnalyzer: finalized results only, each word with
/// its own audio time range.
enum Transcriber {
  static func transcribe(_ url: URL) async throws -> [String: Any] {
    var found = await SpeechTranscriber.supportedLocale(equivalentTo: Locale.current)
    if found == nil { found = await SpeechTranscriber.supportedLocale(equivalentTo: Locale(identifier: "en-US")) }
    guard let locale = found else { throw HelperError("SpeechTranscriber has no supported locale") }
    let transcriber = SpeechTranscriber(locale: locale, transcriptionOptions: [], reportingOptions: [], attributeOptions: [.audioTimeRange])
    if let request = try await AssetInventory.assetInstallationRequest(supporting: [transcriber]) {
      try await request.downloadAndInstall()
    }
    let file = try AVAudioFile(forReading: url)
    let analyzer = try await SpeechAnalyzer(inputAudioFile: file, modules: [transcriber], finishAfterFile: true)
    var segments: [[String: Any]] = []
    var words: [[String: Any]] = []
    for try await result in transcriber.results {
      let text = String(result.text.characters).trimmingCharacters(in: .whitespacesAndNewlines)
      guard !text.isEmpty else { continue }
      for run in result.text.runs {
        guard let range = run.audioTimeRange else { continue }
        let word = String(result.text[run.range].characters).trimmingCharacters(in: .whitespacesAndNewlines)
        guard !word.isEmpty else { continue }
        words.append(["t0": ms(range.start), "t1": ms(range.end), "text": word])
      }
      segments.append(["t0": ms(result.range.start), "t1": ms(result.range.end), "text": text])
    }
    withExtendedLifetime(analyzer) {}
    return [
      "locale": locale.identifier,
      "text": segments.compactMap { $0["text"] as? String }.joined(separator: " "),
      "segments": segments,
      "words": words,
    ]
  }

  private static func ms(_ time: CMTime) -> Int {
    time.seconds.isFinite ? max(0, Int((time.seconds * 1000).rounded())) : 0
  }
}

enum Media {
  static func probe(_ movie: URL) async throws -> [String: Any] {
    let asset = AVURLAsset(url: movie)
    let duration = try await asset.load(.duration).seconds
    var result: [String: Any] = ["durationMs": Int(((duration.isFinite ? duration : 0) * 1000).rounded())]
    if let track = try await asset.loadTracks(withMediaType: .video).first {
      let (size, transform) = try await track.load(.naturalSize, .preferredTransform)
      let shown = size.applying(transform)
      result["width"] = Int(abs(shown.width).rounded())
      result["height"] = Int(abs(shown.height).rounded())
    }
    result["audioTracks"] = try await asset.loadTracks(withMediaType: .audio).count
    return result
  }

  /// Decodes frames as raw RGBA8 (`frame-NNNN.rgba`), either sampled every `everyMs` (nearest
  /// decodable frame, for keyframe candidates) or exactly at `atMs` (for the keyframes themselves).
  static func frames(movie: URL, outDir: URL, everyMs: Int?, atMs: [Int]?, maxEdge: Int) async throws -> [[String: Any]] {
    let asset = AVURLAsset(url: movie)
    let duration = try await asset.load(.duration).seconds
    let durationMs = Int((duration.isFinite ? duration : 0) * 1000)
    guard durationMs > 0 else { throw HelperError("Movie has no duration") }
    try FileManager.default.createDirectory(at: outDir, withIntermediateDirectories: true)

    let generator = AVAssetImageGenerator(asset: asset)
    generator.appliesPreferredTrackTransform = true
    if maxEdge > 0 { generator.maximumSize = CGSize(width: maxEdge, height: maxEdge) }
    var requested: [Int]
    if let everyMs, everyMs > 0 {
      requested = Array(stride(from: 0, to: durationMs, by: everyMs))
      let tolerance = CMTime(value: CMTimeValue(everyMs / 2), timescale: 1000)
      generator.requestedTimeToleranceBefore = tolerance
      generator.requestedTimeToleranceAfter = tolerance
    } else {
      requested = (atMs ?? []).map { min(max(0, $0), max(0, durationMs - 1)) }
      generator.requestedTimeToleranceBefore = .zero
      generator.requestedTimeToleranceAfter = .zero
    }
    guard !requested.isEmpty else { return [] }

    var out: [[String: Any]] = []
    let times = requested.map { CMTime(value: CMTimeValue($0), timescale: 1000) }
    var index = 0
    for await result in generator.images(for: times) {
      defer { index += 1 }
      guard let image = try? result.image else { continue }
      let actual = (try? result.actualTime).map { Int(($0.seconds * 1000).rounded()) } ?? requested[min(index, requested.count - 1)]
      let file = outDir.appending(path: String(format: "frame-%04d.rgba", index))
      try writeRGBA(image, to: file)
      out.append(["t": requested[min(index, requested.count - 1)], "actual": actual, "width": image.width, "height": image.height, "file": file.path])
    }
    return out
  }

  private static func writeRGBA(_ image: CGImage, to url: URL) throws {
    let width = image.width, height = image.height
    var pixels = Data(count: width * height * 4)
    let drawn = pixels.withUnsafeMutableBytes { buffer -> Bool in
      guard let context = CGContext(data: buffer.baseAddress, width: width, height: height, bitsPerComponent: 8, bytesPerRow: width * 4,
                                    space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
      else { return false }
      context.interpolationQuality = .high
      context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
      return true
    }
    guard drawn else { throw HelperError("Could not convert frame") }
    try pixels.write(to: url)
  }

  /// Every audio track mixed to 16 kHz mono 16-bit PCM, padded so sample 0 is the movie's start.
  static func exportAudio(movie: URL, wav: URL) async throws -> [String: Any] {
    let asset = AVURLAsset(url: movie)
    let tracks = try await asset.loadTracks(withMediaType: .audio)
    guard !tracks.isEmpty else { throw HelperError("Movie has no audio") }
    let reader = try AVAssetReader(asset: asset)
    let output = AVAssetReaderAudioMixOutput(audioTracks: tracks, audioSettings: [
      AVFormatIDKey: kAudioFormatLinearPCM,
      AVSampleRateKey: 16000,
      AVNumberOfChannelsKey: 1,
      AVLinearPCMBitDepthKey: 16,
      AVLinearPCMIsFloatKey: false,
      AVLinearPCMIsBigEndianKey: false,
      AVLinearPCMIsNonInterleaved: false,
    ])
    reader.add(output)
    guard reader.startReading() else { throw HelperError(reader.error?.localizedDescription ?? "Could not read audio") }
    var pcm = Data()
    var first = true
    while let buffer = output.copyNextSampleBuffer() {
      if first {
        first = false
        let start = CMSampleBufferGetPresentationTimeStamp(buffer).seconds
        if start.isFinite, start > 0 { pcm.append(Data(count: Int(start * 16000) * 2)) }
      }
      guard let block = CMSampleBufferGetDataBuffer(buffer) else { continue }
      let length = CMBlockBufferGetDataLength(block)
      var chunk = Data(count: length)
      chunk.withUnsafeMutableBytes { _ = CMBlockBufferCopyDataBytes(block, atOffset: 0, dataLength: length, destination: $0.baseAddress!) }
      pcm.append(chunk)
    }
    if reader.status == .failed { throw HelperError(reader.error?.localizedDescription ?? "Audio export failed") }
    var header = Data()
    func u32(_ v: UInt32) { withUnsafeBytes(of: v.littleEndian) { header.append(contentsOf: $0) } }
    func u16(_ v: UInt16) { withUnsafeBytes(of: v.littleEndian) { header.append(contentsOf: $0) } }
    header.append(contentsOf: Array("RIFF".utf8)); u32(UInt32(36 + pcm.count))
    header.append(contentsOf: Array("WAVE".utf8))
    header.append(contentsOf: Array("fmt ".utf8)); u32(16); u16(1); u16(1); u32(16000); u32(32000); u16(2); u16(16)
    header.append(contentsOf: Array("data".utf8)); u32(UInt32(pcm.count))
    try (header + pcm).write(to: wav)
    return ["path": wav.path, "samples": pcm.count / 2, "durationMs": pcm.count / 32]
  }
}
