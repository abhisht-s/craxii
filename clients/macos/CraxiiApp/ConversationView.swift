import SwiftUI
import CraxiiProtocol
import CraxiiClientCore
import CraxiiPresentation

private let transcriptBottomID = "conversation.transcript.bottom"

struct ConversationRootView: View {
    @Bindable var store: ConversationStore

    var body: some View {
        Group {
            if store.presentation.hasUsableProjection {
                ConversationView(store: store)
            } else {
                switch store.presentation.gate {
                case .setup, .credentialRepair, .configurationMismatch:
                    SetupView(store: store)
                case .connecting, .usable, .fatalProtocol:
                    ConnectingView(store: store)
                }
            }
        }
    }
}

struct ConversationView: View {
    @Bindable var store: ConversationStore

    var body: some View {
        VStack(spacing: 0) {
            ConversationHeader(store: store)
            if let banner = store.presentation.banner {
                ConnectionBannerView(banner: banner) {
                    Task { await store.connect() }
                }
            }
            TranscriptView(store: store)
            if let error = store.presentation.error {
                SafeErrorView(error: error, dismiss: store.dismissError)
                    .padding(.horizontal, 16)
                    .padding(.bottom, 8)
            }
            ComposerView(store: store)
        }
        .background(Color(nsColor: .windowBackgroundColor))
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("conversation.root")
    }
}

private struct ConversationHeader: View {
    @Bindable var store: ConversationStore

    var body: some View {
        HStack {
            VStack(alignment: .leading, spacing: 2) {
                Text(store.presentation.craxiiName)
                    .font(.title2.weight(.semibold))
                Text("One continuous conversation")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Spacer()
            SettingsLink {
                Label("Settings", systemImage: "gearshape")
                    .labelStyle(.iconOnly)
            }
            .help("Craxii Settings")
            .accessibilityLabel("Open Craxii settings")
        }
        .padding(.horizontal, 18)
        .padding(.vertical, 12)
        .background(.bar)
    }
}

private struct ConnectionBannerView: View {
    let banner: ConnectionBanner
    let retry: () -> Void

    var body: some View {
        HStack(spacing: 10) {
            if banner.kind == .refreshing || banner.kind == .reconnecting {
                ProgressView().controlSize(.small)
            } else {
                Image(systemName: banner.kind == .blocking ? "exclamationmark.triangle" : "wifi.slash")
            }
            VStack(alignment: .leading, spacing: 2) {
                Text(banner.title).font(.callout.weight(.medium))
                if let detail = banner.detail {
                    Text(detail).font(.caption).foregroundStyle(.secondary)
                }
            }
            Spacer()
            if banner.offersRetry {
                Button("Try Again", action: retry)
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 9)
        .background(.thinMaterial)
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("connection.banner")
    }
}

private struct TranscriptView: View {
    @Bindable var store: ConversationStore
    @State private var nearBottom = true
    @State private var didInitialScroll = false
    @State private var unseenActivity = 0

    var body: some View {
        GeometryReader { viewport in
            ScrollViewReader { reader in
                ScrollView {
                    LazyVStack(spacing: 14) {
                        if store.presentation.transcript.isEmpty
                            && store.presentation.works.isEmpty {
                            EmptyConversationView(name: store.presentation.craxiiName)
                        }
                        ForEach(store.presentation.transcript) { row in
                            TranscriptRowView(row: row)
                                .id(row.id)
                        }
                        if !store.presentation.works.isEmpty {
                            VStack(alignment: .leading, spacing: 10) {
                                Text("Activity")
                                    .font(.caption.weight(.semibold))
                                    .foregroundStyle(.secondary)
                                ForEach(store.presentation.works) { work in
                                    WorkCardView(
                                        work: work,
                                        isPreparingCancellation: store.cancellingWorkIDs.contains(work.workID),
                                        cancel: { Task { await store.cancel(workID: work.workID) } })
                                        .id("work-\(work.id)")
                                }
                            }
                            .frame(maxWidth: 620, alignment: .leading)
                        }
                        Color.clear
                            .frame(height: 1)
                            .id(transcriptBottomID)
                            .background(
                                GeometryReader { proxy in
                                    Color.clear.preference(
                                        key: TranscriptBottomPreferenceKey.self,
                                        value: proxy.frame(in: .named("transcript-scroll")).maxY)
                                })
                    }
                    .padding(.horizontal, 20)
                    .padding(.vertical, 18)
                    .frame(maxWidth: .infinity)
                }
                .coordinateSpace(name: "transcript-scroll")
                .accessibilityElement(children: .contain)
                .accessibilityIdentifier("conversation.transcript")
                .overlay(alignment: .bottomTrailing) {
                    if unseenActivity > 0 {
                        Button {
                            withAnimation { reader.scrollTo(transcriptBottomID, anchor: .bottom) }
                            unseenActivity = 0
                            nearBottom = true
                        } label: {
                            Label(
                                unseenActivity == 1 ? "New activity" : "\(unseenActivity) new activities",
                                systemImage: "arrow.down")
                        }
                        .buttonStyle(.borderedProminent)
                        .padding()
                        .accessibilityLabel("New activity, \(unseenActivity)")
                    }
                }
                .onPreferenceChange(TranscriptBottomPreferenceKey.self) { bottom in
                    let distance = max(0, bottom - viewport.size.height)
                    nearBottom = TranscriptScrollPolicy.isNearBottom(distance: distance)
                    if nearBottom { unseenActivity = 0 }
                }
                .task {
                    guard !didInitialScroll else { return }
                    didInitialScroll = true
                    await Task.yield()
                    reader.scrollTo(transcriptBottomID, anchor: .bottom)
                }
                .onChange(of: store.presentation.activityFingerprint) {
                    let action = TranscriptScrollPolicy.action(
                        initialLoad: false, userSubmitted: false, activityChanged: true,
                        isNearBottom: nearBottom)
                    switch action {
                    case .scrollToBottom:
                        withAnimation { reader.scrollTo(transcriptBottomID, anchor: .bottom) }
                    case .recordUnseenActivity:
                        unseenActivity += 1
                    case .none:
                        break
                    }
                }
                .onChange(of: store.transcriptScrollRequest) {
                    withAnimation { reader.scrollTo(transcriptBottomID, anchor: .bottom) }
                    unseenActivity = 0
                }
            }
        }
    }
}

private struct TranscriptBottomPreferenceKey: PreferenceKey {
    static let defaultValue: CGFloat = 0
    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) {
        value = nextValue()
    }
}

private struct EmptyConversationView: View {
    let name: String

    var body: some View {
        VStack(spacing: 10) {
            Image(systemName: "bubble.left.and.bubble.right")
                .font(.system(size: 30))
                .foregroundStyle(.secondary)
            Text("Start a conversation with \(name)")
                .font(.title3.weight(.medium))
            Text("Messages stay separate and run in the order they are accepted.")
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
        }
        .padding(.vertical, 60)
        .frame(maxWidth: .infinity)
        .accessibilityIdentifier("conversation.empty")
    }
}

private struct TranscriptRowView: View {
    let row: TranscriptRow

    var body: some View {
        HStack {
            if row.role == .user { Spacer(minLength: 80) }
            VStack(alignment: .leading, spacing: 7) {
                if !row.text.isEmpty {
                    Text(row.text)
                        .textSelection(.enabled)
                }
                if !row.refusal.isEmpty {
                    Text(row.refusal)
                        .italic()
                        .textSelection(.enabled)
                        .accessibilityLabel("Refusal: \(row.refusal)")
                }
                if let status = row.status {
                    HStack(spacing: 5) {
                        if row.kind == .draft { ProgressView().controlSize(.mini) }
                        Text(status)
                    }
                    .font(.caption)
                    .foregroundStyle(.secondary)
                }
            }
            .padding(.horizontal, 13)
            .padding(.vertical, 10)
            .background(bubbleBackground)
            .clipShape(RoundedRectangle(cornerRadius: 13, style: .continuous))
            .overlay {
                if !row.isCanonical {
                    RoundedRectangle(cornerRadius: 13, style: .continuous)
                        .strokeBorder(
                            .secondary.opacity(0.45),
                            style: StrokeStyle(lineWidth: 1, dash: [4]))
                }
            }
            .frame(maxWidth: 620, alignment: row.role == .user ? .trailing : .leading)
            .accessibilityElement(children: .combine)
            .accessibilityIdentifier("transcript.row.\(row.id)")
            .accessibilityLabel(accessibilityLabel)
            .accessibilityValue(
                [row.text, row.refusal, row.status ?? ""]
                    .filter { !$0.isEmpty }.joined(separator: ", "))
            if row.role != .user { Spacer(minLength: 80) }
        }
        .frame(maxWidth: .infinity)
    }

    private var bubbleBackground: AnyShapeStyle {
        row.role == .user
            ? AnyShapeStyle(Color.accentColor.opacity(row.isCanonical ? 0.18 : 0.10))
            : AnyShapeStyle(Color(nsColor: .controlBackgroundColor))
    }

    private var accessibilityLabel: String {
        switch row.kind {
        case .canonical:
            row.role == .user ? "Your committed message" : "Craxii committed message"
        case .optimistic:
            "Your local message, not yet canonical"
        case .draft:
            "Craxii draft, not yet committed"
        }
    }
}

private struct WorkCardView: View {
    let work: WorkPresentation
    let isPreparingCancellation: Bool
    let cancel: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Image(systemName: icon)
                    .foregroundStyle(iconColor)
                    .accessibilityHidden(true)
                VStack(alignment: .leading, spacing: 2) {
                    Text(work.title).font(.callout.weight(.medium))
                    Text("Work \(work.ordinal)")
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                }
                Spacer()
                if work.canCancel || isPreparingCancellation {
                    Button("Cancel", action: cancel)
                        .disabled(isPreparingCancellation)
                        .accessibilityIdentifier("work.cancel")
                        .accessibilityLabel("Cancel work \(work.ordinal)")
                }
            }
            if let detail = work.detail {
                Text(detail).font(.caption).foregroundStyle(.secondary)
            }
            if isPreparingCancellation {
                Text("Saving cancellation request…")
                    .font(.caption).foregroundStyle(.secondary)
            } else if let cancellation = work.cancellationStatus {
                Text(cancellation).font(.caption).foregroundStyle(.secondary)
            }
            ForEach(work.tools) { tool in
                HStack(alignment: .top, spacing: 7) {
                    Image(systemName: tool.isUnknownOutcome ? "questionmark.circle" : "wrench.and.screwdriver")
                    VStack(alignment: .leading, spacing: 1) {
                        Text(tool.title).font(.caption.weight(.medium))
                        if let detail = tool.detail {
                            Text(detail).font(.caption2).foregroundStyle(.secondary)
                        }
                    }
                }
                .accessibilityElement(children: .combine)
            }
        }
        .padding(11)
        .background(.thinMaterial)
        .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Work \(work.ordinal), \(work.title)")
    }

    private var icon: String {
        switch work.tone {
        case .neutral: "clock"
        case .active: "sparkles"
        case .success: "checkmark.circle"
        case .warning: "exclamationmark.triangle"
        case .failure: "xmark.circle"
        }
    }

    private var iconColor: Color {
        switch work.tone {
        case .success: .green
        case .warning: .orange
        case .failure: .red
        default: .secondary
        }
    }
}

private struct ComposerView: View {
    @Bindable var store: ConversationStore
    @FocusState private var focused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .bottom, spacing: 10) {
                TextEditor(text: $store.composerText)
                    .font(.body)
                    .scrollContentBackground(.hidden)
                    .frame(minHeight: 58, maxHeight: 150)
                    .padding(6)
                    .background(Color(nsColor: .textBackgroundColor))
                    .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
                    .overlay {
                        RoundedRectangle(cornerRadius: 10, style: .continuous)
                            .stroke(.separator, lineWidth: 1)
                    }
                    .focused($focused)
                    .disabled(!store.presentation.mutationsAllowed || store.isPreparingMessage)
                    .onKeyPress(.return, phases: .down) { press in
                        let action = ComposerPolicy.returnAction(
                            commandModifier: press.modifiers.contains(.command))
                        guard action == .send else { return .ignored }
                        guard store.canSend else { return .handled }
                        Task { await store.sendComposer() }
                        return .handled
                    }
                    .accessibilityLabel("Message Craxii")
                    .accessibilityIdentifier("composer.editor")
                Button {
                    Task { await store.sendComposer() }
                } label: {
                    Image(systemName: "arrow.up.circle.fill")
                        .font(.system(size: 25))
                }
                .buttonStyle(.plain)
                .disabled(!store.canSend)
                .help("Send (Command-Return)")
                .accessibilityLabel("Send message")
                .accessibilityIdentifier("composer.send")
            }
            HStack {
                Text(composerFeedback)
                    .font(.caption)
                    .foregroundStyle(feedbackIsError ? Color.red : Color.secondary)
                Spacer()
                Text("⌘↩ Send · ↩ New line")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 12)
        .background(.bar)
        .onChange(of: store.composerFocusRevision) {
            focused = true
        }
    }

    private var composerFeedback: String {
        switch store.composerValidation {
        case .empty, .whitespaceOnly:
            ""
        case let .valid(_, feedback):
            feedback ?? ""
        case let .overLimit(bytes, limit):
            "\(bytes.formatted()) bytes — limit \(limit.formatted())"
        }
    }

    private var feedbackIsError: Bool {
        if case .overLimit = store.composerValidation { return true }
        return false
    }
}

struct SafeErrorView: View {
    let error: ErrorPresentation
    let dismiss: () -> Void

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: "exclamationmark.circle")
                .foregroundStyle(.orange)
            VStack(alignment: .leading, spacing: 3) {
                Text(error.title).font(.callout.weight(.medium))
                Text(error.message).font(.caption).foregroundStyle(.secondary)
                if let code = error.code {
                    Text("Code: \(code)").font(.caption2).textSelection(.enabled)
                }
                if let requestID = error.requestID {
                    Text("Request ID: \(requestID)").font(.caption2).textSelection(.enabled)
                }
            }
            Spacer()
            Button("Dismiss", systemImage: "xmark", action: dismiss)
                .labelStyle(.iconOnly)
                .buttonStyle(.plain)
                .accessibilityLabel("Dismiss error")
        }
        .padding(10)
        .background(.thinMaterial)
        .clipShape(RoundedRectangle(cornerRadius: 9, style: .continuous))
        .accessibilityElement(children: .contain)
    }
}
