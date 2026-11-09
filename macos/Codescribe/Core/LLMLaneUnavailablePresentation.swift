import Foundation

/// The one sentence for a request lane that cannot reach a model. Rust sends
/// the reason and its facts (`CsLlmLaneUnavailable`); the wording, and its
/// translation, live here. Provider names, endpoints and Keychain account names
/// are shown verbatim. Settings › Providers and the Agent chat both read this.
extension CsLlmLaneUnavailable {
  var userFacingSentence: String {
    switch self {
    case .customProviderMissing(let customId):
      return String(
        localized: "Custom provider “\(customId)” no longer exists.",
        comment: "Request lane unavailable. The placeholder is the removed custom provider's id")
    case .modelMissing(let provider):
      return String(
        localized: "No model selected for provider \(provider).",
        comment: "Request lane unavailable. The placeholder is a provider name, e.g. OpenAI")
    case .credentialMissing(let lane, let provider, let endpoint, let keyAccount):
      return String(
        localized:
          "The \(Self.laneTitle(lane)) lane points at \(provider) (\(endpoint)), which requires a credential, but neither Keychain account \(keyAccount) nor a supported signed-in provider account is available.",
        comment:
          "Request lane unavailable. Placeholders: lane name (Formatting or Assistive), provider name, its endpoint URL, Keychain account name such as LLM_OPENAI_API_KEY"
      )
    case .cloudCodeAssistAccountRequired(let lane):
      return String(
        localized:
          "The \(Self.laneTitle(lane)) lane uses Cloud Code Assist, which requires locally provisioned Google account credentials or authorized-user ADC. A public Gemini API key cannot authenticate this endpoint.",
        comment: "Request lane unavailable. The placeholder is the lane name (Formatting or Assistive)"
      )
    case .acpLaunchInvalid:
      return String(
        localized:
          "ACP requires absolute program/cwd paths and literal argv without NUL or traversal.",
        comment: "Agent lane unavailable: the ACP launch command in settings is invalid")
    case .acpWorkspaceRootsMissing:
      return String(
        localized:
          "ACP requires explicit absolute system.agent_workspace_roots; home expansion and process cwd are not implicit grants.",
        comment: "Agent lane unavailable: ACP needs workspace roots in settings")
    case .acpCwdOutsideWorkspace:
      return String(
        localized: "ACP cwd must be inside a configured Agent workspace root.",
        comment: "Agent lane unavailable: the ACP working directory is outside the workspace roots")
    }
  }

  private static func laneTitle(_ lane: CsLlmLane) -> String {
    (lane == .assistive ? LLMLane.assistive : LLMLane.formatting).title
  }
}
