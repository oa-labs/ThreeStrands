import AVFoundation
import CoreMedia
import Foundation
import Speech

@_cdecl("threestrands_speech_analyzer_available")
public func threestrandsSpeechAnalyzerAvailable() -> Bool {
    if #available(macOS 26.0, *) {
        return SpeechTranscriber.isAvailable
    }
    return false
}

@_cdecl("threestrands_speech_analyzer_free_str")
public func threestrandsSpeechAnalyzerFreeStr(_ ptr: UnsafeMutablePointer<CChar>?) {
    if let ptr {
        free(ptr)
    }
}

private func dupCString(_ s: String) -> UnsafeMutablePointer<CChar>? {
    strdup(s)
}

@available(macOS 26.0, *)
private struct SegmentOut: Encodable {
    let startMs: UInt64
    let endMs: UInt64
    let text: String
}

@available(macOS 26.0, *)
private func transcribeAsync(path: String, language: String?, prompt: String?) async throws -> String {
    guard SpeechTranscriber.isAvailable else {
        throw NSError(
            domain: "ThreeStrandsSpeech",
            code: 1,
            userInfo: [NSLocalizedDescriptionKey: "SpeechAnalyzer isn't available on this Mac."]
        )
    }

    let requested = language.flatMap { $0.isEmpty ? nil : Locale(identifier: $0) } ?? Locale.current
    var resolved = await SpeechTranscriber.supportedLocale(equivalentTo: requested)
    if resolved == nil {
        resolved = await SpeechTranscriber.supportedLocale(equivalentTo: Locale(identifier: "en-US"))
    }
    guard let locale = resolved else {
        throw NSError(
            domain: "ThreeStrandsSpeech",
            code: 2,
            userInfo: [NSLocalizedDescriptionKey: "No supported on-device speech locale is available."]
        )
    }

    let transcriber = SpeechTranscriber(
        locale: locale,
        transcriptionOptions: [],
        reportingOptions: [],
        attributeOptions: [.audioTimeRange]
    )

    let status = await AssetInventory.status(forModules: [transcriber])
    if status < .installed {
        if let request = try await AssetInventory.assetInstallationRequest(supporting: [transcriber]) {
            try await request.downloadAndInstall()
        }
    }

    let context = AnalysisContext()
    if let prompt, !prompt.isEmpty {
        let terms = prompt
            .split(separator: ",")
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty }
        if !terms.isEmpty {
            context.contextualStrings[.general] = terms
        }
    }

    let url = URL(fileURLWithPath: path)
    let audioFile = try AVAudioFile(forReading: url)

    async let collected: [SegmentOut] = {
        var out: [SegmentOut] = []
        for try await result in transcriber.results {
            let text = String(result.text.characters).trimmingCharacters(in: .whitespacesAndNewlines)
            if text.isEmpty {
                continue
            }
            let startSec = max(0.0, CMTimeGetSeconds(result.range.start))
            let endSec = max(startSec, CMTimeGetSeconds(CMTimeRangeGetEnd(result.range)))
            let startMs = startSec.isFinite ? UInt64((startSec * 1000.0).rounded()) : 0
            let endMs = endSec.isFinite ? UInt64((endSec * 1000.0).rounded()) : startMs
            out.append(SegmentOut(startMs: startMs, endMs: max(startMs, endMs), text: text))
        }
        return out
    }()

    let analyzer = try await SpeechAnalyzer(
        inputAudioFile: audioFile,
        modules: [transcriber],
        options: nil,
        analysisContext: context,
        finishAfterFile: true
    )
    _ = analyzer
    let items = try await collected
    let data = try JSONEncoder().encode(items)
    return String(data: data, encoding: .utf8) ?? "[]"
}

@_cdecl("threestrands_speech_analyzer_transcribe_file")
public func threestrandsSpeechAnalyzerTranscribeFile(
    _ pathPtr: UnsafePointer<CChar>,
    _ langPtr: UnsafePointer<CChar>?,
    _ promptPtr: UnsafePointer<CChar>?,
    _ outJson: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>,
    _ outErr: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>
) -> Int32 {
    outJson.pointee = nil
    outErr.pointee = nil

    guard #available(macOS 26.0, *) else {
        outErr.pointee = dupCString("Apple SpeechAnalyzer requires macOS 26 or later.")
        return 1
    }

    let path = String(cString: pathPtr)
    let language = langPtr.map { String(cString: $0) }
    let prompt = promptPtr.map { String(cString: $0) }

    final class ResultBox: @unchecked Sendable {
        var json: String?
        var err: String?
    }

    let semaphore = DispatchSemaphore(value: 0)
    let box = ResultBox()

    Task.detached {
        do {
            box.json = try await transcribeAsync(path: path, language: language, prompt: prompt)
        } catch {
            box.err = error.localizedDescription
        }
        semaphore.signal()
    }
    semaphore.wait()

    if let jsonResult = box.json {
        outJson.pointee = dupCString(jsonResult)
        return 0
    }
    outErr.pointee = dupCString(box.err ?? "Apple SpeechAnalyzer failed.")
    return 1
}
