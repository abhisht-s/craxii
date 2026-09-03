import SwiftUI
import CraxiiProtocol

struct SetupView: View {
    @Bindable var store: ConversationStore

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            VStack(alignment: .leading, spacing: 6) {
                Text("Connect to Craxii")
                    .font(.largeTitle.weight(.semibold))
                Text("Enter the endpoint and provisioned device credential.")
                    .foregroundStyle(.secondary)
            }
            Form {
                TextField("Endpoint", text: $store.endpoint)
                    .accessibilityIdentifier("setup.endpoint")
                SecureField("Device credential", text: $store.credentialInput)
                    .accessibilityIdentifier("setup.credential")
            }
            .formStyle(.grouped)
            if let error = store.presentation.error {
                SafeErrorView(error: error, dismiss: store.dismissError)
            }
            HStack {
                Button("Apply Endpoint") { Task { await store.applyEndpoint() } }
                Button("Save Credential") { Task { await store.installCredential() } }
                    .disabled(store.credentialInput.isEmpty)
                if store.presentation.gate == .configurationMismatch {
                    Button("Reset Disposable State", role: .destructive) {
                        Task { await store.reset() }
                    }
                }
                Spacer()
                Button("Connect") { Task { await store.connect() } }
                    .keyboardShortcut(.defaultAction)
            }
        }
        .padding(32)
        .frame(maxWidth: 620)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("setup.root")
    }
}

struct ConnectingView: View {
    @Bindable var store: ConversationStore

    var body: some View {
        VStack(spacing: 16) {
            if store.presentation.gate == .fatalProtocol {
                Image(systemName: "exclamationmark.triangle")
                    .font(.largeTitle)
                    .foregroundStyle(.orange)
            } else {
                ProgressView()
            }
            Text(store.presentation.banner?.title ?? "Connecting to Craxii")
                .font(.title2)
            if store.presentation.banner?.offersRetry == true {
                Button("Try Again") { Task { await store.connect() } }
            }
            if let error = store.presentation.error {
                SafeErrorView(error: error, dismiss: store.dismissError)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .accessibilityIdentifier("connection.banner")
    }
}

struct DiagnosticView: View {
    @Bindable var store: ConversationStore

    var body: some View {
        Form {
            Section("Backend") {
                TextField("Endpoint", text: $store.endpoint)
                    .accessibilityIdentifier("setup.endpoint")
                Button("Apply Endpoint") { Task { await store.applyEndpoint() } }
            }
            Section("Device Credential") {
                LabeledContent("Status", value: store.snapshot.credentialStatus.rawValue)
                SecureField("Provisioned bearer token", text: $store.credentialInput)
                    .accessibilityIdentifier("setup.credential")
                HStack {
                    Button("Save Credential") { Task { await store.installCredential() } }
                    Button("Delete Credential", role: .destructive) {
                        Task { await store.deleteCredential() }
                    }
                }
            }
            Section("Connection") {
                LabeledContent("State", value: store.snapshot.connectionState.rawValue)
                HStack {
                    Button("Connect / Retry") { Task { await store.connect() } }
                    Button("Reset Disposable State", role: .destructive) {
                        Task { await store.reset() }
                    }
                }
            }
            Section("Safe Diagnostics") {
                LabeledContent("App version", value: "0.0.1")
                LabeledContent("Protocol version", value: String(ProtocolConstants.version))
                LabeledContent("Connection", value: store.snapshot.connectionState.rawValue)
                LabeledContent("Generation", value: String(store.snapshot.generation))
                LabeledContent("Presentation revision", value: String(store.snapshot.presentationRevision))
                LabeledContent("Durable cursor", value: String(store.snapshot.projection.lastAppliedCursor.rawValue))
                LabeledContent("Messages", value: String(store.snapshot.projection.messages.count))
                LabeledContent("Work items", value: String(store.snapshot.projection.works.count))
                LabeledContent("Pending commands", value: String(store.snapshot.pendingCommands.count))
                if let detail = store.snapshot.lastBackendError {
                    LabeledContent("Public error code", value: detail.code)
                    LabeledContent("Request ID", value: detail.requestID.rawValue)
                }
            }
            if let error = store.presentation.error {
                Section("Last Safe Error") {
                    SafeErrorView(error: error, dismiss: store.dismissError)
                }
            }
        }
        .formStyle(.grouped)
        .padding()
        .frame(minWidth: 520, minHeight: 480)
        .accessibilityIdentifier("settings.diagnostics")
    }
}
