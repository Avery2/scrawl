// scrawl-helper: the parts of Scrawl that need Swift-only Apple frameworks.
//
//   scrawl-helper devices [--timeout S]                 cabled iPhones/iPads as JSON
//   scrawl-helper mirror --out FILE.mov [--device ID]   live mirror window + native recording (JSON lines)
//   scrawl-helper transcribe FILE                       SpeechAnalyzer transcript with word timings
//   scrawl-helper frames MOVIE --out-dir DIR (--every MS | --at MS,MS…) [--max-edge PX]
//   scrawl-helper audio MOVIE OUT.wav                   16 kHz mono PCM of every audio track, mixed
//   scrawl-helper probe MOVIE                           duration and video size
//
// Every timestamp in the output is integer milliseconds. Scrawl drives the long-running
// `mirror` command over stdin (`stop`, `cancel`) and reads its events from stdout.

import AppKit
import Foundation

func emit(_ object: Any) {
  guard let data = try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys, .withoutEscapingSlashes]) else { return }
  FileHandle.standardOutput.write(data + Data("\n".utf8))
}

func fail(_ message: String) -> Never {
  FileHandle.standardError.write(Data("scrawl-helper: \(message)\n".utf8))
  exit(1)
}

struct HelperError: LocalizedError {
  let errorDescription: String?
  init(_ message: String) { errorDescription = message }
}

func option(_ name: String, in arguments: [String]) -> String? {
  guard let index = arguments.firstIndex(of: name), index + 1 < arguments.count else { return nil }
  return arguments[index + 1]
}

/// Runs async work from a synchronous entry point, keeping the main queue free for AVFoundation.
func runAsync(_ body: @escaping () async throws -> Void) -> Never {
  Task {
    do {
      try await body()
      exit(0)
    } catch {
      fail(error.localizedDescription)
    }
  }
  dispatchMain()
}

let arguments = Array(CommandLine.arguments.dropFirst())
switch arguments.first {
case "devices":
  let timeout = option("--timeout", in: arguments).flatMap(Double.init) ?? 2.5
  emit(Phone.waitForDevices(timeout: timeout).map(Phone.describe))
case "mirror":
  guard let out = option("--out", in: arguments) else { fail("mirror needs --out FILE.mov") }
  Mirror.run(output: URL(filePath: out), deviceID: option("--device", in: arguments))
case "transcribe":
  guard arguments.count >= 2 else { fail("transcribe needs a file") }
  runAsync { emit(try await Transcriber.transcribe(URL(filePath: arguments[1]))) }
case "frames":
  guard arguments.count >= 2, let dir = option("--out-dir", in: arguments) else {
    fail("frames needs MOVIE --out-dir DIR (--every MS | --at MS,MS…)")
  }
  let maxEdge = option("--max-edge", in: arguments).flatMap(Int.init) ?? 0
  let every = option("--every", in: arguments).flatMap(Int.init)
  let at = option("--at", in: arguments)?.split(separator: ",").compactMap { Int($0) }
  runAsync {
    emit(try await Media.frames(movie: URL(filePath: arguments[1]), outDir: URL(filePath: dir), everyMs: every, atMs: at, maxEdge: maxEdge))
  }
case "audio":
  guard arguments.count >= 3 else { fail("audio needs MOVIE OUT.wav") }
  runAsync { emit(try await Media.exportAudio(movie: URL(filePath: arguments[1]), wav: URL(filePath: arguments[2]))) }
case "probe":
  guard arguments.count >= 2 else { fail("probe needs MOVIE") }
  runAsync { emit(try await Media.probe(URL(filePath: arguments[1]))) }
default:
  fail("usage: scrawl-helper devices|mirror|transcribe|frames|audio|probe …")
}
