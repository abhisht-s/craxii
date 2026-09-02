import SwiftUI

struct DiagnosticView: View {
    @Bindable var store: ConversationStore

    var body: some View {
        Form {
            Section("Backend profile") {
                TextField("Endpoint", text: $store.endpoint)
                    .accessibilityIdentifier("diagnostic.endpoint")
                Button("Apply endpoint") { Task { await store.applyEndpoint() } }
                    .accessibilityIdentifier("diagnostic.applyEndpoint")
            }
            Section("Device credential") {
                LabeledContent("Status", value: store.snapshot.credentialStatus.rawValue)
                    .accessibilityIdentifier("diagnostic.credentialStatus")
                SecureField("Provisioned bearer token", text: $store.credentialInput)
                    .accessibilityIdentifier("diagnostic.credential")
                HStack {
                    Button("Install credential") { Task { await store.installCredential() } }
                        .accessibilityIdentifier("diagnostic.installCredential")
                    Button("Delete credential") { Task { await store.deleteCredential() } }
                        .accessibilityIdentifier("diagnostic.deleteCredential")
                }
            }
            Section("Connection") {
                LabeledContent("State", value: store.snapshot.connectionState.rawValue)
                    .accessibilityIdentifier("diagnostic.connectionState")
                HStack {
                    Button("Connect / retry") { Task { await store.connect() } }
                        .accessibilityIdentifier("diagnostic.connect")
                    Button("Reset disposable state") { Task { await store.reset() } }
                        .accessibilityIdentifier("diagnostic.reset")
                }
            }
            Section("Projection") {
                LabeledContent("Craxii ID", value: store.snapshot.projection.craxii?.craxiiID.rawValue ?? "—")
                LabeledContent("Conversation ID", value: store.snapshot.projection.primaryConversation?.conversationID.rawValue ?? "—")
                LabeledContent("Durable cursor", value: String(store.snapshot.projection.lastAppliedCursor.rawValue))
                LabeledContent("Messages", value: String(store.snapshot.projection.messages.count))
                LabeledContent("Work items", value: String(store.snapshot.projection.works.count))
                LabeledContent("Drafts", value: String(store.snapshot.drafts.count))
                LabeledContent("Pending commands", value: String(store.snapshot.pendingCommandCount))
            }
            Section("Last safe error") {
                Text((store.localActionError ?? store.snapshot.lastError)?.description ?? "none")
                    .accessibilityIdentifier("diagnostic.lastError")
            }
        }
        .formStyle(.grouped)
        .padding()
        .accessibilityIdentifier("diagnostic.root")
    }
}
