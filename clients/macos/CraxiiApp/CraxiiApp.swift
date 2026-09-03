import SwiftUI
import AppKit

@MainActor
final class CraxiiApplicationDelegate: NSObject, NSApplicationDelegate {
    weak var store: ConversationStore?
    private var terminationPending = false

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        guard let store else { return .terminateNow }
        guard !terminationPending else { return .terminateLater }
        terminationPending = true
        Task { @MainActor in
            await store.shutdown()
            sender.reply(toApplicationShouldTerminate: true)
        }
        return .terminateLater
    }
}

@main
struct CraxiiApp: App {
    @NSApplicationDelegateAdaptor(CraxiiApplicationDelegate.self) private var appDelegate
    @State private var store = ConversationStore()

    var body: some Scene {
        WindowGroup("Craxii") {
            ConversationRootView(store: store)
                .frame(minWidth: 680, minHeight: 560)
                .task {
                    appDelegate.store = store
                    await store.launch()
                }
        }
        .windowResizability(.contentMinSize)

        Settings {
            DiagnosticView(store: store)
        }
    }
}
