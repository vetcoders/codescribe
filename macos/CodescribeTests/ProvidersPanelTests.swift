import SwiftUI
import XCTest

@testable import Codescribe

/// Witnesses for provider-registry-v2 §F.7 (Swift side). Effects, not names:
/// what the lane picker lists after Add, what Remove does to a lane bound to
/// the removed row, and that no view-model path can write a vendor endpoint.
@MainActor
final class ProvidersPanelTests: XCTestCase {
  private static let localDraft = CsCustomProviderDraft(
    name: "My Local",
    wire: "responses",
    endpoint: "http://localhost:8080/v1/responses",
    apiKey: nil
  )

  /// Model over the mock bridge: custom rows and lane bindings live in `store`,
  /// and the sealed runtime lane is projected from that same store.
  private func makeModel(
    store: MockProviderStore = MockProviderStore(),
    configWrites: (((String, String)) -> Void)? = nil
  ) -> SettingsViewModel {
    let engine = MockSettingsEngine(
      providerStore: store,
      updateConfigObserver: { configWrites?(($0, $1)) }
    )
    return SettingsViewModel(engine: engine, runtimeLlmLaneProvider: { store.runtimeLane($0) })
  }

  func testAddCustomProviderAppearsInLanePicker() throws {
    let model = makeModel()
    XCTAssertEqual(model.customProviders.map(\.id), [])

    try model.addCustomProvider(Self.localDraft)

    // The lane editor's Provider menu iterates `providers`: vendors first,
    // then custom rows in insertion order.
    XCTAssertEqual(model.providers.map(\.id).last, "custom:my-local")
    let custom = try XCTUnwrap(model.customProviders.first)
    XCTAssertEqual(custom.kind, "custom")
    XCTAssertEqual(custom.displayName, "My Local")
    XCTAssertEqual(custom.apiKeyAccount, "LLM_CUSTOM_MY_LOCAL_API_KEY")
    XCTAssertFalse(custom.keyRequired, "custom hosts are key-optional")

    model.setLaneProvider("custom:my-local", for: .formatting)
    let lane = model.llmLane(.formatting)
    XCTAssertEqual(lane.providerId, "custom:my-local")
    XCTAssertEqual(lane.providerDisplayName, "My Local")
    XCTAssertEqual(lane.resolvedEndpoint, "http://localhost:8080/v1/responses")
    XCTAssertTrue(lane.runtime.available, "no key required → available without a key")
  }

  func testRemoveCustomProviderInUseResetsLane() throws {
    let store = MockProviderStore()
    let model = makeModel(store: store)
    try model.addCustomProvider(Self.localDraft)
    model.setLaneProvider("custom:my-local", for: .assistive)
    XCTAssertEqual(model.llmLane(.assistive).providerId, "custom:my-local")
    XCTAssertNil(model.laneResetNotice)

    model.removeCustomProvider(id: "custom:my-local")

    XCTAssertEqual(model.customProviders.map(\.id), [])
    XCTAssertEqual(
      model.llmLane(.assistive).providerId, "openai-responses",
      "the bound lane falls back to the default vendor")
    XCTAssertEqual(model.llmLane(.formatting).providerId, "openai-responses")
    XCTAssertEqual(
      model.laneResetNotice,
      "Assistive lane reset to the default vendor — the custom provider was removed"
    )
    XCTAssertNil(model.lastError)

    model.setLaneProvider("xai-responses", for: .assistive)
    XCTAssertNil(model.laneResetNotice, "the next lane edit clears the note")
  }

  func testVendorEndpointHasNoSetter() throws {
    var writes: [(key: String, value: String)] = []
    let model = makeModel { writes.append((key: $0.0, value: $0.1)) }

    // Vendor set is the contract (§B.3 adds Libraxis); ORDER is the registry's
    // and the UI must not re-sort it — first place is I1's call, not this test's.
    XCTAssertEqual(
      Set(model.vendorProviders.map(\.id)),
      ["libraxis-responses", "openai-responses", "xai-responses", "anthropic-messages"]
    )
    XCTAssertEqual(
      model.vendorProviders.map(\.id),
      CsProviderOption.sampleProviders.map(\.id),
      "the picker keeps availableProviders() order verbatim")
    for vendor in model.vendorProviders {
      XCTAssertEqual(vendor.kind, "vendor")
      XCTAssertTrue(vendor.keyRequired)
      model.setLaneProvider(vendor.id, for: .assistive)
      model.setLLMModel("model-x", for: .assistive)
      XCTAssertEqual(
        model.llmLane(.assistive).resolvedEndpoint, vendor.endpoint,
        "the lane's endpoint IS the vendor's factory endpoint")
    }

    try model.addCustomProvider(Self.localDraft)
    try model.updateCustomProvider(
      id: "custom:my-local",
      CsCustomProviderDraft(
        name: "My Local", wire: "messages", endpoint: "http://localhost:9090/v1/messages",
        apiKey: nil)
    )
    XCTAssertEqual(model.customProviders.first?.endpoint, "http://localhost:9090/v1/messages")
    model.removeCustomProvider(id: "custom:my-local")

    XCTAssertFalse(
      writes.contains { $0.key.contains("ENDPOINT") },
      "no view-model path writes an endpoint config key: \(writes)")
    XCTAssertEqual(Set(writes.map(\.key)), ["LLM_ASSISTIVE_MODEL"])
  }

  func testCustomProviderFormSurfacesBridgeValidationError() {
    let model = makeModel()

    XCTAssertThrowsError(
      try model.addCustomProvider(
        CsCustomProviderDraft(name: "   ", wire: "responses", endpoint: "https://x", apiKey: nil))
    ) { error in
      XCTAssertEqual(error as? MockProviderStore.Failure, .emptyName)
    }
    XCTAssertThrowsError(
      try model.addCustomProvider(
        CsCustomProviderDraft(
          name: "Relay", wire: "messages", endpoint: "relay.local", apiKey: nil))
    ) { error in
      XCTAssertEqual(error as? MockProviderStore.Failure, .invalidEndpoint("relay.local"))
    }

    XCTAssertNil(model.lastError, "the form owns the message; no modal error")
    XCTAssertEqual(model.customProviders.count, 0)
  }

  func testKeyStatusMapsContractAccountsOnly() {
    let status = CsKeyStatus(
      llmLibraxisApiKeySet: true,
      llmOpenaiApiKeySet: false,
      llmXaiApiKeySet: true,
      llmAnthropicApiKeySet: false,
      sttFileApiKeySet: true,
      sttLiveApiKeySet: false,
      githubTokenSet: false
    )
    XCTAssertTrue(status.isSet(account: "LLM_LIBRAXIS_API_KEY"))
    XCTAssertFalse(status.isSet(account: "LLM_OPENAI_API_KEY"))
    XCTAssertTrue(status.isSet(account: "LLM_XAI_API_KEY"))
    XCTAssertFalse(status.isSet(account: "LLM_ANTHROPIC_API_KEY"))
    XCTAssertTrue(status.isSet(account: "STT_FILE_API_KEY"))
    XCTAssertFalse(status.isSet(account: "STT_LIVE_API_KEY"))
    XCTAssertFalse(status.isSet(account: "GITHUB_TOKEN"))
    for legacy in [
      "LLM_API_KEY", "LLM_ASSISTIVE_API_KEY", "LLM_FORMATTING_API_KEY", "STT_API_KEY",
    ] {
      XCTAssertFalse(
        status.isSet(account: legacy), "\(legacy): retired account (D3 / stt-lanes-v1)")
    }
    XCTAssertEqual(makeModel().serviceKeyAccounts, ["GITHUB_TOKEN"])
    XCTAssertEqual(SettingsViewModel.keyLabel(for: "LLM_LIBRAXIS_API_KEY"), "Libraxis API key")
    XCTAssertEqual(SettingsViewModel.keyLabel(for: "STT_FILE_API_KEY"), "File transcription key")
    XCTAssertEqual(SettingsViewModel.keyLabel(for: "STT_LIVE_API_KEY"), "Live transcript key")
  }

  /// stt-lanes-v1 §F.7: Providers renders File then Live, each lane one atomic
  /// endpoint + key row, and a lane save writes ONLY that lane's wire key.
  func testSpeechToTextSectionRendersFileThenLiveAsAtomicRows() {
    var writes: [(key: String, value: String)] = []
    let model = makeModel { writes.append((key: $0.0, value: $0.1)) }
    _ = SpeechToTextSection(model: model)

    XCTAssertEqual(
      model.sttLanes.map(\.id), ["file", "live"], "sttLanes() order is the contract's")
    XCTAssertEqual(
      model.sttLanes.map(\.endpointWireKey), ["STT_FILE_ENDPOINT", "STT_LIVE_ENDPOINT"])
    XCTAssertEqual(model.sttLanes.map(\.keyAccount), ["STT_FILE_API_KEY", "STT_LIVE_API_KEY"])
    XCTAssertEqual(model.sttLanes.map(\.title), ["File transcription", "Live transcript"])
    XCTAssertFalse(
      model.serviceKeyAccounts.contains { $0.hasPrefix("STT_") },
      "STT keys ride on the lanes, never on Service keys")

    model.setSttLaneEndpoint("file", " https://asr.example/v1/audio/transcriptions ")
    XCTAssertEqual(writes.last?.key, "STT_FILE_ENDPOINT")
    XCTAssertEqual(writes.last?.value, "https://asr.example/v1/audio/transcriptions")
    model.setSttLaneEndpoint("live", "wss://asr.example/v1/audio/transcribe")
    XCTAssertEqual(writes.last?.key, "STT_LIVE_ENDPOINT")
    XCTAssertEqual(writes.map(\.key), ["STT_FILE_ENDPOINT", "STT_LIVE_ENDPOINT"])

    model.setSttLaneEndpoint("ndjson", "https://x")
    XCTAssertEqual(writes.count, 2, "an unknown lane id writes nothing (two lanes, not three)")
    XCTAssertNil(model.lastError)
  }

  /// LLMLaneEditor.body reads `llmLane` per menu item; the FFI loader must not.
  func testLlmLaneHitsRuntimeProviderOncePerLanePerRefresh() {
    var assistiveHits = 0
    var formattingHits = 0
    let store = MockProviderStore()
    let model = SettingsViewModel(
      engine: MockSettingsEngine(providerStore: store),
      runtimeLlmLaneProvider: { lane in
        if lane == .assistive {
          assistiveHits += 1
        } else {
          formattingHits += 1
        }
        return store.runtimeLane(lane)
      }
    )

    for _ in 0..<25 {
      for lane in LLMLane.allCases {
        let projected = model.llmLane(lane)
        _ = projected.providerId
        _ = projected.resolvedModel
        _ = projected.providerDisplayName
      }
    }
    XCTAssertEqual(assistiveHits, 1)
    XCTAssertEqual(formattingHits, 1)

    model.setFormattingEnabled(true)
    for _ in 0..<10 {
      for lane in LLMLane.allCases {
        _ = model.llmLane(lane)
      }
    }
    XCTAssertEqual(assistiveHits, 2)
    XCTAssertEqual(formattingHits, 2)
  }
}
