import SwiftUI
import YeTrackerKit

/// Server URL (the web's `PUBLIC_API_URL`), playback defaults, cache and about.
struct SettingsView: View {
  @Environment(AppModel.self) private var app
  @State private var serverText = ""
  @State private var serverError: String?
  @State private var connection: ConnectionState = .idle
  @State private var cacheSize = 0
  @FocusState private var serverFieldFocused: Bool

  private enum ConnectionState: Equatable {
    case idle
    case testing
    case success(String)
    case failure(String)
  }

  var body: some View {
    let settings = app.settings
    Form {
      Section {
        TextField("https://example.com/api", text: $serverText)
          .keyboardType(.URL)
          .textContentType(.URL)
          .textInputAutocapitalization(.never)
          .autocorrectionDisabled()
          .submitLabel(.done)
          .focused($serverFieldFocused)
          .onSubmit { saveServer() }
        if let serverError {
          Text(serverError)
            .font(.footnote)
            .foregroundStyle(Theme.error)
        }
        Button("Save and Reload") { saveServer() }
          .disabled(normalizedInput == settings.apiBaseURL.absoluteString)
        Button {
          Task { await testConnection() }
        } label: {
          HStack {
            Text("Test Connection")
            Spacer()
            connectionStatus
          }
        }
        .disabled(connection == .testing)
        if settings.customAPIBaseURL != nil {
          Button("Use Default (\(settings.defaultAPIBaseURL.absoluteString))", role: .destructive) {
            app.resetServer()
            serverText = app.settings.apiBaseURL.absoluteString
            serverError = nil
            connection = .idle
          }
        }
      } header: {
        Text("Server")
      } footer: {
        Text(
          "The YeTracker API base URL. A path prefix such as /api is kept. iOS only allows plain http:// for localhost and local network names."
        )
      }

      Section("Playback") {
        Picker(
          "Default quality",
          selection: Binding(get: { app.player.quality }, set: { app.player.setQuality($0) })
        ) {
          ForEach(PlaybackQuality.allCases) { quality in
            Text(quality.label).tag(quality)
          }
        }
        LabeledContent("Volume") {
          Slider(
            value: Binding(get: { app.player.volume }, set: { app.player.volume = $0 }),
            in: 0...1
          )
          .frame(maxWidth: 200)
        }
      }

      Section {
        LabeledContent(
          "Cover cache", value: ByteCountFormatter.string(fromByteCount: Int64(cacheSize), countStyle: .file))
        Button("Clear Cover Cache", role: .destructive) {
          ImagePipeline.shared.removeAll()
          cacheSize = ImagePipeline.shared.diskUsage
        }
      } header: {
        Text("Storage")
      }

      Section("About") {
        LabeledContent("Version", value: Self.version)
        Link("Ye Tracker", destination: URL(string: "https://yetracker.net")!)
      }
    }
    .navigationTitle("Settings")
    .onAppear {
      if serverText.isEmpty { serverText = settings.apiBaseURL.absoluteString }
      cacheSize = ImagePipeline.shared.diskUsage
    }
  }

  @ViewBuilder
  private var connectionStatus: some View {
    switch connection {
    case .idle:
      EmptyView()
    case .testing:
      ProgressView()
    case .success(let summary):
      Label(summary, systemImage: "checkmark.circle.fill")
        .labelStyle(.titleAndIcon)
        .font(.footnote)
        .foregroundStyle(.green)
    case .failure(let message):
      Label(message, systemImage: "xmark.octagon.fill")
        .font(.footnote)
        .foregroundStyle(Theme.error)
        .lineLimit(3)
    }
  }

  /// The field's value as it would be stored.
  private var normalizedInput: String {
    APIBaseURL.parse(serverText)?.absoluteString ?? serverText
  }

  private func saveServer() {
    serverFieldFocused = false
    do {
      try app.updateServer(serverText)
      serverText = app.settings.apiBaseURL.absoluteString
      serverError = nil
      connection = .idle
    } catch {
      serverError = "Enter an http:// or https:// address, e.g. https://example.com/api."
    }
  }

  private func testConnection() async {
    connection = .testing
    // Test what is typed, before saving it.
    guard let url = APIBaseURL.parse(serverText) else {
      connection = .failure("Not a valid http(s) address.")
      return
    }
    let client = APIClient(baseURL: url)
    do {
      try await client.health()
      let eras = try await client.eras(reload: true)
      connection = .success("\(eras.count) eras")
    } catch {
      connection = .failure(APIError(transportError: error).userMessage)
    }
  }

  private static var version: String {
    let info = Bundle.main.infoDictionary
    let version = info?["CFBundleShortVersionString"] as? String ?? "?"
    let build = info?["CFBundleVersion"] as? String ?? "?"
    return "\(version) (\(build))"
  }
}
